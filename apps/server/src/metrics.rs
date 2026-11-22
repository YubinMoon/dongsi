//! Prometheus 지표 HTTP 엔드포인트(D-083).
//!
//! `GET /metrics`에 서버 지표([`dongsi_server::Metrics`])를 텍스트 형식(0.0.4)으로 내준다. 지표 하나만 내주는
//! 작은 HTTP/1.1 서버라서 요청마다 응답 하나를 보내고 연결을 닫는다(`Connection: close`).
//!
//! 인증과 TLS가 없으므로 믿을 수 있는 망(127.0.0.1, 사설망, Docker 내부 네트워크)에만 연다. 설정
//! (`--metrics-addr`, `DONGSI_METRICS_ADDR`, `metrics_addr`)을 주지 않으면 열지 않는다.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tracing::debug;

/// 요청 머리를 다 받을 때까지 기다리는 시간.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// 요청 머리를 이만큼까지만 읽는다(첫 줄만 쓴다).
const MAX_HEAD_BYTES: usize = 8 * 1024;
/// 동시에 처리하는 요청 수. 넘으면 바로 닫는다.
const MAX_CONCURRENT: usize = 8;
const METRICS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
const TEXT_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// 지표 HTTP 서버를 돌린다(끝나지 않는다. 멈추려면 작업을 abort한다). `render`가 지표 텍스트를 만든다.
pub async fn serve<F>(listener: TcpListener, render: F)
where
    F: Fn() -> String + Send + Sync + 'static,
{
    let render = Arc::new(render);
    let permits = Arc::new(Semaphore::new(MAX_CONCURRENT));
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                // 파일 핸들이 모자란 경우 등: 잠시 쉬었다가 다시 받는다.
                debug!("지표 접속 accept 실패: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            continue;
        };
        let render = render.clone();
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = handle(stream, render.as_ref()).await {
                debug!(%peer, "지표 요청 실패: {e}");
            }
        });
    }
}

async fn handle(
    mut stream: TcpStream,
    render: &(dyn Fn() -> String + Send + Sync),
) -> std::io::Result<()> {
    let Ok(head) = tokio::time::timeout(READ_TIMEOUT, read_head(&mut stream)).await else {
        return Ok(()); // 시간 초과: 응답 없이 닫는다
    };
    let response = respond(&head?, render);
    stream.write_all(&response).await?;
    stream.shutdown().await
}

/// 요청 머리(빈 줄까지)를 읽는다. 상대가 먼저 닫거나 [`MAX_HEAD_BYTES`]를 넘으면 거기까지 돌려준다.
async fn read_head(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut head = Vec::with_capacity(512);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(head);
        }
        head.extend_from_slice(&chunk[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") || head.len() >= MAX_HEAD_BYTES {
            return Ok(head);
        }
    }
}

/// 요청 머리를 보고 응답 전체(상태 줄, 헤더, 본문)를 만든다.
fn respond(head: &[u8], render: &(dyn Fn() -> String + Send + Sync)) -> Vec<u8> {
    let line = head.split(|b| *b == b'\n').next().unwrap_or_default();
    let line = String::from_utf8_lossy(line);
    let mut parts = line.trim_end().split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let path = target.split('?').next().unwrap_or("");
    let head_only = method == "HEAD";
    match (method, path) {
        ("GET" | "HEAD", "/metrics") => response(200, METRICS_CONTENT_TYPE, &render(), head_only),
        ("GET" | "HEAD", "/") => response(
            200,
            TEXT_CONTENT_TYPE,
            "dongsi-server: 지표는 /metrics\n",
            head_only,
        ),
        ("GET" | "HEAD", _) => response(404, TEXT_CONTENT_TYPE, "찾을 수 없습니다\n", head_only),
        ("", _) => response(400, TEXT_CONTENT_TYPE, "잘못된 요청입니다\n", false),
        _ => response(405, TEXT_CONTENT_TYPE, "GET만 받습니다\n", false),
    }
}

fn response(status: u16, content_type: &str, body: &str, head_only: bool) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Method Not Allowed",
    };
    let allow = if status == 405 {
        "Allow: GET, HEAD\r\n"
    } else {
        ""
    };
    let mut out = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{allow}Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    if !head_only {
        out.extend_from_slice(body.as_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render() -> String {
        "# HELP x 시험\n# TYPE x gauge\nx 1\n".into()
    }

    fn text(head: &str) -> String {
        String::from_utf8(respond(head.as_bytes(), &render)).unwrap()
    }

    #[test]
    fn routes() {
        let ok = text("GET /metrics HTTP/1.1\r\nHost: a\r\nAccept-Encoding: gzip\r\n\r\n");
        assert!(ok.starts_with("HTTP/1.1 200 OK\r\n"), "{ok}");
        assert!(ok.contains("Content-Type: text/plain; version=0.0.4; charset=utf-8\r\n"));
        assert!(ok.contains(&format!("Content-Length: {}\r\n", render().len())));
        assert!(ok.ends_with("\r\n\r\n# HELP x 시험\n# TYPE x gauge\nx 1\n"));
        // 질의 문자열은 무시한다.
        assert!(text("GET /metrics?x=1 HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 200 "));
        // HEAD는 본문 없이 같은 길이를 알린다.
        let head = text("HEAD /metrics HTTP/1.1\r\n\r\n");
        assert!(head.contains(&format!("Content-Length: {}\r\n", render().len())));
        assert!(head.ends_with("\r\n\r\n"));
        assert!(text("GET / HTTP/1.1\r\n\r\n").contains("/metrics"));
        assert!(text("GET /favicon.ico HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404 "));
        let post = text("POST /metrics HTTP/1.1\r\n\r\n");
        assert!(post.starts_with("HTTP/1.1 405 ") && post.contains("Allow: GET, HEAD\r\n"));
        assert!(text("").starts_with("HTTP/1.1 400 "));
    }

    #[tokio::test]
    async fn serves_over_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(serve(listener, render));
        for _ in 0..2 {
            let mut s = TcpStream::connect(addr).await.unwrap();
            s.write_all(b"GET /metrics HTTP/1.1\r\nHost: x\r\n\r\n")
                .await
                .unwrap();
            let mut got = String::new();
            tokio::time::timeout(Duration::from_secs(5), s.read_to_string(&mut got))
                .await
                .unwrap()
                .unwrap();
            assert!(got.starts_with("HTTP/1.1 200 OK\r\n"), "{got}");
            assert!(got.ends_with("x 1\n"), "{got}");
        }
        task.abort();
    }
}
