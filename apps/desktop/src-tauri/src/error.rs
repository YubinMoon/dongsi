//! 화면(WebView)으로 돌려주는 오류. `{ "message": "...", "code": "..." }` 형태로 직렬화된다.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CmdError {
    message: String,
    /// 화면이 따로 처리해야 하는 오류(예: `passwordRequired`, `wrongPassword`). 없으면 생략한다.
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
}

impl CmdError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
        }
    }

    pub fn with_code(message: impl Into<String>, code: &'static str) -> Self {
        Self {
            message: message.into(),
            code: Some(code),
        }
    }
}

impl std::fmt::Display for CmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// 코어 크레이트의 오류는 사용자에게 보여 줄 한국어 메시지를 담고 있으므로 그대로 전달한다.
impl<E: std::error::Error> From<E> for CmdError {
    fn from(err: E) -> Self {
        Self::new(err.to_string())
    }
}

pub type CmdResult<T> = Result<T, CmdError>;
