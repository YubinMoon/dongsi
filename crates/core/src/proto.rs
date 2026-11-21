//! 시그널링 프로토콜.
//!
//! 클라이언트와 서버는 TCP(기본 7884) 위의 TLS 위에서 WebSocket 텍스트 메시지로 JSON을 주고받는다.
//! TLS 인증서는 자체 서명이고, 클라이언트는 인증서 SHA-256 지문으로 서버를 확인한다
//! (초대 코드의 지문으로 고정하거나, 처음 접속할 때 기억한다).
//!
//! 흐름:
//! 1. 접속하면 클라이언트가 [`ClientMsg::Hello`]를 보내고, 서버가 [`ServerMsg::Welcome`]으로 답한다.
//! 2. 서버는 방송 목록이 바뀔 때마다 [`ServerMsg::Broadcasts`]를 모든 접속자에게 보낸다.
//! 3. 방송: [`ClientMsg::Publish`](SDP offer) → [`ServerMsg::Published`](SDP answer). 미디어는 UDP(WebRTC)로 간다.
//! 4. 시청: [`ClientMsg::Watch`](SDP offer) → [`ServerMsg::Watching`](SDP answer).

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use crate::id::InstallId;
use crate::media::VideoCodec;

/// 프로토콜 버전. 호환되지 않게 바뀌면 올린다.
pub const PROTOCOL_VERSION: u32 = 1;

/// TLS SNI에 쓰는 이름. 인증서는 지문으로 확인하므로 이름 자체는 의미가 없다.
pub const TLS_SERVER_NAME: &str = "dongsi.invalid";

/// WebSocket 경로.
pub const WS_PATH: &str = "/dongsi";

/// 표시 이름 최대 길이(문자 수).
pub const MAX_DISPLAY_NAME_CHARS: usize = 32;

/// 방송 비밀번호 최대 길이(문자 수).
pub const MAX_PASSWORD_CHARS: usize = 128;

/// SDP 최대 길이(바이트).
pub const MAX_SDP_BYTES: usize = 64 * 1024;

/// 클라이언트 → 서버.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ClientMsg {
    /// 접속 직후 한 번 보낸다.
    Hello {
        protocol: u32,
        app_version: String,
        install_id: InstallId,
        display_name: String,
        /// 클라이언트가 접속에 쓴 서버 IP(클라이언트 쪽에서 본 TCP 상대 주소).
        /// 서버는 이 주소를 WebRTC 후보로 알려 준다(NAT·포트포워딩 대응).
        connected_ip: Option<IpAddr>,
        /// 내장 서버에서 호스트임을 증명하는 토큰. 호스트 앱만 안다.
        host_token: Option<String>,
    },
    /// 방송 시작. `offer`는 송출 클라이언트의 SDP offer.
    Publish {
        req: u32,
        offer: String,
        info: PublishInfo,
        password: Option<String>,
    },
    /// 방송 중 화질·코덱 정보가 바뀌었음을 알린다(목록 표시용).
    UpdatePublish {
        info: PublishInfo,
    },
    /// 방송 종료.
    Unpublish,
    /// 시청 시작. `offer`는 시청자 WebView2의 SDP offer(수신 전용).
    Watch {
        req: u32,
        broadcaster: InstallId,
        password: Option<String>,
        offer: String,
    },
    /// 시청 종료.
    StopWatch {
        session: u64,
    },
    /// 송출 클라이언트가 서버에 키프레임을 요청하는 경우는 없지만, 시청자 앱은 화면을 다시 그리고
    /// 싶을 때 이 메시지로 방송자에게 키프레임을 요청할 수 있다.
    RequestKeyframe {
        session: u64,
    },
    Ping {
        nonce: u64,
    },
}

