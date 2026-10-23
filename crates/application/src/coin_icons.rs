//! Coin icon library on the controller. Icons added from the console (upload, link, CoinGecko)
//! live in the database; files dropped into the icon directory are picked up as they are.
//! Database icons win over directory files; the console falls back to its bundled set.
use crate::App;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::Utc;
use rig_domain::*;

/// Upper-case symbols like `KAS`, `XMR`, `ZEPH`, `ETH-CLASSIC`.
pub fn valid_symbol(symbol: &str) -> Result<String> {
    let symbol = symbol.trim().to_ascii_uppercase();
    if symbol.is_empty()
        || symbol.len() > 20
        || !symbol
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err(Error::Validation(
            "币种符号只能包含字母、数字、- _ .，最长 20 个字符".into(),
        ));
    }
    Ok(symbol)
}

/// Check that the bytes really are a small PNG, JPEG, WebP or SVG image; returns the media type.
pub fn valid_image(content_type: &str, bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() || bytes.len() > COIN_ICON_MAX_BYTES {
        return Err(Error::Validation("图标需在 256 KB 以内".into()));
    }
    let sniffed = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
        let head = head.trim_start_matches('\u{feff}').trim_start();
        (head.starts_with("<svg")
            || (head.starts_with("<?xml") && head.contains("<svg"))
            || (head.starts_with("<!--") && head.contains("<svg")))
        .then_some("image/svg+xml")
    };
    let declared = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match sniffed {
        Some(kind)
            if declared.is_empty()
                || declared == kind
                || declared.starts_with("image/")
                || declared == "application/octet-stream"
                || declared == "text/xml"
                || declared == "text/plain" =>
        {
            Ok(kind.to_string())
        }
        _ => Err(Error::Validation(
            "只支持 PNG、JPEG、WebP 或 SVG 图片".into(),
        )),
    }
}

impl App {
    fn icon_source(&self) -> Result<&dyn IconSource> {
        self.icons
            .as_deref()
            .ok_or_else(|| Error::Unavailable("主控未启用币种图标库".into()))
    }
    pub async fn coin_icons(&self) -> Result<CoinIconList> {
        let mut icons = self.repository.coin_icons().await?;
        let (directory, search) = match &self.icons {
            Some(source) => {
                for icon in source.directory_icons().await? {
                    if !icons.iter().any(|i| i.symbol == icon.symbol) {
                        icons.push(icon);
                    }
                }
                (source.directory(), source.search_enabled())
            }
            None => (None, false),
        };
        icons.sort_by(|a, b| a.symbol.cmp(&b.symbol));
        Ok(CoinIconList {
            icons,
            directory,
            search,
        })
    }
    pub async fn coin_icon(&self, symbol: &str) -> Result<(CoinIcon, Vec<u8>)> {
        let symbol = valid_symbol(symbol)?;
        if let Some(found) = self.repository.coin_icon(&symbol).await? {
            return Ok(found);
        }
        if let Some(source) = &self.icons
            && let Some(found) = source.directory_icon(&symbol).await?
        {
            return Ok(found);
        }
        Err(Error::NotFound)
    }
    pub async fn save_coin_icon(
        &self,
        actor: &Actor,
        symbol: &str,
        input: CoinIconInput,
    ) -> Result<CoinIcon> {
        let symbol = valid_symbol(symbol)?;
        let (image, source, source_ref) = match input {
            CoinIconInput::Upload {
                content_type,
                data_base64,
            } => {
                let bytes = STANDARD
                    .decode(data_base64.trim())
                    .map_err(|_| Error::Validation("图片数据无效".into()))?;
                (
                    IconImage {
                        content_type,
                        bytes,
                    },
                    "upload",
                    None,
                )
            }
            CoinIconInput::Url { url } => {
                let url = url.trim().to_string();
                if !(url.starts_with("https://") || url.starts_with("http://")) || url.len() > 2048
                {
                    return Err(Error::Validation(
                        "图片链接需以 http:// 或 https:// 开头".into(),
                    ));
                }
                (self.icon_source()?.download(&url).await?, "url", Some(url))
            }
            CoinIconInput::Coingecko { id } => {
                let id = id.trim().to_string();
                if id.is_empty()
                    || id.len() > 100
                    || !id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                {
                    return Err(Error::Validation("CoinGecko 币种 id 无效".into()));
                }
                (
                    self.icon_source()?.coingecko_image(&id).await?,
                    "coingecko",
                    Some(id),
                )
            }
        };
        let content_type = valid_image(&image.content_type, &image.bytes)?;
        let icon = CoinIcon {
            symbol,
            source: source.into(),
            content_type,
            source_ref,
            updated_at: Utc::now(),
        };
        self.repository
            .save_coin_icon(&icon, &image.bytes, actor)
            .await?;
        Ok(icon)
    }
    pub async fn delete_coin_icon(&self, actor: &Actor, symbol: &str) -> Result<()> {
        let symbol = valid_symbol(symbol)?;
        self.repository.delete_coin_icon(&symbol, actor).await
    }
    pub async fn search_coins(&self, query: &str) -> Result<Vec<CoinCandidate>> {
        let query = query.trim();
        if query.is_empty() || query.chars().count() > 50 {
            return Err(Error::Validation("输入 1–50 个字符的币种名称或符号".into()));
        }
        let source = self.icon_source()?;
        if !source.search_enabled() {
            return Err(Error::Unavailable("主控未启用 CoinGecko 搜索".into()));
        }
        source.search(query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_are_upper_case_and_plain() {
        assert_eq!(valid_symbol(" kas ").unwrap(), "KAS");
        assert_eq!(valid_symbol("ETH-CLASSIC").unwrap(), "ETH-CLASSIC");
        for bad in ["", "../x", "a b", "<svg>", &"X".repeat(21)] {
            assert!(valid_symbol(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn images_are_sniffed_not_trusted() {
        let png = b"\x89PNG\r\n\x1a\n0000";
        assert_eq!(valid_image("image/png", png).unwrap(), "image/png");
        // A wrong declared type is corrected from the bytes.
        assert_eq!(valid_image("image/jpeg", png).unwrap(), "image/png");
        assert_eq!(
            valid_image("", b"<?xml version=\"1.0\"?><svg/>").unwrap(),
            "image/svg+xml"
        );
        assert_eq!(
            valid_image("text/plain", b"  <svg viewBox='0 0 1 1'/>").unwrap(),
            "image/svg+xml"
        );
        assert!(valid_image("text/html", b"<html><script>").is_err());
        assert!(valid_image("image/png", b"GIF89a").is_err());
        assert!(valid_image("image/png", &[]).is_err());
        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(COIN_ICON_MAX_BYTES + 1, 0);
        assert!(valid_image("image/png", &big).is_err());
    }
}
