//! 시험용 클라이언트: 시그널링 연결 하나(설치 ID 하나)와 그 위의 방송·시청 도우미.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use dongsi_core::InstallId;
use dongsi_core::proto::{
    BroadcastInfo, ClientMsg, ErrorCode, PROTOCOL_VERSION, PublishInfo, ServerMsg,
};
use dongsi_net::publish::{PendingPublisher, Publisher};
use dongsi_net::signal::{self, Connection};

use crate::target::Target;
use crate::viewer::{PendingViewer, TestViewer};

/// 서버 메시지를 기다리는 기본 제한 시간.
pub const WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Welcome {
    pub protocol: u32,
    pub server_name: String,
    pub can_publish: bool,
    pub host_only: bool,
}

pub enum Next {
    Msg(ServerMsg),
    Closed,
    Timeout,
}

pub struct Client {
    pub conn: Connection,
    pub id: InstallId,
    pub name: String,
    pub welcome: Welcome,
    /// 마지막으로 받은 방송 목록(Welcome·Broadcasts)
    pub list: Vec<BroadcastInfo>,
    req: u32,
}

pub struct Watching {
    pub session: u64,
    pub viewer: TestViewer,
}

pub fn hello(
    id: InstallId,
    name: &str,
    protocol: u32,
    token: Option<&str>,
    ip: IpAddr,
) -> ClientMsg {
    ClientMsg::Hello {
        protocol,
        app_version: "test".into(),
        install_id: id,
        display_name: name.into(),
        connected_ip: Some(ip),
        host_token: token.map(Into::into),
    }
}

pub fn find(list: &[BroadcastInfo], id: InstallId) -> Option<&BroadcastInfo> {
    list.iter().find(|b| b.broadcaster == id)
}

impl Client {
    /// 새 설치 ID로 접속한다. 방송할 수 있어야 한다.
    pub async fn connect(t: &Target, name: &str) -> Self {
        let c = Self::open(t, name, InstallId::generate(), None).await;
        assert!(c.welcome.can_publish, "{name}: 방송 권한이 없습니다");
        c
    }

    /// 접속하고 Hello·Welcome을 주고받는다(권한은 확인하지 않는다).
    pub async fn open(t: &Target, name: &str, id: InstallId, token: Option<&str>) -> Self {
        let conn = signal::connect(&t.addr, Some(t.fp))
            .await
            .expect("접속 실패");
        Self::handshake(conn, name, id, token).await
    }

    /// 서버가 다시 뜰 때까지 `within` 동안 접속을 되풀이한다.
    pub async fn open_retry(
        t: &Target,
        name: &str,
        id: InstallId,
        within: Duration,
    ) -> (Self, Duration) {
        let started = Instant::now();
        loop {
            match signal::connect(&t.addr, Some(t.fp)).await {
                Ok(conn) => {
                    return (
                        Self::handshake(conn, name, id, None).await,
                        started.elapsed(),
                    );
                }
                Err(e) => {
                    assert!(
                        started.elapsed() < within,
                        "서버에 {within:?} 안에 다시 접속하지 못했습니다: {e}"
                    );
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
        }
    }

    async fn handshake(conn: Connection, name: &str, id: InstallId, token: Option<&str>) -> Self {
        let mut c = Self {
            id,
            name: name.into(),
            welcome: Welcome {
                protocol: 0,
                server_name: String::new(),
                can_publish: false,
                host_only: false,
            },
            list: Vec::new(),
            req: 0,
            conn,
        };
        let ip = signal::connected_ip(&c.conn);
        c.send(hello(id, name, PROTOCOL_VERSION, token, ip));
        match c.recv().await {
            ServerMsg::Welcome {
                protocol,
                server_name,
                can_publish,
                host_only,
                broadcasts,
                ..
            } => {
                c.welcome = Welcome {
                    protocol,
                    server_name,
                    can_publish,
                    host_only,
                };
                c.list = broadcasts;
            }
            other => panic!("Welcome이 아닙니다: {other:?}"),
        }
        c
    }

    pub fn send(&self, msg: ClientMsg) {
        self.conn
            .sender
            .send(msg)
            .expect("메시지를 보내지 못했습니다");
    }

    pub fn server_ip(&self) -> IpAddr {
        signal::connected_ip(&self.conn)
    }

    pub fn next_req(&mut self) -> u32 {
        self.req += 1;
        self.req
    }

    /// 메시지 하나를 기다린다. Broadcasts는 `list`에 반영한다.
    pub async fn poll_msg(&mut self, within: Duration) -> Next {
        match tokio::time::timeout(within, self.conn.receiver.recv()).await {
            Err(_) => Next::Timeout,
            Ok(None) => Next::Closed,
            Ok(Some(msg)) => {
                if let ServerMsg::Broadcasts { broadcasts } = &msg {
                    self.list = broadcasts.clone();
                }
                Next::Msg(msg)
            }
        }
    }

    pub async fn recv(&mut self) -> ServerMsg {
        match self.poll_msg(Duration::from_secs(10)).await {
            Next::Msg(m) => m,
            Next::Closed => panic!("연결이 닫혔습니다"),
            Next::Timeout => panic!("서버 메시지 시간 초과"),
        }
    }

    /// 원하는 메시지가 올 때까지 다른 메시지를 건너뛴다. 제한 시간 안에 안 오면 `Err`.
    pub async fn try_wait_for<T>(
        &mut self,
        within: Duration,
        mut f: impl FnMut(&ServerMsg) -> Option<T>,
    ) -> Result<T, &'static str> {
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.poll_msg(left.max(Duration::from_millis(1))).await {
                Next::Msg(m) => {
                    if let Some(v) = f(&m) {
                        return Ok(v);
                    }
                }
                Next::Closed => return Err("연결이 닫혔습니다"),
                Next::Timeout => return Err("시간 초과"),
            }
        }
    }

