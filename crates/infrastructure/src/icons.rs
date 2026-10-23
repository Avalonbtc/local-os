//! Coin icon sources outside the database: the controller's icon directory (drop `KAS.svg`,
//! `TSC.png`, ... in and they appear), image links and CoinGecko. Only the controller talks to
//! the outside; browsers load every icon from the controller.
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use rig_domain::*;
use serde_json::Value;
use std::{path::PathBuf, time::Duration};

const EXTENSIONS: [(&str, &str); 5] = [
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("webp", "image/webp"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
];

pub struct IconStore {
    pub dir: PathBuf,
    /// CoinGecko API base (`https://api.coingecko.com`); `None` disables search.
    pub coingecko: Option<String>,
    /// Optional CoinGecko demo / pro key, sent as `x-cg-demo-api-key`.
    pub coingecko_key: Option<String>,
    http: reqwest::Client,
}

impl IconStore {
    pub fn new(
        dir: PathBuf,
        coingecko: Option<String>,
        coingecko_key: Option<String>,
    ) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::limited(3))
            .user_agent(concat!("RigDeck/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            dir,
            coingecko: coingecko.map(|url| url.trim_end_matches('/').to_string()),
            coingecko_key,
            http,
        })
    }
    /// `KAS.svg` → (`KAS`, path, media type); files with other names or types are ignored.
    async fn files(&self) -> Vec<(String, PathBuf, &'static str, Option<DateTime<Utc>>)> {
        let mut found = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(&self.dir).await else {
            return found;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let (Some(stem), Some(ext)) = (
                path.file_stem().and_then(|s| s.to_str()),
                path.extension().and_then(|s| s.to_str()),
            ) else {
                continue;
            };
            let Some((_, kind)) = EXTENSIONS.iter().find(|(e, _)| e.eq_ignore_ascii_case(ext))
            else {
                continue;
            };
            let Ok(symbol) = rig_application::coin_icons::valid_symbol(stem) else {
                continue;
            };
            let modified = entry
                .metadata()
                .await
                .ok()
                .and_then(|m| m.modified().ok())
                .map(DateTime::<Utc>::from);
            found.push((symbol, path, *kind, modified));
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }
    async fn get_json(&self, url: &str) -> Result<Value> {
        let mut request = self.http.get(url).header("accept", "application/json");
        if let Some(key) = &self.coingecko_key {
            request = request.header("x-cg-demo-api-key", key);
        }
        let response = request
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("无法连接 CoinGecko：{e}")))?;
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(Error::Unavailable(
                "CoinGecko 请求过于频繁，请稍后再试".into(),
            ));
        }
        if !response.status().is_success() {
            return Err(Error::Unavailable(format!(
                "CoinGecko 返回 {}",
                response.status()
            )));
        }
        response
            .json()
            .await
            .map_err(|e| Error::Unavailable(format!("CoinGecko 响应无效：{e}")))
    }
    fn api(&self) -> Result<&str> {
        self.coingecko
            .as_deref()
            .ok_or_else(|| Error::Unavailable("主控未启用 CoinGecko 搜索".into()))
    }
}

