//! base64url(패딩 없음) 인코딩. 디코딩할 때는 패딩이 있어도 받아 준다.

use base64::Engine;
use base64::alphabet::URL_SAFE;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

const ENGINE: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

pub(crate) fn encode(bytes: impl AsRef<[u8]>) -> String {
    ENGINE.encode(bytes)
}

pub(crate) fn decode(text: &str) -> Option<Vec<u8>> {
    ENGINE.decode(text).ok()
}

/// 정확히 `N`바이트로 디코딩되는 경우만 받아 준다.
pub(crate) fn decode_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    decode(text)?.try_into().ok()
}
