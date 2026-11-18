//! 화면(WebView)으로 돌려주는 오류. `{ "message": "..." }` 형태로 직렬화된다.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CmdError {
    message: String,
}

impl CmdError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// 코어 크레이트의 오류는 사용자에게 보여 줄 한국어 메시지를 담고 있으므로 그대로 전달한다.
impl<E: std::error::Error> From<E> for CmdError {
    fn from(err: E) -> Self {
        Self::new(err.to_string())
    }
}

pub type CmdResult<T> = Result<T, CmdError>;
