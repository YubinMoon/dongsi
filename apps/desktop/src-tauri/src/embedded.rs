//! 앱에 내장된 서버(로컬 랜·PC 직접 호스팅)를 Tauri 없이 켜고 끄는 로직.
//!
//! 방송 명령(`broadcast.rs`)과 통합 시험이 같은 함수를 쓴다.
//! - [`start_embedded`]: 인증서를 읽거나 만들고, 호스트 토큰을 정하고, 서버를 열고, Hub에 `host` 대상을 등록한다.
//!   `direct`면 [`PortMapper`]로 공유기 포트도 연다(실제 구현은 UPnP, 시험은 가짜).
//! - [`Embedded::shutdown`]: 갱신 작업을 멈추고, 서버를 닫고, 포트 매핑을 지운다.
//! - 초대 코드에 넣을 주소: [`local_addrs`], [`embedded_invite`].

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use dongsi_core::{Fingerprint, Host, InstallId, Invite, ServerAddr};
use dongsi_net::ifaces::{IfaceKind, Ipv4Iface};
use dongsi_net::tls::Identity;
use dongsi_net::upnp::{self, UpnpMapping};
use dongsi_server::{RunningServer, ServerConfig};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::error::{CmdError, CmdResult};
use crate::hub::{HOST_KEY, Hub, REASON_BROADCAST_ENDED, Target};

/// 내장 서버: 방송 하나, 시청자 최대 10명(D-007).
pub const EMBEDDED_MAX_VIEWERS: usize = 10;
/// 초대 코드에 넣는 주소 후보의 최대 개수.
pub const MAX_INVITE_ADDRS: usize = 6;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostingKind {
    Server,
    Lan,
    Direct,
}

/// 열어 둔 포트 매핑(공유기).
pub trait PortMapping: Send + Sync {
    /// 인터넷에서 보이는 외부 주소와 포트.
    fn external(&self) -> SocketAddr;
    /// 임대를 갱신한다(임대 시간의 절반마다 부른다).
    fn renew(&self) -> BoxFuture<'_, Result<(), String>>;
    /// 매핑을 지운다.
    fn remove(&self) -> BoxFuture<'_, ()>;
}

/// 공유기에 포트를 여는 방법. 앱은 UPnP([`UpnpMapper`])를, 시험은 가짜를 쓴다.
pub trait PortMapper: Send + Sync {
    /// `lan_ip:port`로 TCP·UDP 포트를 연다. 실패하면 사용자에게 보여 줄 이유를 돌려준다.
    fn map_ports(
        &self,
        lan_ip: Ipv4Addr,
        port: u16,
    ) -> BoxFuture<'_, Result<Arc<dyn PortMapping>, String>>;
}

/// 실제 UPnP 구현.
pub struct UpnpMapper;

impl PortMapper for UpnpMapper {
    fn map_ports(
        &self,
        lan_ip: Ipv4Addr,
        port: u16,
    ) -> BoxFuture<'_, Result<Arc<dyn PortMapping>, String>> {
        Box::pin(async move {
            let mapping = upnp::map_ports(lan_ip, port)
                .await
                .map_err(|e| e.to_string())?;
            Ok(Arc::new(UpnpPortMapping(mapping)) as Arc<dyn PortMapping>)
        })
    }
}

struct UpnpPortMapping(UpnpMapping);

impl PortMapping for UpnpPortMapping {
    fn external(&self) -> SocketAddr {
        SocketAddr::new(self.0.external_ip, self.0.external_port)
    }

    fn renew(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move { self.0.renew().await.map_err(|e| e.to_string()) })
    }

    fn remove(&self) -> BoxFuture<'_, ()> {
        Box::pin(self.0.remove())
    }
}

/// [`start_embedded`]의 입력.
pub struct EmbeddedParams<'a> {
    /// 인증서 폴더(앱 데이터의 `host/`). 없으면 만들고, 인증서가 없으면 새로 만든다.
    pub dir: &'a Path,
    /// 방송 목록·감지에 보이는 서버 이름.
    pub name: &'a str,
    pub hosting: HostingKind,
    /// 열 포트(`DONGSI_PORT` 또는 7884). 쓰이고 있으면 빈 포트로 대신한다.
    pub port: u16,
    /// 이 PC의 LAN 주소(UPnP가 가리킬 곳). `None`이면 `direct`가 UPnP를 시도하지 못한다.
    pub lan_ip: Option<Ipv4Addr>,
}

