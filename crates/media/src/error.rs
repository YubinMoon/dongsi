//! 미디어 오류 타입. 메시지는 사용자에게 그대로 보여 줄 수 있게 한국어로 쓴다.

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("이 PC에서는 화면 캡처(Windows.Graphics.Capture)를 쓸 수 없습니다")]
    CaptureUnsupported,

    #[error("캡처할 대상을 찾을 수 없습니다: {0}")]
    SourceNotFound(String),

    #[error("잘못된 캡처 대상 ID입니다: {0}")]
    InvalidSourceId(String),

    #[error("화면 캡처를 시작하지 못했습니다: {0}")]
    Capture(String),

    #[error("캡처 대상이 닫혔습니다")]
    SourceClosed,

    #[error("그래픽 장치(Direct3D 11) 오류: {0}")]
    Graphics(String),

    /// GPU가 초기화되거나 사라졌다(드라이버 재설정·오류, `DXGI_ERROR_DEVICE_REMOVED`·`DEVICE_RESET`·`DEVICE_HUNG` 등).
    /// 장치·캡처·변환·인코더 세션이 모두 그 장치에 묶여 있어 방송은 멈추고, 방송을 다시 시작해야 한다.
    #[error("그래픽 장치가 초기화되어 방송을 멈췄습니다({0}). 방송을 다시 시작해 주세요")]
    DeviceLost(String),

    /// 이 PC에서 열리는 하드웨어 H.264 인코더(nvenc·amf·qsv)가 없다(D-103). 소프트웨어 인코더는 없으므로 방송할 수 없다.
    #[error("하드웨어 H.264 인코더(NVIDIA·AMD·Intel)를 찾지 못해 방송할 수 없습니다")]
    NoEncoder,

    /// GPU 변환(D3D11 Video Processor, BGRA → NV12)을 쓸 수 없다(D-105). CPU 변환은 없으므로 방송할 수 없다.
    #[error("GPU 영상 변환(Direct3D 11 Video Processor)을 쓸 수 없어 방송할 수 없습니다: {0}")]
    NoGpuConvert(String),

    /// 방송 중 GPU 변환이 실패했다(D-105). 다른 변환 경로가 없어 장치 손실처럼 방송을 멈춘다.
    #[error("GPU 영상 변환(Direct3D 11 Video Processor)이 실패해 방송을 멈췄습니다: {0}")]
    GpuConvert(String),

    #[error("인코더 오류({encoder}): {message}")]
    Encoder { encoder: String, message: String },

    #[error("오디오 캡처 오류: {0}")]
    Audio(String),

    #[error("오디오 인코더(Opus) 오류: {0}")]
    AudioEncoder(String),

    #[error("창 캡처 제외 설정에 실패했습니다: {0}")]
    DisplayAffinity(String),

    #[error("내부 스레드를 시작하지 못했습니다: {0}")]
    Thread(String),
}

impl MediaError {
    pub(crate) fn graphics(context: &str, e: impl std::fmt::Display) -> Self {
        MediaError::Graphics(format!("{context}: {e}"))
    }

    pub(crate) fn capture(context: &str, e: impl std::fmt::Display) -> Self {
        MediaError::Capture(format!("{context}: {e}"))
    }
}
