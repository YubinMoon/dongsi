//! 서버 TLS 신원(자체 서명 인증서)과, 지문으로 서버를 확인하는 클라이언트 설정.
//!
//! - 서버는 처음 실행할 때 자체 서명 인증서를 만들어 보관한다. 인증서가 바뀌지 않아야
//!   지문과 초대 코드가 유지된다.
//! - 클라이언트는 CA 체인을 보지 않고, 인증서 DER의 SHA-256 지문만 비교한다.
//!   기대하는 지문이 없으면(처음 접속) 받은 지문을 기록해서 돌려준다(TOFU).

use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use dongsi_core::Fingerprint;
use dongsi_core::proto::TLS_SERVER_NAME;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

use crate::NetError;

const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";

/// 서버의 TLS 신원.
pub struct Identity {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    fingerprint: Fingerprint,
}

impl Identity {
    /// 새 자체 서명 인증서를 만든다.
    pub fn generate() -> Result<Self, NetError> {
        let certified = rcgen::generate_simple_self_signed(vec![TLS_SERVER_NAME.to_string()])
            .map_err(|e| NetError::Tls(format!("인증서를 만들지 못했습니다: {e}")))?;
        let cert = certified.cert.der().clone();
        let key = PrivateKeyDer::try_from(certified.signing_key.serialize_der())
            .map_err(|e| NetError::Tls(format!("개인 키 형식 오류: {e}")))?;
        Ok(Self::from_parts(cert, key))
    }

    fn from_parts(cert: CertificateDer<'static>, key: PrivateKeyDer<'static>) -> Self {
        let fingerprint = fingerprint_of(&cert);
        Self {
            cert,
            key,
            fingerprint,
        }
    }

    /// `dir`의 `cert.pem`·`key.pem`을 읽는다. 없으면 새로 만들어 저장한다.
    pub fn load_or_generate(dir: &Path) -> Result<Self, NetError> {
        let cert_path = dir.join(CERT_FILE);
        let key_path = dir.join(KEY_FILE);
        if cert_path.exists() && key_path.exists() {
            let cert = CertificateDer::from_pem_file(&cert_path).map_err(|e| {
                NetError::Tls(format!("{}을 읽지 못했습니다: {e}", cert_path.display()))
            })?;
            let key = PrivateKeyDer::from_pem_file(&key_path).map_err(|e| {
                NetError::Tls(format!("{}을 읽지 못했습니다: {e}", key_path.display()))
            })?;
            return Ok(Self::from_parts(cert, key));
        }

        fs::create_dir_all(dir)?;
        let certified = rcgen::generate_simple_self_signed(vec![TLS_SERVER_NAME.to_string()])
            .map_err(|e| NetError::Tls(format!("인증서를 만들지 못했습니다: {e}")))?;
        fs::write(&cert_path, certified.cert.pem())?;
        write_private(&key_path, certified.signing_key.serialize_pem().as_bytes())?;
        let cert = certified.cert.der().clone();
        let key = PrivateKeyDer::try_from(certified.signing_key.serialize_der())
            .map_err(|e| NetError::Tls(format!("개인 키 형식 오류: {e}")))?;
        Ok(Self::from_parts(cert, key))
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    /// TLS 서버 설정.
    pub fn server_config(&self) -> Result<Arc<rustls::ServerConfig>, NetError> {
        let config = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(|e| NetError::Tls(e.to_string()))?
            .with_no_client_auth()
            .with_single_cert(vec![self.cert.clone()], self.key.clone_key())
            .map_err(|e| NetError::Tls(format!("인증서를 설정하지 못했습니다: {e}")))?;
        Ok(Arc::new(config))
    }
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    // Windows에서는 사용자 프로필 아래 폴더의 기본 ACL로 충분하다고 보고 일반 쓰기를 한다.
    fs::write(path, data)
}

/// 인증서 DER의 SHA-256 지문.
pub fn fingerprint_of(cert_der: &[u8]) -> Fingerprint {
    let digest = Sha256::digest(cert_der);
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&digest);
    Fingerprint::from_bytes(bytes)
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// 접속하면서 실제로 받은 서버 지문.
#[derive(Debug, Default)]
pub struct ObservedFingerprint(Mutex<Option<Fingerprint>>);

impl ObservedFingerprint {
    pub fn get(&self) -> Option<Fingerprint> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, fp: Fingerprint) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(fp);
    }
}

/// 지문으로 서버를 확인하는 TLS 클라이언트 설정.
///
/// `expected`가 `Some`이면 지문이 다를 때 핸드셰이크가 실패한다. `None`이면 무엇이든 받고,
/// 받은 지문을 `ObservedFingerprint`에 남긴다.
pub fn client_config(
    expected: Option<Fingerprint>,
) -> Result<(Arc<rustls::ClientConfig>, Arc<ObservedFingerprint>), NetError> {
    let provider = provider();
    let observed = Arc::new(ObservedFingerprint::default());
    let verifier = Arc::new(PinnedVerifier {
        expected,
        observed: observed.clone(),
        algorithms: provider.signature_verification_algorithms,
    });
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| NetError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok((Arc::new(config), observed))
}

pub(crate) fn server_name() -> ServerName<'static> {
    ServerName::try_from(TLS_SERVER_NAME).expect("고정된 이름은 유효하다")
}

#[derive(Debug)]
struct PinnedVerifier {
    expected: Option<Fingerprint>,
    observed: Arc<ObservedFingerprint>,
    algorithms: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint_of(end_entity);
        self.observed.set(fp);
        match self.expected {
            Some(expected) if expected != fp => Err(rustls::Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            )),
            _ => Ok(ServerCertVerified::assertion()),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_persists_fingerprint() {
        let dir = std::env::temp_dir().join(format!("dongsi-tls-{}", rand::random::<u64>()));
        let a = Identity::load_or_generate(&dir).unwrap();
        let b = Identity::load_or_generate(&dir).unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
        let c = Identity::generate().unwrap();
        assert_ne!(a.fingerprint(), c.fingerprint());
        a.server_config().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