    pub async fn wait_for<T>(&mut self, what: &str, f: impl FnMut(&ServerMsg) -> Option<T>) -> T {
        self.try_wait_for(WAIT, f)
            .await
            .unwrap_or_else(|e| panic!("{}: {what}: {e}", self.name))
    }

    /// 방송 목록이 조건을 만족할 때까지 기다린다(이미 만족하면 바로 돌아온다).
    pub async fn wait_list(&mut self, what: &str, mut pred: impl FnMut(&[BroadcastInfo]) -> bool) {
        if pred(&self.list) {
            return;
        }
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.poll_msg(left.max(Duration::from_millis(1))).await {
                Next::Msg(ServerMsg::Broadcasts { broadcasts }) => {
                    if pred(&broadcasts) {
                        return;
                    }
                }
                Next::Msg(_) => {}
                Next::Closed => panic!("{}: {what}: 연결이 닫혔습니다", self.name),
                Next::Timeout => panic!(
                    "{}: 방송 목록이 바뀌지 않았습니다: {what} (마지막 목록: {:?})",
                    self.name, self.list
                ),
            }
        }
    }

    /// 연결이 닫힐 때까지 받는다. 그동안 받은 메시지를 돌려준다.
    pub async fn expect_closed(&mut self, within: Duration) -> Vec<ServerMsg> {
        let deadline = Instant::now() + within;
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.poll_msg(left.max(Duration::from_millis(1))).await {
                Next::Msg(m) => seen.push(m),
                Next::Closed => return seen,
                Next::Timeout => panic!("{}: 연결이 {within:?} 안에 닫히지 않았습니다", self.name),
            }
        }
    }

    /// `req`에 대한 성공 응답(`f`) 또는 오류 코드를 기다린다.
    async fn answer<T>(
        &mut self,
        req: u32,
        mut f: impl FnMut(&ServerMsg) -> Option<T>,
    ) -> Result<T, ErrorCode> {
        self.wait_for("요청 응답", |m| match m {
            ServerMsg::Error {
                req: Some(r), code, ..
            } if *r == req => Some(Err(*code)),
            other => f(other).map(Ok),
        })
        .await
    }

    /// 방송을 시작한다. 송출 연결이 될 때까지 기다린다.
    pub async fn publish(
        &mut self,
        info: PublishInfo,
        password: Option<&str>,
    ) -> Result<Publisher, ErrorCode> {
        let pending = PendingPublisher::new(self.server_ip(), true).unwrap();
        let req = self.next_req();
        self.send(ClientMsg::Publish {
            req,
            offer: pending.offer.clone(),
            info,
            password: password.map(Into::into),
        });
        let answer = self
            .answer(req, |m| match m {
                ServerMsg::Published { req: r, answer } if *r == req => Some(answer.clone()),
                _ => None,
            })
            .await?;
        let publisher = pending.start(&answer).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !publisher.stats().connected {
            assert!(
                Instant::now() < deadline,
                "{}: 송출 연결 실패: {:?}",
                self.name,
                publisher.stats()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // 첫 연결 시 키프레임 요청 표시가 켜진다. 비워 둔다.
        publisher.take_keyframe_request();
        Ok(publisher)
    }

    /// 방송을 보기 시작한다(미디어 연결은 기다리지 않는다).
    pub async fn watch(
        &mut self,
        broadcaster: InstallId,
        password: Option<&str>,
    ) -> Result<Watching, ErrorCode> {
        let pending = PendingViewer::new(self.server_ip());
        let req = self.next_req();
        self.send(ClientMsg::Watch {
            req,
            broadcaster,
            password: password.map(Into::into),
            offer: pending.offer.clone(),
        });
        let (session, answer) = self
            .answer(req, |m| match m {
                ServerMsg::Watching {
                    req: r,
                    session,
                    answer,
                } if *r == req => Some((*session, answer.clone())),
                _ => None,
            })
            .await?;
        Ok(Watching {
            session,
            viewer: pending.start(&answer),
        })
    }

    /// 보기 시작하고 미디어 연결이 될 때까지 기다린다. 실패하면 패닉.
    pub async fn watch_connected(
        &mut self,
        broadcaster: InstallId,
        password: Option<&str>,
    ) -> Watching {
        let w = self
            .watch(broadcaster, password)
            .await
            .unwrap_or_else(|code| panic!("{}: 시청 실패: {code:?}", self.name));
        w.viewer.wait_connected(Duration::from_secs(10)).await;
        w
    }

    /// 보내는 쪽 오류 코드만 확인하는 요청: 방송 시작이 거절돼야 하는 경우.
    pub async fn publish_err(&mut self, info: PublishInfo, password: Option<&str>) -> ErrorCode {
        match self.publish(info, password).await {
            Ok(_) => panic!("{}: 방송이 거절되지 않았습니다", self.name),
            Err(code) => code,
        }
    }

    /// 시그널링 연결을 닫는다(서버가 방송을 정리한다).
    pub fn close(self) {
        self.conn.sender.close();
    }
}
