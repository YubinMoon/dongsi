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
//! 5. 방송 목록 썸네일(D-076): 서버가 [`ServerMsg::Welcome`]의 `features`에 [`FEATURE_THUMBNAILS`]를 넣으면
//!    방송자가 [`ClientMsg::Thumbnail`]로 JPEG을 올린다. 방송 목록 항목에는 그림 번호([`BroadcastInfo::thumbnail`])만
//!    싣고, 그림은 시청자가 [`ClientMsg::GetThumbnail`]로 요청해 [`ServerMsg::Thumbnail`]로 받는다.
//!
//! 호환(0.1.0): 서버는 모르는 클라이언트 메시지를 받으면 연결을 끊고, 클라이언트는 모르는 서버 메시지를 버린다.
//! 모르는 필드는 양쪽 모두 무시한다. 그래서 새 메시지는 상대가 안다고 알렸을 때만 보낸다.

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

/// 서버 기능 표시: 방송 목록 썸네일(D-076). [`ServerMsg::Welcome`]의 `features`에 있을 때만
/// [`ClientMsg::Thumbnail`]을 보낸다(0.1.0 서버는 모르는 메시지를 받으면 연결을 끊는다).
pub const FEATURE_THUMBNAILS: &str = "thumbnails";

/// 썸네일 JPEG 최대 크기(바이트, base64를 푼 값). 방송자는 48KB 안쪽으로 만든다.
pub const MAX_THUMBNAIL_BYTES: usize = 64 * 1024;

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
    /// (방송자) 방송 목록 썸네일을 올린다(D-076). `jpeg`는 [`encode_thumbnail`]로 싼 JPEG.
    /// 서버가 [`FEATURE_THUMBNAILS`]를 알렸을 때만 보낸다. 비밀번호 방송은 보내지 않는다(서버도 받지 않는다).
    Thumbnail {
        jpeg: String,
    },
    /// 방송 목록 항목의 썸네일을 요청한다. 답은 [`ServerMsg::Thumbnail`] 또는 `Error`(`NotFound`, `RateLimited`).
    /// 목록 항목에 `thumbnail` 번호가 있을 때만 보낸다(0.1.0 서버는 번호를 싣지 않는다).
    GetThumbnail {
        req: u32,
        broadcaster: InstallId,
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
        /// 서버가 아는 추가 기능(예: [`FEATURE_THUMBNAILS`]). 0.1.0 서버는 보내지 않는다(빈 목록).
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        features: Vec<String>,
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
    /// [`ClientMsg::GetThumbnail`]의 답. `id`는 목록 항목의 `thumbnail`과 같은 번호, `jpeg`는
    /// [`encode_thumbnail`]로 싼 JPEG.
    Thumbnail {
        req: u32,
        broadcaster: InstallId,
        id: u64,
        jpeg: String,
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
    /// 썸네일 번호(D-076). 방송자가 새 그림을 올릴 때마다 바뀐다(서버 안에서 겹치지 않음).
    /// 그림이 없거나 비밀번호 방송이면 없다. 0.1.0 서버는 싣지 않는다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<u64>,
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

/// 썸네일 JPEG을 메시지에 싣는 문자열(base64 표준, 패딩 있음)로 바꾼다. 화면은 그대로
/// `data:image/jpeg;base64,` 뒤에 붙여 쓴다.
pub fn encode_thumbnail(jpeg: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(jpeg)
}

/// 메시지의 썸네일을 푼다. base64가 틀리거나, 비었거나, [`MAX_THUMBNAIL_BYTES`]를 넘거나,
/// JPEG(SOI 표시 `FF D8 FF`)이 아니면 `None`.
pub fn decode_thumbnail(text: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    // 풀기 전에 길이로 먼저 거른다(base64는 3바이트를 4글자로 싼다).
    if text.len() > MAX_THUMBNAIL_BYTES.div_ceil(3) * 4 {
        return None;
    }
    let jpeg = base64::engine::general_purpose::STANDARD
        .decode(text)
        .ok()?;
    (jpeg.len() <= MAX_THUMBNAIL_BYTES && jpeg.starts_with(&[0xFF, 0xD8, 0xFF])).then_some(jpeg)
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

    fn sample_info(thumbnail: Option<u64>) -> BroadcastInfo {
        BroadcastInfo {
            broadcaster: InstallId::from_bytes([3; 16]),
            name: "철수".into(),
            codec: VideoCodec::H264,
            width: 1920,
            height: 1080,
            fps: 60,
            has_audio: true,
            locked: false,
            viewers: 2,
            started_at: 1_700_000_000,
            thumbnail,
        }
    }

    #[test]
    fn thumbnail_msgs_json_shape() {
        let up = ClientMsg::Thumbnail {
            jpeg: encode_thumbnail(&[0xFF, 0xD8, 0xFF, 0xE0]),
        };
        let json = serde_json::to_value(&up).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "thumbnail", "jpeg": "/9j/4A==" })
        );
        assert_eq!(serde_json::from_value::<ClientMsg>(json).unwrap(), up);

        let get = ClientMsg::GetThumbnail {
            req: 4,
            broadcaster: InstallId::from_bytes([1; 16]),
        };
        let json = serde_json::to_value(&get).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "getThumbnail", "req": 4, "broadcaster": "AQEBAQEBAQEBAQEBAQEBAQ" })
        );
        assert_eq!(serde_json::from_value::<ClientMsg>(json).unwrap(), get);

        let got = ServerMsg::Thumbnail {
            req: 4,
            broadcaster: InstallId::from_bytes([1; 16]),
            id: 77,
            jpeg: "/9j/4A==".into(),
        };
        let json = serde_json::to_value(&got).unwrap();
        assert_eq!(json["type"], "thumbnail");
        assert_eq!(json["id"], 77);
        assert_eq!(serde_json::from_value::<ServerMsg>(json).unwrap(), got);
    }

    #[test]
    fn new_fields_are_left_out_when_empty() {
        // 그림 번호가 없으면 목록 항목은 0.1.0과 같은 모양이다.
        let json = serde_json::to_value(sample_info(None)).unwrap();
        assert!(json.get("thumbnail").is_none());
        let json = serde_json::to_value(sample_info(Some(9))).unwrap();
        assert_eq!(json["thumbnail"], 9);

        let welcome = |features: Vec<String>| ServerMsg::Welcome {
            protocol: PROTOCOL_VERSION,
            server_name: "s".into(),
            server_version: "0.2.0".into(),
            can_publish: true,
            host_only: false,
            broadcasts: vec![],
            features,
        };
        let json = serde_json::to_value(welcome(vec![])).unwrap();
        assert!(json.get("features").is_none());
        let json = serde_json::to_value(welcome(vec![FEATURE_THUMBNAILS.into()])).unwrap();
        assert_eq!(json["features"], serde_json::json!(["thumbnails"]));
    }

    #[test]
    fn thumbnail_decoding_checks_size_and_jpeg_marker() {
        let jpeg = [&[0xFF, 0xD8, 0xFF][..], &[7u8; 100]].concat();
        assert_eq!(decode_thumbnail(&encode_thumbnail(&jpeg)), Some(jpeg));
        // 상한 바로 안쪽은 되고, 한 바이트라도 넘으면 안 된다.
        let mut big = vec![0u8; MAX_THUMBNAIL_BYTES];
        big[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
        assert!(decode_thumbnail(&encode_thumbnail(&big)).is_some());
        big.push(0);
        assert_eq!(decode_thumbnail(&encode_thumbnail(&big)), None);
        // JPEG이 아님, 빈 값, base64가 아님
        assert_eq!(decode_thumbnail(&encode_thumbnail(b"\x89PNG\r\n")), None);
        assert_eq!(decode_thumbnail(""), None);
        assert_eq!(decode_thumbnail("이건 base64가 아님"), None);
    }

    /// 0.1.0(태그 `v0.1.0`)의 메시지 정의를 그대로 옮긴 것. 새 메시지·필드가 0.1.0 쪽에서 어떻게 읽히는지 본다.
    mod v010 {
        use serde::{Deserialize, Serialize};

        use crate::id::InstallId;
        use crate::media::VideoCodec;

        #[derive(Debug, Serialize, Deserialize)]
        #[serde(
            tag = "type",
            rename_all = "camelCase",
            rename_all_fields = "camelCase"
        )]
        pub enum ClientMsg {
            Hello {
                protocol: u32,
                app_version: String,
                install_id: InstallId,
                display_name: String,
                connected_ip: Option<std::net::IpAddr>,
                host_token: Option<String>,
            },
            Publish {
                req: u32,
                offer: String,
                info: super::PublishInfo,
                password: Option<String>,
            },
            UpdatePublish {
                info: super::PublishInfo,
            },
            Unpublish,
            Watch {
                req: u32,
                broadcaster: InstallId,
                password: Option<String>,
                offer: String,
            },
            StopWatch {
                session: u64,
            },
            RequestKeyframe {
                session: u64,
            },
            Ping {
                nonce: u64,
            },
        }

        #[derive(Debug, Serialize, Deserialize)]
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
                can_publish: bool,
                host_only: bool,
                broadcasts: Vec<BroadcastInfo>,
            },
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
            BroadcastEnded {
                broadcaster: InstallId,
            },
            ViewerCount {
                count: u32,
            },
            Error {
                req: Option<u32>,
                code: super::ErrorCode,
                message: String,
            },
            Pong {
                nonce: u64,
            },
            Closing {
                reason: String,
            },
        }

        #[derive(Debug, Serialize, Deserialize)]
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
            pub started_at: u64,
        }
    }

    #[test]
    fn v010_client_reads_new_server_messages() {
        // 새 서버의 Welcome(기능 표시)·목록(그림 번호)은 0.1.0 클라이언트가 그대로 읽는다(모르는 필드는 무시).
        let welcome = ServerMsg::Welcome {
            protocol: PROTOCOL_VERSION,
            server_name: "s".into(),
            server_version: "0.2.0".into(),
            can_publish: true,
            host_only: false,
            broadcasts: vec![sample_info(Some(5))],
            features: vec![FEATURE_THUMBNAILS.into()],
        };
        let text = serde_json::to_string(&welcome).unwrap();
        match serde_json::from_str::<v010::ServerMsg>(&text).unwrap() {
            v010::ServerMsg::Welcome { broadcasts, .. } => {
                assert_eq!(broadcasts[0].name, "철수");
                assert_eq!(broadcasts[0].viewers, 2);
            }
            other => panic!("{other:?}"),
        }
        let list = ServerMsg::Broadcasts {
            broadcasts: vec![sample_info(Some(5))],
        };
        let text = serde_json::to_string(&list).unwrap();
        assert!(serde_json::from_str::<v010::ServerMsg>(&text).is_ok());
        // 그림(Thumbnail)은 0.1.0 클라이언트가 못 읽는다. 0.1.0 클라이언트(crates/net signal.rs)는 못 읽은 메시지를
        // 로그만 남기고 버리지만, 서버는 요청(GetThumbnail)한 접속에만 보내므로 0.1.0 클라이언트에게는 가지 않는다.
        let got = ServerMsg::Thumbnail {
            req: 1,
            broadcaster: InstallId::from_bytes([1; 16]),
            id: 5,
            jpeg: "/9j/".into(),
        };
        let text = serde_json::to_string(&got).unwrap();
        assert!(serde_json::from_str::<v010::ServerMsg>(&text).is_err());
    }

    #[test]
    fn v010_server_reads_new_client_messages() {
        // Hello는 바뀌지 않았다.
        let hello = ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            app_version: "0.2.0".into(),
            install_id: InstallId::from_bytes([2; 16]),
            display_name: "a".into(),
            connected_ip: None,
            host_token: None,
        };
        let text = serde_json::to_string(&hello).unwrap();
        assert!(matches!(
            serde_json::from_str::<v010::ClientMsg>(&text).unwrap(),
            v010::ClientMsg::Hello { .. }
        ));
        // 새 메시지는 0.1.0 서버가 못 읽는다. 0.1.0 서버(crates/server signaling.rs)는 못 읽은 메시지에서
        // 연결을 끊고 그 연결의 방송도 끝내므로, 클라이언트는 Welcome의 기능 표시가 있을 때만 보낸다.
        for msg in [
            ClientMsg::Thumbnail {
                jpeg: "/9j/".into(),
            },
            ClientMsg::GetThumbnail {
                req: 1,
                broadcaster: InstallId::from_bytes([1; 16]),
            },
        ] {
            let text = serde_json::to_string(&msg).unwrap();
            assert!(
                serde_json::from_str::<v010::ClientMsg>(&text).is_err(),
                "{text}"
            );
        }
    }

    #[test]
    fn new_client_reads_v010_server_messages() {
        // 0.1.0 서버의 Welcome에는 기능 표시가 없다 → 빈 목록(썸네일을 보내지 않는다).
        let old = v010::ServerMsg::Welcome {
            protocol: 1,
            server_name: "s".into(),
            server_version: "0.1.0".into(),
            can_publish: true,
            host_only: false,
            broadcasts: vec![v010::BroadcastInfo {
                broadcaster: InstallId::from_bytes([3; 16]),
                name: "철수".into(),
                codec: VideoCodec::H264,
                width: 1920,
                height: 1080,
                fps: 60,
                has_audio: true,
                locked: false,
                viewers: 2,
                started_at: 1_700_000_000,
            }],
        };
        let text = serde_json::to_string(&old).unwrap();
        match serde_json::from_str::<ServerMsg>(&text).unwrap() {
            ServerMsg::Welcome {
                features,
                broadcasts,
                ..
            } => {
                assert!(features.is_empty());
                assert_eq!(broadcasts, vec![sample_info(None)]);
            }
            other => panic!("{other:?}"),
        }
    }
}
