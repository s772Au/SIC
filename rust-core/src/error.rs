use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    pub code: &'static str,
    pub message: String,
    pub status: u16,
}

impl CoreError {
    pub fn new(code: &'static str, message: impl Into<String>, status: u16) -> Self {
        Self {
            code,
            message: message.into(),
            status,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid", message, 422)
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new("conflict", message, 409)
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new("forbidden", message, 403)
    }
    pub fn storage() -> Self {
        Self::new("storage_error", "database operation failed", 500)
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CoreError {}
impl From<sqlx::Error> for CoreError {
    fn from(_: sqlx::Error) -> Self {
        Self::storage()
    }
}
impl From<serde_json::Error> for CoreError {
    fn from(_: serde_json::Error) -> Self {
        Self::invalid("invalid stored JSON")
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