/// 켜 둔 내장 서버와 그 부속(UPnP 매핑·갱신 작업).
pub struct Embedded {
    server: RunningServer,
    /// 열려고 한 포트(`EmbeddedParams::port`).
    requested_port: u16,
    hosting: HostingKind,
    host_token: String,
    mapping: Option<Arc<dyn PortMapping>>,
    renew: Option<tokio::task::JoinHandle<()>>,
    upnp_error: Option<String>,
}

/// 화면에 보내는 UPnP 상태.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpnpView {
    pub external: Option<String>,
    /// 외부 IP가 사설 대역(이중 NAT·CGNAT)이라 인터넷에서 접속하지 못할 수 있다.
    pub double_nat: bool,
    pub error: Option<String>,
}

impl Embedded {
    /// 실제로 열린 포트(요청한 포트가 쓰이고 있으면 다른 번호).
    pub fn port(&self) -> u16 {
        self.server.port()
    }

    /// 열려고 한 포트(`DONGSI_PORT` 또는 7884).
    pub fn requested_port(&self) -> u16 {
        self.requested_port
    }

    /// 요청한 포트의 TCP나 UDP가 쓰이고 있어 다른 포트로 열었으면 true. 요청이 0(아무 포트)이면 항상 false.
    pub fn port_fallback(&self) -> bool {
        self.requested_port != 0 && self.port() != self.requested_port
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.server.fingerprint()
    }

    pub fn server(&self) -> &RunningServer {
        &self.server
    }

    /// 이 서버에 방송할 수 있는 호스트 토큰(`host` 대상에 들어 있다).
    pub fn host_token(&self) -> &str {
        &self.host_token
    }

    pub fn hosting(&self) -> HostingKind {
        self.hosting
    }

    pub fn mapping(&self) -> Option<&Arc<dyn PortMapping>> {
        self.mapping.as_ref()
    }

    pub fn upnp_error(&self) -> Option<&str> {
        self.upnp_error.as_deref()
    }

    /// `direct`일 때만 있다.
    pub fn upnp_view(&self) -> Option<UpnpView> {
        if self.hosting != HostingKind::Direct {
            return None;
        }
        Some(UpnpView {
            external: self.mapping.as_ref().map(|m| m.external().to_string()),
            double_nat: self
                .mapping
                .as_ref()
                .is_some_and(|m| !upnp::is_public_ip(m.external().ip())),
            error: self.upnp_error.clone(),
        })
    }

    /// 갱신 작업을 멈추고, 서버를 닫고(접속자에게 닫힘을 알린다), 포트 매핑을 지운다.
    pub async fn shutdown(self) {
        if let Some(task) = self.renew {
            task.abort();
        }
        self.server.shutdown().await;
        if let Some(mapping) = self.mapping {
            mapping.remove().await;
        }
    }

    /// Hub의 `host` 연결을 닫고(`방송을 끝냈습니다`) 대상을 지운 뒤 [`Embedded::shutdown`]한다.
    pub async fn teardown(self, hub: &Hub) {
        hub.disconnect(HOST_KEY, REASON_BROADCAST_ENDED);
        hub.remove_target(HOST_KEY);
        self.shutdown().await;
    }
}