/// 서버 → 클라이언트.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ServerMsg {
    Welcome {
        protocol: u32,
        server_name: String,
        server_version: String,
        /// 이 접속자가 방송할 수 있는지. 내장 서버에서는 호스트만 true.
        can_publish: bool,
        /// 내장 서버(호스트만 방송, 방송이 끝나면 닫힘)인지.
        host_only: bool,
        broadcasts: Vec<BroadcastInfo>,
    },
    /// 방송 목록 전체. 목록이 바뀔 때마다 보낸다.
    Broadcasts {
        broadcasts: Vec<BroadcastInfo>,
    },
    Published {
        req: u32,
        answer: String,
    },
    Watching {
        req: u32,
        session: u64,
        answer: String,
    },
    /// 보고 있던 방송이 끝났다.
    BroadcastEnded {
        broadcaster: InstallId,
    },
    /// (방송자에게) 현재 시청자 수.
    ViewerCount {
        count: u32,
    },
    Error {
        req: Option<u32>,
        code: ErrorCode,
        message: String,
    },
    Pong {
        nonce: u64,
    },
    /// 서버가 곧 닫힌다(내장 서버의 방송 종료 등).
    Closing {
        reason: String,
    },
}

/// 방송자가 알리는 스트림 정보.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishInfo {
    /// 0.1.0의 VP8·VP9·H.265는 받지 않는다(D-075). 그런 메시지는 [`unsupported_codec_error`]로 거절한다.
    #[serde(deserialize_with = "VideoCodec::deserialize_strict")]
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub has_audio: bool,
}

/// 방송 목록 항목.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BroadcastInfo {
    pub broadcaster: InstallId,
    pub name: String,
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub has_audio: bool,
    pub locked: bool,
    pub viewers: u32,
    /// 방송 시작 시각(유닉스 초).
    pub started_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    BadRequest,
    ProtocolMismatch,
    /// 방송 권한이 없다(내장 서버에서 호스트가 아님).
    NotAllowed,
    AlreadyPublishing,
    NotFound,
    PasswordRequired,
    WrongPassword,
    RateLimited,
    ServerFull,
    Internal,
}

impl ErrorCode {
    /// 사용자에게 보여 줄 기본 문구.
    pub fn default_message(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "잘못된 요청입니다",
            ErrorCode::ProtocolMismatch => "앱 버전이 서버와 맞지 않습니다. 앱을 업데이트해 주세요",
            ErrorCode::NotAllowed => "이 서버에서는 방송할 수 없습니다",
            ErrorCode::AlreadyPublishing => "이미 이 서버에서 방송 중입니다",
            ErrorCode::NotFound => "방송을 찾을 수 없습니다. 이미 끝났을 수 있습니다",
            ErrorCode::PasswordRequired => "비밀번호가 필요한 방송입니다",
            ErrorCode::WrongPassword => "비밀번호가 맞지 않습니다",
            ErrorCode::RateLimited => "시도가 너무 많습니다. 잠시 뒤에 다시 시도해 주세요",
            ErrorCode::ServerFull => "서버가 가득 찼습니다",
            ErrorCode::Internal => "서버 내부 오류입니다",
        }
    }
}


/// 이 버전에 없는 코덱(0.1.0의 VP8·VP9·H.265, D-075)으로 방송하려는 메시지(`publish`·`updatePublish`)에 돌려줄 오류.
///
/// [`PublishInfo`]의 코덱은 엄격하게 읽으므로 서버는 이런 메시지를 [`ClientMsg`]로 읽지 못한다. 그때 연결을 끊지
/// 않고 이 오류로 답한다(0.1.0 앱은 방송 시작 오류의 문구를 그대로 보인다). 다른 이유로 읽지 못한 메시지면 `None`.
pub fn unsupported_codec_error(text: &str) -> Option<ServerMsg> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if !matches!(v.get("type")?.as_str()?, "publish" | "updatePublish") {
        return None;
    }
    let name = v.get("info")?.get("codec")?.as_str()?;
    if VideoCodec::from_name(name).is_some() {
        return None;
    }
    let label: String = match name {
        "h265" => "H.265".into(),
        "vp8" => "VP8".into(),
        "vp9" => "VP9".into(),
        other => other.chars().take(16).collect(),
    };
    Some(ServerMsg::Error {
        req: v
            .get("req")
            .and_then(serde_json::Value::as_u64)
            .and_then(|r| u32::try_from(r).ok()),
        code: ErrorCode::BadRequest,
        message: format!(
            "이 서버는 H.264·AV1 방송만 받습니다(받은 코덱: {label}). 코덱을 H.264로 바꾸거나 앱을 업데이트해 주세요"
        ),
    })
}

