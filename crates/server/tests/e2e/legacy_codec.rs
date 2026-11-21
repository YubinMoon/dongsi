//! 0.1.0 클라이언트가 이 버전에 없는 코덱(VP8·VP9·H.265, D-075)으로 방송하려 할 때: 서버는 연결을 닫지 않고
//! 읽을 수 있는 오류(`BadRequest`)로 답한다. 0.1.0 앱은 방송 시작 오류의 메시지를 그대로 보여 준다.
//!
//! 지금 클라이언트(`dongsi_net::signal`)는 그런 메시지를 만들 수 없어서 WebSocket에 JSON을 직접 보낸다.

use std::time::Duration;

use dongsi_core::proto::{
    ClientMsg, ErrorCode, PROTOCOL_VERSION, PublishInfo, ServerMsg, TLS_SERVER_NAME, WS_PATH,
};
use dongsi_core::{Host, InstallId, VideoCodec};
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use crate::client;
use crate::target::Target;

type Ws = WebSocketStream<TlsStream<TcpStream>>;

/// 서버에 TLS·WebSocket으로 접속해 Hello를 보내고 Welcome까지 받는다.
async fn open_raw(t: &Target, name: &str) -> Ws {
    let host = match t.addr.host() {
        Host::Ip(ip) => ip.to_string(),
        Host::Domain(d) => d.clone(),
    };
    let tcp = TcpStream::connect((host.as_str(), t.addr.port()))
        .await
        .expect("TCP 접속");
    let ip = tcp.peer_addr().unwrap().ip();
    let (config, _) = dongsi_net::tls::client_config(Some(t.fp)).unwrap();
    let tls = TlsConnector::from(config)
        .connect(ServerName::try_from(TLS_SERVER_NAME).unwrap(), tcp)
        .await
        .expect("TLS 접속");
    let (mut ws, _) =
        tokio_tungstenite::client_async(format!("wss://{TLS_SERVER_NAME}{WS_PATH}"), tls)
            .await
            .expect("WebSocket 접속");
    let hello = client::hello(InstallId::generate(), name, PROTOCOL_VERSION, None, ip);
    send_json(&mut ws, serde_json::to_value(&hello).unwrap()).await;
    next_where(&mut ws, "Welcome", |m| {
        matches!(m, ServerMsg::Welcome { .. }).then_some(())
    })
    .await
    .expect("Welcome 전에 연결이 닫혔습니다");
    ws
}

async fn send_json(ws: &mut Ws, v: serde_json::Value) {
    ws.send(Message::text(v.to_string())).await.expect("보내기");
}

/// `f`가 `Some`을 돌려주는 서버 메시지를 10초 안에 기다린다. 연결이 닫히면 `None`.
async fn next_where<T>(
    ws: &mut Ws,
    what: &str,
    mut f: impl FnMut(&ServerMsg) -> Option<T>,
) -> Option<T> {
    let wait = async {
        while let Some(m) = ws.next().await {
            match m {
                Ok(Message::Text(text)) => {
                    if let Ok(msg) = serde_json::from_str::<ServerMsg>(&text)
                        && let Some(v) = f(&msg)
                    {
                        return Some(v);
                    }
                }
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
        None
    };
    tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .unwrap_or_else(|_| panic!("{what}이(가) 10초 안에 오지 않았습니다"))
}

/// 0.1.0 방송 메시지처럼 `info.codec`만 옛 이름으로 바꾼 JSON.
fn legacy_publish(req: u32, codec: &str) -> serde_json::Value {
    let mut v = serde_json::to_value(ClientMsg::Publish {
        req,
        offer: "v=0".into(),
        info: crate::media::info(VideoCodec::H264),
        password: None,
    })
    .unwrap();
    v["info"]["codec"] = codec.into();
    v
}

fn legacy_update(codec: &str) -> serde_json::Value {
    let mut v = serde_json::to_value(ClientMsg::UpdatePublish {
        info: PublishInfo {
            codec: VideoCodec::H264,
            width: 1920,
            height: 1080,
            fps: 60,
            has_audio: true,
        },
    })
    .unwrap();
    v["info"]["codec"] = codec.into();
    v
}

/// 연결이 살아 있는지 Ping·Pong으로 본다.
async fn assert_alive(ws: &mut Ws, nonce: u64) {
    send_json(ws, serde_json::to_value(ClientMsg::Ping { nonce }).unwrap()).await;
    let got = next_where(ws, "Pong", |m| match m {
        ServerMsg::Pong { nonce: n } if *n == nonce => Some(()),
        _ => None,
    })
    .await;
    assert!(got.is_some(), "오류를 답한 뒤 연결이 닫혔습니다");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn legacy_codec_publish_gets_a_readable_error() {
    let t = Target::shared().await;
    let mut ws = open_raw(&t, "옛 방송자").await;

    for (req, codec, label) in [
        (41, "vp8", "VP8"),
        (42, "vp9", "VP9"),
        (43, "h265", "H.265"),
    ] {
        send_json(&mut ws, legacy_publish(req, codec)).await;
        let (code, message) = next_where(&mut ws, "방송 시작 오류", |m| match m {
            ServerMsg::Error {
                req: Some(r),
                code,
                message,
            } if *r == req => Some((*code, message.clone())),
            _ => None,
        })
        .await
        .unwrap_or_else(|| panic!("{codec} 방송 시작에 오류 대신 연결이 닫혔습니다"));
        eprintln!("[e2e] {codec} 방송 시작 → {code:?}: {message}");
        assert_eq!(code, ErrorCode::BadRequest);
        assert!(
            message.contains("H.264") && message.contains(label),
            "읽을 수 있는 오류가 아닙니다: {message}"
        );
        assert_alive(&mut ws, u64::from(req)).await;
    }

    // 방송 중 코덱 변경(UpdatePublish)은 req가 없다.
    send_json(&mut ws, legacy_update("vp8")).await;
    let message = next_where(&mut ws, "UpdatePublish 오류", |m| match m {
        ServerMsg::Error {
            req: None,
            code: ErrorCode::BadRequest,
            message,
        } => Some(message.clone()),
        _ => None,
    })
    .await
    .expect("UpdatePublish에 오류 대신 연결이 닫혔습니다");
    assert!(message.contains("VP8"), "{message}");
    assert_alive(&mut ws, 99).await;

    // 그 밖의 읽지 못하는 메시지는 전처럼 연결을 닫는다.
    send_json(&mut ws, serde_json::json!({ "type": "아무것도아님" })).await;
    let closed = next_where(&mut ws, "연결 닫힘", |_| None::<()>).await;
    assert!(closed.is_none());
}