#[async_trait]
impl IconSource for IconStore {
    fn directory(&self) -> Option<String> {
        Some(self.dir.display().to_string())
    }
    fn search_enabled(&self) -> bool {
        self.coingecko.is_some()
    }
    async fn directory_icons(&self) -> Result<Vec<CoinIcon>> {
        let mut icons: Vec<CoinIcon> = Vec::new();
        for (symbol, path, kind, modified) in self.files().await {
            // KAS.svg and KAS.png: the first one (svg before png) wins.
            if icons.iter().any(|i| i.symbol == symbol) {
                continue;
            }
            icons.push(CoinIcon {
                symbol,
                source: "directory".into(),
                content_type: kind.into(),
                source_ref: path.file_name().map(|n| n.to_string_lossy().into_owned()),
                updated_at: modified.unwrap_or_else(Utc::now),
            });
        }
        Ok(icons)
    }
    async fn directory_icon(&self, symbol: &str) -> Result<Option<(CoinIcon, Vec<u8>)>> {
        let files = self.files().await;
        let Some((symbol, path, kind, modified)) = EXTENSIONS
            .iter()
            .find_map(|(ext, _)| {
                files.iter().find(|f| {
                    f.0 == symbol && f.1.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
                })
            })
            .cloned()
        else {
            return Ok(None);
        };
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|e| Error::Unavailable(format!("读取图标文件失败：{e}")))?;
        // A bad file in the directory is skipped, never served.
        let Ok(content_type) = rig_application::coin_icons::valid_image(kind, &bytes) else {
            tracing::warn!(path = %path.display(), "ignored invalid coin icon file");
            return Ok(None);
        };
        Ok(Some((
            CoinIcon {
                symbol,
                source: "directory".into(),
                content_type,
                source_ref: path.file_name().map(|n| n.to_string_lossy().into_owned()),
                updated_at: modified.unwrap_or_else(Utc::now),
            },
            bytes,
        )))
    }
    async fn download(&self, url: &str) -> Result<IconImage> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| Error::Unavailable(format!("下载图片失败：{e}")))?;
        if !response.status().is_success() {
            return Err(Error::Unavailable(format!(
                "下载图片失败：HTTP {}",
                response.status()
            )));
        }
        if response
            .content_length()
            .is_some_and(|n| n as usize > COIN_ICON_MAX_BYTES)
        {
            return Err(Error::Validation("图片超过 256 KB".into()));
        }
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| Error::Unavailable(format!("下载图片失败：{e}")))?;
            if bytes.len() + chunk.len() > COIN_ICON_MAX_BYTES {
                return Err(Error::Validation("图片超过 256 KB".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(IconImage {
            content_type,
            bytes,
        })
    }
    async fn search(&self, query: &str) -> Result<Vec<CoinCandidate>> {
        let url = reqwest::Url::parse_with_params(
            &format!("{}/api/v3/search", self.api()?),
            &[("query", query)],
        )
        .map_err(|e| Error::Internal(e.to_string()))?;
        let body = self.get_json(url.as_str()).await?;
        let coins: Vec<&Value> = body["coins"]
            .as_array()
            .map(|a| a.iter().take(8).collect())
            .unwrap_or_default();
        // Thumbnails go through the controller too, so the browser never contacts CoinGecko.
        let thumbs = futures_util::future::join_all(coins.iter().map(|coin| async move {
            let url = coin["large"].as_str().or_else(|| coin["thumb"].as_str())?;
            let image = self.download(url).await.ok()?;
            let kind =
                rig_application::coin_icons::valid_image(&image.content_type, &image.bytes).ok()?;
            Some(format!(
                "data:{kind};base64,{}",
                STANDARD.encode(&image.bytes)
            ))
        }))
        .await;
        Ok(coins
            .iter()
            .zip(thumbs)
            .filter_map(|(coin, thumb)| {
                Some(CoinCandidate {
                    id: coin["id"].as_str()?.to_string(),
                    name: coin["name"].as_str().unwrap_or("").to_string(),
                    symbol: coin["symbol"].as_str().unwrap_or("").to_ascii_uppercase(),
                    market_cap_rank: coin["market_cap_rank"].as_u64().map(|n| n as u32),
                    thumb,
                })
            })
            .collect())
    }
    async fn coingecko_image(&self, id: &str) -> Result<IconImage> {
        let body = self
            .get_json(&format!(
                "{}/api/v3/coins/{id}?localization=false&tickers=false&market_data=false&community_data=false&developer_data=false&sparkline=false",
                self.api()?
            ))
            .await?;
        let url = body["image"]["large"]
            .as_str()
            .or_else(|| body["image"]["small"].as_str())
            .ok_or_else(|| Error::Unavailable("CoinGecko 没有这个币种的图标".into()))?
            .to_string();
        self.download(&url).await
    }
}