/// 표시 이름을 정리한다: 제어 문자를 지우고, 앞뒤 공백을 지우고, 최대 길이로 자른다.
/// 비어 있으면 `None`.
pub fn sanitize_display_name(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_DISPLAY_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim().to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_msg_json_shape() {
        let msg = ClientMsg::Watch {
            req: 3,
            broadcaster: InstallId::from_bytes([1; 16]),
            password: None,
            offer: "v=0".into(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "watch");
        assert_eq!(json["broadcaster"], "AQEBAQEBAQEBAQEBAQEBAQ");
        assert_eq!(json["offer"], "v=0");
        let back: ClientMsg = serde_json::from_value(json).unwrap();
        assert_eq!(back, msg);

        let hello = ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            app_version: "0.1.0".into(),
            install_id: InstallId::from_bytes([2; 16]),
            display_name: "a".into(),
            connected_ip: Some("192.168.0.2".parse().unwrap()),
            host_token: None,
        };
        let json = serde_json::to_value(&hello).unwrap();
        assert_eq!(json["type"], "hello");
        assert_eq!(json["connectedIp"], "192.168.0.2");
        assert_eq!(json["appVersion"], "0.1.0");
    }

    #[test]
    fn server_msg_json_shape() {
        let msg = ServerMsg::Error {
            req: Some(1),
            code: ErrorCode::WrongPassword,
            message: "x".into(),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["code"], "wrongPassword");
        let msg = ServerMsg::Watching {
            req: 1,
            session: 9,
            answer: "v=0".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"watching\""));
        let back: ServerMsg = serde_json::from_str(&json).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn display_name_sanitizing() {
        assert_eq!(sanitize_display_name("  철수 \n").as_deref(), Some("철수"));
        assert_eq!(sanitize_display_name("\u{7}\u{1b}"), None);
        assert_eq!(sanitize_display_name("   "), None);
        let long = "가".repeat(40);
        assert_eq!(sanitize_display_name(&long).unwrap().chars().count(), 32);
    }

    #[test]
    fn legacy_codec_publish_is_rejected_with_a_readable_error() {
        // 0.1.0 앱이 VP8로 방송을 시작하는 메시지: ClientMsg로 읽지 못하고, 알아볼 수 있는 오류가 나온다.
        let publish = r#"{"type":"publish","req":5,"offer":"v=0","info":{"codec":"vp8","width":1280,"height":720,"fps":30,"hasAudio":true},"password":null}"#;
        assert!(serde_json::from_str::<ClientMsg>(publish).is_err());
        match unsupported_codec_error(publish) {
            Some(ServerMsg::Error { req, code, message }) => {
                assert_eq!((req, code), (Some(5), ErrorCode::BadRequest));
                assert!(
                    message.contains("H.264·AV1") && message.contains("VP8"),
                    "{message}"
                );
            }
            other => panic!("{other:?}"),
        }
        let update = r#"{"type":"updatePublish","info":{"codec":"h265","width":1,"height":1,"fps":1,"hasAudio":false}}"#;
        assert!(serde_json::from_str::<ClientMsg>(update).is_err());
        assert!(matches!(
            unsupported_codec_error(update),
            Some(ServerMsg::Error { req: None, ref message, .. }) if message.contains("H.265")
        ));
        // 지원하는 코덱이거나 다른 형식 오류면 None(서버는 전처럼 연결을 닫는다).
        let ok = publish.replace("vp8", "h264");
        assert!(serde_json::from_str::<ClientMsg>(&ok).is_ok());
        assert_eq!(unsupported_codec_error(&ok), None);
        assert_eq!(unsupported_codec_error(r#"{"type":"ping"}"#), None);
        assert_eq!(unsupported_codec_error("not json"), None);
        // 방송 목록(서버 → 클라이언트)에서는 0.1.0 코덱을 H.264로 읽는다.
        let list = r#"{"type":"broadcasts","broadcasts":[{"broadcaster":"AQEBAQEBAQEBAQEBAQEBAQ","name":"a","codec":"vp9","width":1,"height":1,"fps":1,"hasAudio":true,"locked":false,"viewers":0,"startedAt":0}]}"#;
        match serde_json::from_str::<ServerMsg>(list).unwrap() {
            ServerMsg::Broadcasts { broadcasts } => {
                assert_eq!(broadcasts[0].codec, VideoCodec::H264)
            }
            other => panic!("{other:?}"),
        }
    }

}
