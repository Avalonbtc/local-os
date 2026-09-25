pub mod models;
pub mod ports;
pub use models::*;
pub use ports::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Validation(String),
    #[error("需要登录或令牌已失效")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(String),
    #[error("记录不存在")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Internal(String),
}
pub type Result<T> = std::result::Result<T, Error>;

pub fn valid_name(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(Error::Validation(
            "名称不能为空、包含控制字符或超过 128 字节".into(),
        ));
    }
    Ok(())
}

pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