/// 내장 서버를 열고 Hub에 `host` 대상(루프백 주소, 서버 지문, 호스트 토큰)을 등록한다.
/// `direct`면 공유기 포트도 연다. 포트 매핑이 실패해도 서버는 LAN으로 쓸 수 있게 열어 둔다
/// (실패 이유는 [`Embedded::upnp_error`]).
pub async fn start_embedded(
    hub: &Hub,
    params: EmbeddedParams<'_>,
    mapper: &dyn PortMapper,
) -> CmdResult<Embedded> {
    std::fs::create_dir_all(params.dir).map_err(|e| CmdError::new(e.to_string()))?;
    let identity =
        Identity::load_or_generate(params.dir).map_err(|e| CmdError::new(e.to_string()))?;
    let token = InstallId::generate().to_string();
    let server = dongsi_server::start(
        ServerConfig {
            name: params.name.to_owned(),
            port: params.port,
            fallback_port: true,
            host_token: Some(token.clone()),
            max_broadcasts: 1,
            max_viewers_per_broadcast: EMBEDDED_MAX_VIEWERS,
            discovery: true,
            ..Default::default()
        },
        &identity,
    )
    .await
    .map_err(|e| CmdError::new(format!("내장 서버를 열지 못했습니다: {e}")))?;
    let port = server.port();
    hub.set_target(
        HOST_KEY,
        Target {
            addrs: vec![ServerAddr::new(
                Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                port,
            )],
            fingerprint: Some(server.fingerprint()),
            saved: None,
            host_token: Some(token.clone()),
        },
    );

    let mut embedded = Embedded {
        server,
        requested_port: params.port,
        hosting: params.hosting,
        host_token: token,
        mapping: None,
        renew: None,
        upnp_error: None,
    };
    if params.hosting == HostingKind::Direct {
        match params.lan_ip {
            None => embedded.upnp_error = Some("LAN 주소를 찾지 못했습니다".into()),
            Some(lan) => match mapper.map_ports(lan, port).await {
                Ok(mapping) => {
                    let external_port = mapping.external().port();
                    if external_port != port {
                        embedded.server.set_external_port(Some(external_port));
                    }
                    let renew_mapping = mapping.clone();
                    embedded.renew = Some(tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(UpnpMapping::renew_interval()).await;
                            if let Err(e) = renew_mapping.renew().await {
                                warn!("{e}");
                            }
                        }
                    }));
                    embedded.mapping = Some(mapping);
                }
                Err(e) => {
                    warn!("UPnP 실패: {e}");
                    embedded.upnp_error = Some(format!(
                        "{e}. 공유기에서 TCP·UDP {port} 포트를 이 PC({lan})로 직접 열어 주세요."
                    ));
                }
            },
        }
    }
    Ok(embedded)
}

/// 이 PC의 LAN·Tailscale 주소(초대 코드용). LAN이 앞, Tailscale이 뒤.
pub fn local_addrs(ifaces: &[Ipv4Iface], port: u16) -> Vec<ServerAddr> {
    let mut ifaces = ifaces.to_vec();
    ifaces.sort_by_key(|i| match i.kind {
        IfaceKind::Lan => 0,
        IfaceKind::Tailscale => 1,
        _ => 2,
    });
    ifaces
        .into_iter()
        .filter(|i| matches!(i.kind, IfaceKind::Lan | IfaceKind::Tailscale))
        .map(|i| ServerAddr::new(Host::Ip(IpAddr::V4(i.ip)), port))
        .collect()
}

/// 내장 서버의 초대 주소 후보: 포트가 열렸으면 외부 주소가 맨 앞, 그 뒤에 이 PC의 LAN·Tailscale 주소.
pub fn embedded_addrs(embedded: &Embedded, ifaces: &[Ipv4Iface]) -> Vec<ServerAddr> {
    let mut addrs = Vec::new();
    if let Some(m) = embedded.mapping() {
        let ext = m.external();
        addrs.push(ServerAddr::new(Host::Ip(ext.ip()), ext.port()));
    }
    addrs.extend(local_addrs(ifaces, embedded.port()));
    addrs
}

/// 내장 서버의 방송 초대 코드(비밀번호 있음)와 비밀번호를 뺀 코드. 주소 후보는 [`embedded_addrs`],
/// 이름은 서버 이름이다.
pub fn embedded_broadcast_invite(
    embedded: &Embedded,
    ifaces: &[Ipv4Iface],
    server_name: &str,
    broadcaster: InstallId,
    password: Option<&str>,
) -> CmdResult<(String, Option<String>)> {
    broadcast_invite(
        embedded_addrs(embedded, ifaces),
        embedded.port(),
        embedded.fingerprint(),
        Some(server_name.to_owned()),
        broadcaster,
        password,
    )
}

/// 방송 초대 코드와 비밀번호를 뺀 코드를 만든다. 주소가 없으면 `fallback_port`의 루프백을 넣는다.
pub fn broadcast_invite(
    mut addrs: Vec<ServerAddr>,
    fallback_port: u16,
    fingerprint: Fingerprint,
    name: Option<String>,
    broadcaster: InstallId,
    password: Option<&str>,
) -> CmdResult<(String, Option<String>)> {
    if addrs.is_empty() {
        addrs.push(ServerAddr::new(
            Host::Ip(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            fallback_port,
        ));
    }
    addrs.truncate(MAX_INVITE_ADDRS);
    let invite = Invite::for_broadcast(
        addrs,
        fingerprint,
        name,
        broadcaster,
        password.map(str::to_owned),
    )
    .map_err(|e| CmdError::new(format!("초대 코드를 만들지 못했습니다: {e}")))?;
    let without = password.map(|_| invite.without_password().encode());
    Ok((invite.encode(), without))
}
