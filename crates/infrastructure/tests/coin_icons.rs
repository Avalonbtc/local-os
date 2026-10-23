//! Coin icon sources: the icon directory, bounded link downloads and CoinGecko search / pick,
//! against a local fake server (no internet needed).
use rig_domain::*;
use rig_infrastructure::icons::IconStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-png-body";

/// Minimal HTTP/1.1 server: path → (status, content type, body).
async fn fake_server(routes: Vec<(&'static str, u16, &'static str, Vec<u8>)>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, kind, body) = routes
                    .iter()
                    .find(|(p, ..)| path.starts_with(p))
                    .map(|(_, s, k, b)| (*s, *k, b.clone()))
                    .unwrap_or((404, "text/plain", b"no".to_vec()));
                let head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });
    format!("http://{addr}")
}

fn temp_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rig-icons-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn directory_icons_are_listed_by_symbol_and_bad_files_are_skipped() {
    let dir = temp_dir();
    std::fs::write(
        dir.join("kas.svg"),
        b"<svg xmlns='http://www.w3.org/2000/svg'/>",
    )
    .unwrap();
    std::fs::write(dir.join("KAS.png"), PNG).unwrap();
    std::fs::write(dir.join("TSC.png"), PNG).unwrap();
    std::fs::write(dir.join("EVIL.svg"), b"<html><script>alert(1)</script>").unwrap();
    std::fs::write(dir.join("notes.txt"), b"x").unwrap();
    std::fs::write(dir.join("bad name.png"), PNG).unwrap();
    let store = IconStore::new(dir.clone(), None, None).unwrap();
    let symbols: Vec<String> = store
        .directory_icons()
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.symbol)
        .collect();
    assert_eq!(symbols, ["EVIL", "KAS", "TSC"]);
    let (kas, bytes) = store.directory_icon("KAS").await.unwrap().unwrap();
    assert_eq!(
        kas.content_type, "image/svg+xml",
        "svg wins over png for the same symbol"
    );
    assert!(bytes.starts_with(b"<svg"));
    assert_eq!(kas.source, "directory");
    assert!(
        store.directory_icon("EVIL").await.unwrap().is_none(),
        "invalid image never served"
    );
    assert!(store.directory_icon("XMR").await.unwrap().is_none());
    assert!(!store.search_enabled());
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn downloads_are_bounded_and_coingecko_goes_through_the_controller() {
    let big = vec![0u8; COIN_ICON_MAX_BYTES + 1];
    let base = fake_server(vec![
        ("/kas.png", 200, "image/png", PNG.to_vec()),
        ("/big.png", 200, "image/png", big),
        ("/missing.png", 404, "text/plain", b"gone".to_vec()),
    ])
    .await;
    let store = IconStore::new(temp_dir(), Some(base.clone()), None).unwrap();
    let image = store.download(&format!("{base}/kas.png")).await.unwrap();
    assert_eq!(image.bytes, PNG);
    assert!(matches!(
        store.download(&format!("{base}/big.png")).await,
        Err(Error::Validation(_))
    ));
    assert!(matches!(
        store.download(&format!("{base}/missing.png")).await,
        Err(Error::Unavailable(_))
    ));

    // A fake CoinGecko whose results point their images at the image server above.
    let search_json = format!(
        r#"{{"coins":[{{"id":"kaspa","name":"Kaspa","symbol":"kas","market_cap_rank":30,"large":"{0}/kas.png"}},{{"id":"kas-clone","name":"Clone","symbol":"KAS","market_cap_rank":null,"large":"{0}/missing.png"}}]}}"#,
        base
    );
    let coin_json = format!(r#"{{"image":{{"large":"{base}/kas.png"}}}}"#);
    let api = fake_server(vec![
        (
            "/api/v3/search",
            200,
            "application/json",
            search_json.into_bytes(),
        ),
        (
            "/api/v3/coins/kaspa",
            200,
            "application/json",
            coin_json.into_bytes(),
        ),
    ])
    .await;
    let store = IconStore::new(temp_dir(), Some(api), None).unwrap();
    assert!(store.search_enabled());
    let found = store.search("kas").await.unwrap();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].id, "kaspa");
    assert_eq!(found[0].symbol, "KAS");
    assert_eq!(found[0].market_cap_rank, Some(30));
    assert!(
        found[0]
            .thumb
            .as_deref()
            .unwrap()
            .starts_with("data:image/png;base64,")
    );
    assert!(
        found[1].thumb.is_none(),
        "a broken thumbnail is left out, not an error"
    );
    let picked = store.coingecko_image("kaspa").await.unwrap();
    assert_eq!(picked.bytes, PNG);
}
