//! 화면(WebView)으로 돌려주는 오류.
//!
//! `{ "message": "...", "code"?: "...", "detail"?: {...} }` 형태로 직렬화된다.
//! `code`와 `detail`은 화면이 따로 처리해야 할 때만 들어가고, 없으면 생략한다.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CmdError {
    message: String,
    /// 화면이 따로 처리해야 하는 오류(예: `passwordRequired`, `wrongPassword`). 없으면 생략한다.
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
    /// `code`에 딸린 구조화된 정보(예: 지문 불일치의 저장·받은 지문). 없으면 생략한다.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<serde_json::Value>,
}

impl CmdError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            detail: None,
        }
    }

    pub fn with_code(message: impl Into<String>, code: &'static str) -> Self {
        Self {
            message: message.into(),
            code: Some(code),
            detail: None,
        }
    }

    /// 코드와 함께 구조화된 정보를 담는다. `detail`을 JSON으로 바꾸지 못하면 생략한다.
    pub fn with_detail(
        message: impl Into<String>,
        code: &'static str,
        detail: impl Serialize,
    ) -> Self {
        Self {
            message: message.into(),
            code: Some(code),
            detail: serde_json::to_value(detail).ok(),
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn to_json(e: &CmdError) -> serde_json::Value {
        serde_json::to_value(e).unwrap()
    }

    #[test]
    fn plain_and_coded_errors_keep_their_shape() {
        assert_eq!(
            to_json(&CmdError::new("실패")),
            json!({ "message": "실패" })
        );
        assert_eq!(
            to_json(&CmdError::with_code("비밀번호 필요", "passwordRequired")),
            json!({ "message": "비밀번호 필요", "code": "passwordRequired" })
        );
    }

    #[test]
    fn detail_is_serialized_when_present() {
        let e = CmdError::with_detail("지문 불일치", "fingerprintMismatch", json!({ "a": 1 }));
        assert_eq!(
            to_json(&e),
            json!({ "message": "지문 불일치", "code": "fingerprintMismatch", "detail": { "a": 1 } })
        );
    }
}
