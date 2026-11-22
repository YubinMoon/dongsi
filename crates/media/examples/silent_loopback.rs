//! 소리 없이 실제 출력을 잴 수 있는지 확인하는 예제: 세션 볼륨 0 + 프로세스 루프백.
//!
//! # 목적
//!
//! 시청 앱의 A/V를 "실제 출력"(WASAPI 렌더) 기준으로 재고 싶은데, 하네스는 소리를 내면 안 된다.
//! --mute-audio 상태에서는 프로세스 루프백(INCLUDE)이 샘플은 주지만 값이 전부 0이었다.
//! 이 예제는 **오디오 세션 볼륨(ISimpleAudioVolume)을 0으로(또는 음소거로) 둔 프로세스의 소리를
//! 프로세스 루프백이 받는지** 확인한다. 곧, 루프백이 세션 볼륨 앞에서 뜨는지 뒤에서 뜨는지를 본다.
//! 앞에서 뜬다면 시청 앱의 세션만 0으로 두고 들리지 않게 실제 출력 시각을 잴 수 있다.
//!
//! # 구조
//!
//! 부모(캡처)와 자식(재생) 두 프로세스다. 자식은 같은 예제를 --render <모드>로 다시 실행한 것이다.
//!
//! - 자식: 기본 렌더 장치에서 IAudioClient를 공유 모드로 초기화하고(48kHz float 스테레오, 세션 GUID는
//!   null = 이 프로세스의 기본 세션) 세션 볼륨을 0으로 맞춘다(vol0mute 모드는 음소거도). 값이 정말 0인지
//!   읽어서 확인한 뒤에야 READY를 내보낸다. 이어서 부모가 GO를 보내면 1kHz 사인(진폭 0.3)을 1.5초 재생한다.
//! - 부모: 모드 vol0mute, vol0 차례로 자식을 띄우고, READY를 받으면 자식 pid로 프로세스 루프백(INCLUDE)을
//!   시작한 뒤 GO를 보내고, DONE 뒤 루프백이 받은 샘플 수·최댓값·삐 수를 모아 JSON 한 줄로 낸다.
//!
//! # 안전 장치 (소리가 들리면 안 된다)
//!
//! - **확인 전에는 절대 Start하지 않는다.** 볼륨이 정확히 0.0이 아니거나 음소거 상태가 기대와 다르면
//!   FAIL을 내고 끝낸다. 재생 직전(버퍼를 채운 뒤 Start 바로 앞)에 한 번 더 확인하고, 재생 중에도 10ms마다
//!   확인해 값이 달라지면 즉시 Stop하고 FAIL로 끝낸다.
//! - 자식은 부모가 보낸 GO 한 줄이 있을 때만 재생한다. 표준 입력이 닫히거나 다른 글자가 오면 재생 없이 끝낸다.
//! - 장치 볼륨(IAudioEndpointVolume)·장치 음소거는 건드리지 않는다. 세션 볼륨은 이 자식 프로세스의
//!   기본 세션 것만 바꾼다. 다른 프로세스의 세션은 읽기만 하고(진단에서 이 pid의 세션만 거른다) 바꾸지 않는다.
//! - AUDCLNT_STREAMFLAGS_NOPERSIST로 열어 세션 볼륨·음소거가 저장되지 않는다. 이 예제가 어떤 값을 남기지 않는다.
//! - 알 수 없는 모드는 거부한다(볼륨 0이 아닌 모드는 아예 없다).
//! - 자식이 10초 안에 응답하지 않으면 부모가 자식을 끝낸다. 부모가 죽어도 자식의 표준 입력이 닫혀 재생하지 않는다.
//! - --dry-run은 자식이 READY까지만 하고 Start하지 않는다(확인 경로만 시험).
//!
//! # 실행
//!
//! 저장소 루트에서 PATH 앞에 ffmpeg bin을 준 PowerShell 예(ffmpeg·libclang은 `third_party/` 정션으로 찾는다, AGENTS.md "빌드 환경"):
//!
//! ~~~text
//! $env:PATH = (Resolve-Path third_party\ffmpeg\bin).Path + ';' + $env:PATH
//! cargo run -p dongsi-media --example silent_loopback -- --dry-run   # 확인 경로만(Start 안 함)
//! cargo run -p dongsi-media --example silent_loopback                # 본 시험
//! ~~~
//!
//! # 출력 읽는 법
//!
//! 모드마다 JSON 한 줄이 나온다.
//!
//! ~~~text
//! {"mode":"vol0mute","verified":true,"volume":0.0,"mute":true,"secondClientVolume":0.0,
//!  "sessions":[{"volume":0.0,"mute":true,"state":1}],"renderedFrames":72000,
//!  "loopbackSamples":..., "loopbackPeak":..., "beeps":..., "error":null}
//! ~~~
//!
//! - verified: 볼륨 0·음소거 기대값을 읽어서 확인했는가(false면 재생하지 않았다).
//! - volume, mute: 자식이 설정한 뒤 읽은 값. secondClientVolume: 같은 장치·같은 플래그로 연 두 번째
//!   클라이언트(Start 안 함)가 읽은 세션 볼륨. 0이면 같은 프로세스에 나중에 생기는 스트림도 같은 세션 볼륨을
//!   받는다(안전). sessions: 이 pid의 세션 목록과 각 볼륨·음소거(진단).
//! - renderedFrames: 렌더 버퍼에 써 넣은 프레임 수(1.5초면 약 72000). loopbackSamples·loopbackPeak·beeps:
//!   프로세스 루프백이 받은 샘플 수(채널당)·최대 절댓값·1kHz 삐 시작 수. error: 이 모드를 못 끝낸 까닭.
//!
//! 마지막 줄의 판정:
//!
//! - VERDICT: PRE_VOLUME — 어느 모드든 peak > 0.05. 루프백이 세션 볼륨 앞에서 뜬다(소리 없이 L을 잴 수 있다).
//!   어느 모드에서 그랬는지 함께 적는다. vol0도 포함되면 음소거와 무관하게 볼륨 0 앞에서 뜬다는 뜻이고,
//!   vol0mute만이면 음소거 앞에서 뜨지만 볼륨 0만으로는 아직 모른다.
//! - VERDICT: POST_VOLUME — 샘플은 오는데 peak < 0.001. 루프백이 세션 볼륨 뒤에서 뜬다(이 방법은 안 된다).
//! - VERDICT: NO_SAMPLES — 샘플이 하나도 안 왔다.
//! - VERDICT: INCONCLUSIVE — peak가 0.001~0.05 사이(애매함).
//! - VERDICT: ERROR — 끝낸 모드가 하나도 없다(오류 내용은 모드 줄의 error).
//! - VERDICT: DRY_RUN — --dry-run에서 두 모드 모두 확인까지 통과.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use dongsi_media::probe::{DEFAULT_THRESHOLD, ProbeTarget, ToneProbe};
use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_NOPERSIST,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, IAudioClient, IAudioRenderClient,
    IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceEnumerator,
    ISimpleAudioVolume, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender,
};
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::Interface;

type Res<T> = Result<T, String>;

/// 모드 이름과 음소거 기대값. 볼륨이 0이 아닌 모드는 없다.
const MODES: [(&str, bool); 2] = [("vol0mute", true), ("vol0", false)];
/// 부모가 자식의 응답을 기다리는 한도.
const CHILD_TIMEOUT: Duration = Duration::from_secs(10);
/// 렌더 버퍼 길이(100ns 단위): 100ms
const BUFFER_HNS: i64 = 1_000_000;
const RATE: u32 = 48_000;
/// 재생 시간
const PLAY: Duration = Duration::from_millis(1500);
const AMPLITUDE: f32 = 0.3;
/// 1kHz 사인의 주기(샘플): 48
const PERIOD: u64 = (RATE / 1000) as u64;

fn ctx<T>(r: windows::core::Result<T>, what: &str) -> Res<T> {
    r.map_err(|e| format!("{what}: {e}"))
}

// ---------------------------------------------------------------- JSON (손으로)

fn jstr(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn jf(v: f32) -> String {
    if v.is_finite() {
        format!("{v:?}")
    } else {
        "null".into()
    }
}

fn jopt_str(s: &Option<String>) -> String {
    s.as_deref().map(jstr).unwrap_or_else(|| "null".into())
}

/// 한 줄 JSON 객체에서 최상위 키의 값 원문을 꺼낸다(자식이 만든 단순한 JSON 전용).
/// 같은 이름이 중첩 안에도 있을 수 있으니 값이 중첩인 필드(sessions)는 항상 마지막에 쓴다.
fn json_raw<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\":");
    let rest = &s[s.find(&pat)? + pat.len()..];
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, &b) in rest.as_bytes().iter().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
                if depth == 0 {
                    return Some(&rest[..=i]);
                }
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                if depth == 0 {
                    return Some(&rest[..i]);
                }
                depth -= 1;
                if depth == 0 {
                    return Some(&rest[..=i]);
                }
            }
            b',' if depth == 0 => return Some(&rest[..i]),
            _ => {}
        }
    }
    Some(rest)
}

fn json_string(s: &str, key: &str) -> Option<String> {
    let raw = json_raw(s, key)?;
    if raw == "null" {
        return None;
    }
    let inner = raw.strip_prefix('"')?.strip_suffix('"')?;
    // 자식이 쓴 이스케이프(\" \\ \n \r \t)만 되돌린다.
    Some(
        inner
            .replace("\\n", "\n")
            .replace("\\r", "\r")
            .replace("\\t", "\t")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\"),
    )
}

fn json_u64(s: &str, key: &str) -> Option<u64> {
    json_raw(s, key)?.parse().ok()
}

// ---------------------------------------------------------------- 자식(재생)

/// 세션 볼륨 하나를 읽은 값.
#[derive(Clone, Copy)]
struct VolState {
    volume: f32,
    mute: bool,
}

fn read_vol(v: &ISimpleAudioVolume) -> Res<VolState> {
    // 읽기만 한다.
    let volume = ctx(unsafe { v.GetMasterVolume() }, "GetMasterVolume")?;
    let mute = ctx(unsafe { v.GetMute() }, "GetMute")?.as_bool();
    Ok(VolState { volume, mute })
}

/// 정확히 0.0이고 음소거가 기대값인가.
fn vol_ok(s: VolState, mute_expected: bool) -> bool {
    s.volume == 0.0 && s.mute == mute_expected
}

fn float_format() -> WAVEFORMATEXTENSIBLE {
    let mut f = WAVEFORMATEXTENSIBLE::default();
    f.Format.wFormatTag = WAVE_FORMAT_EXTENSIBLE as u16;
    f.Format.nChannels = 2;
    f.Format.nSamplesPerSec = RATE;
    f.Format.nAvgBytesPerSec = RATE * 8;
    f.Format.nBlockAlign = 8;
    f.Format.wBitsPerSample = 32;
    f.Format.cbSize = 22;
    f.Samples.wValidBitsPerSample = 32;
    f.dwChannelMask = 3; // FL | FR
    f.SubFormat = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    f
}

/// 렌더 클라이언트와 그 세션 볼륨.
struct Client {
    client: IAudioClient,
    vol: ISimpleAudioVolume,
}

/// 공유 모드 렌더 클라이언트를 연다(Start하지 않는다). 세션 GUID는 null(이 프로세스의 기본 세션),
/// NOPERSIST라 세션 볼륨·음소거가 저장되지 않는다.
fn open_client(dev: &IMMDevice, fmt: &WAVEFORMATEXTENSIBLE) -> Res<Client> {
    let flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
        | AUDCLNT_STREAMFLAGS_NOPERSIST;
    unsafe {
        let client: IAudioClient = ctx(dev.Activate(CLSCTX_ALL, None), "IAudioClient 활성화")?;
        ctx(
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags,
                BUFFER_HNS,
                0,
                fmt as *const WAVEFORMATEXTENSIBLE as *const WAVEFORMATEX,
                None,
            ),
            "IAudioClient::Initialize",
        )?;
        let vol: ISimpleAudioVolume = ctx(client.GetService(), "ISimpleAudioVolume 얻기")?;
        Ok(Client { client, vol })
    }
}

/// 이 pid의 세션들의 볼륨·음소거·상태(진단). 다른 pid의 세션은 보지도 않는다. 읽기만 한다.
fn own_sessions(dev: &IMMDevice, pid: u32) -> Res<Vec<String>> {
    unsafe {
        let mgr: IAudioSessionManager2 = ctx(
            dev.Activate(CLSCTX_ALL, None),
            "IAudioSessionManager2 활성화",
        )?;
        let en = ctx(mgr.GetSessionEnumerator(), "GetSessionEnumerator")?;
        let n = ctx(en.GetCount(), "세션 수")?;
        let mut out = Vec::new();
        for i in 0..n {
            let Ok(ctl) = en.GetSession(i) else { continue };
            let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() else {
                continue;
            };
            if ctl2.GetProcessId().ok() != Some(pid) {
                continue;
            }
            let state = ctl.GetState().map(|s| s.0).unwrap_or(-1);
            let Ok(sv) = ctl.cast::<ISimpleAudioVolume>() else {
                continue;
            };
            let Ok(s) = read_vol(&sv) else { continue };
            out.push(format!(
                "{{\"volume\":{},\"mute\":{},\"state\":{state}}}",
                jf(s.volume),
                s.mute
            ));
        }
        Ok(out)
    }
}

fn fail(json: String) -> i32 {
    println!("FAIL {json}");
    let _ = std::io::stdout().flush();
    1
}

fn child_main(mode: &str, dry_run: bool) -> i32 {
    let Some(&(_, mute)) = MODES.iter().find(|(m, _)| *m == mode) else {
        return fail(format!(
            "{{\"mode\":{},\"verified\":false,\"error\":{}}}",
            jstr(mode),
            jstr("알 수 없는 모드(vol0mute, vol0만 허용)")
        ));
    };
    match child_run(mode, mute, dry_run) {
        Ok(()) => 0,
        Err(json) => fail(json),
    }
}

/// 확인 실패 등의 JSON 본문을 만든다.
fn fail_json(mode: &str, verified: bool, st: Option<VolState>, error: &str) -> String {
    let (v, m) = match st {
        Some(s) => (jf(s.volume), s.mute.to_string()),
        None => ("null".into(), "null".into()),
    };
    format!(
        "{{\"mode\":{},\"verified\":{verified},\"volume\":{v},\"mute\":{m},\"error\":{}}}",
        jstr(mode),
        jstr(error)
    )
}

fn child_run(mode: &str, mute: bool, dry_run: bool) -> Result<(), String> {
    let e = |msg: String| fail_json(mode, false, None, &msg);
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|x| e(format!("CoInitializeEx: {x}")))?;
    let enumerator: IMMDeviceEnumerator = ctx(
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) },
        "장치 열거자",
    )
    .map_err(e)?;
    let dev = ctx(
        unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) },
        "기본 렌더 장치",
    )
    .map_err(e)?;
    let fmt = float_format();

    // 1) 첫 클라이언트: 초기화(Start 안 함) → 세션 볼륨 0(·음소거) → 읽어서 확인.
    //    NOPERSIST이므로 이 세션의 값은 저장되지 않는다. 장치 볼륨은 건드리지 않는다.
    let c1 = open_client(&dev, &fmt).map_err(e)?;
    unsafe {
        ctx(
            c1.vol.SetMasterVolume(0.0, std::ptr::null()),
            "SetMasterVolume(0)",
        )
        .map_err(e)?;
        ctx(c1.vol.SetMute(mute, std::ptr::null()), "SetMute").map_err(e)?;
    }
    let st1 = read_vol(&c1.vol).map_err(e)?;
    if !vol_ok(st1, mute) {
        return Err(fail_json(
            mode,
            false,
            Some(st1),
            "볼륨·음소거 확인 실패(Start하지 않음)",
        ));
    }

    // 2) 두 번째 클라이언트(같은 장치·플래그, Start 안 함): 같은 세션 볼륨을 받는지 읽는다.
    //    새 클라이언트를 열었다고 첫 클라이언트의 값이 바뀌지 않았는지도 다시 확인한다.
    let c2 = open_client(&dev, &fmt).map_err(e)?;
    let st2 = read_vol(&c2.vol).map_err(e)?;
    let st1b = read_vol(&c1.vol).map_err(e)?;
    if !vol_ok(st1b, mute) {
        return Err(fail_json(
            mode,
            false,
            Some(st1b),
            "두 번째 클라이언트를 연 뒤 볼륨·음소거가 달라짐(Start하지 않음)",
        ));
    }

    // 3) 진단: 이 pid의 세션들.
    let pid = std::process::id();
    let (sessions, sessions_err) = match own_sessions(&dev, pid) {
        Ok(v) => (v, None),
        Err(x) => (Vec::new(), Some(x)),
    };

    println!(
        "READY {{\"mode\":{},\"verified\":true,\"pid\":{pid},\"volume\":{},\"mute\":{},\
         \"secondClientVolume\":{},\"secondClientMute\":{},\"sessionsError\":{},\"sessions\":[{}]}}",
        jstr(mode),
        jf(st1b.volume),
        st1b.mute,
        jf(st2.volume),
        st2.mute,
        jopt_str(&sessions_err),
        sessions.join(",")
    );
    let _ = std::io::stdout().flush();
    if dry_run {
        return Ok(());
    }

    // 4) GO 한 줄을 기다린다. 닫힘(EOF)이나 다른 글자는 재생 없이 끝낸다.
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(n) if n > 0 && line.trim() == "GO" => {}
        _ => {
            return Err(fail_json(
                mode,
                true,
                Some(st1b),
                "GO를 받지 못해 재생하지 않음",
            ));
        }
    }
    play(mode, mute, &c1, &c2).map_err(|x| fail_json(mode, true, None, &x))
}

/// 렌더 버퍼를 사인으로 채우며 재생한다. 매 단계 볼륨을 확인하고 달라지면 즉시 멈춘다.
fn play(mode: &str, mute: bool, c1: &Client, c2: &Client) -> Res<()> {
    let guard = |what: &str| -> Res<()> {
        for c in [c1, c2] {
            let s = read_vol(&c.vol)?;
            if !vol_ok(s, mute) {
                return Err(format!(
                    "{what}: 볼륨·음소거가 달라짐(volume={}, mute={})",
                    jf(s.volume),
                    s.mute
                ));
            }
        }
        Ok(())
    };
    unsafe {
        let render: IAudioRenderClient = ctx(c1.client.GetService(), "IAudioRenderClient")?;
        let size = ctx(c1.client.GetBufferSize(), "GetBufferSize")?;
        let mut phase = 0u64;
        let mut rendered = 0u64;
        let mut fill = |n: u32| -> Res<()> {
            if n == 0 {
                return Ok(());
            }
            let p = ctx(render.GetBuffer(n), "GetBuffer")? as *mut f32;
            for i in 0..n as usize {
                let v = AMPLITUDE
                    * (2.0 * std::f32::consts::PI * (phase % PERIOD) as f32 / PERIOD as f32).sin();
                p.add(i * 2).write_unaligned(v);
                p.add(i * 2 + 1).write_unaligned(v);
                phase += 1;
            }
            ctx(render.ReleaseBuffer(n, 0), "ReleaseBuffer")?;
            rendered += n as u64;
            Ok(())
        };

        // Start 전에 버퍼를 채운다(Start 전이라 소리는 나지 않는다). 그리고 Start 바로 앞에서 마지막 확인.
        guard("재생 직전")?;
        fill(size)?;
        guard("Start 직전")?;
        ctx(c1.client.Start(), "Start")?;

        let begin = Instant::now();
        let mut failure: Option<String> = None;
        while begin.elapsed() < PLAY {
            std::thread::sleep(Duration::from_millis(10));
            // 재생 중에도 10ms마다 확인한다. 달라지면 바로 멈춘다.
            if let Err(x) = guard("재생 중") {
                failure = Some(x);
                break;
            }
            let padding = match c1.client.GetCurrentPadding() {
                Ok(p) => p,
                Err(x) => {
                    failure = Some(format!("GetCurrentPadding: {x}"));
                    break;
                }
            };
            if let Err(x) = fill(size.saturating_sub(padding)) {
                failure = Some(x);
                break;
            }
        }
        if failure.is_none() {
            // 버퍼에 남은 소리가 다 나가도록 최대 300ms 기다린다.
            let drain = Instant::now();
            while drain.elapsed() < Duration::from_millis(300)
                && c1.client.GetCurrentPadding().unwrap_or(0) > 0
            {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let _ = c1.client.Stop();
        if let Some(x) = failure {
            return Err(x);
        }
        println!(
            "DONE {{\"mode\":{},\"renderedFrames\":{rendered},\"playMs\":{}}}",
            jstr(mode),
            begin.elapsed().as_millis()
        );
        let _ = std::io::stdout().flush();
    }
    Ok(())
}

// ---------------------------------------------------------------- 부모(캡처)

#[derive(Default)]
struct ModeResult {
    mode: String,
    verified: bool,
    volume: Option<String>,
    mute: Option<String>,
    second: Option<String>,
    sessions: Option<String>,
    rendered: Option<u64>,
    samples: Option<u64>,
    peak: Option<f32>,
    beeps: Option<usize>,
    probe_error: Option<String>,
    error: Option<String>,
}

impl ModeResult {
    fn line(&self) -> String {
        let raw = |o: &Option<String>| o.clone().unwrap_or_else(|| "null".into());
        let num = |o: Option<String>| o.unwrap_or_else(|| "null".into());
        format!(
            "{{\"mode\":{},\"verified\":{},\"volume\":{},\"mute\":{},\"secondClientVolume\":{},\
             \"sessions\":{},\"renderedFrames\":{},\"loopbackSamples\":{},\"loopbackPeak\":{},\
             \"beeps\":{},\"probeError\":{},\"error\":{}}}",
            jstr(&self.mode),
            self.verified,
            raw(&self.volume),
            raw(&self.mute),
            raw(&self.second),
            raw(&self.sessions),
            num(self.rendered.map(|v| v.to_string())),
            num(self.samples.map(|v| v.to_string())),
            num(self.peak.map(jf)),
            num(self.beeps.map(|v| v.to_string())),
            jopt_str(&self.probe_error),
            jopt_str(&self.error),
        )
    }
}

/// 자식의 표준 출력 줄을 받는 채널.
type Lines = mpsc::Receiver<String>;

/// READY·FAIL·DONE 중 하나가 올 때까지 기다린다. 반환: (종류, JSON 본문).
fn wait_line(rx: &Lines, deadline: Instant) -> Res<(String, String)> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(l) => {
                for kind in ["READY", "FAIL", "DONE"] {
                    if let Some(body) = l.strip_prefix(kind).and_then(|r| r.strip_prefix(' ')) {
                        return Ok((kind.to_string(), body.to_string()));
                    }
                }
                // 그 밖의 줄은 무시한다.
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(format!(
                    "자식이 {}초 안에 응답하지 않음",
                    CHILD_TIMEOUT.as_secs()
                ));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("자식이 응답 없이 끝남".into());
            }
        }
    }
}

fn run_mode(exe: &Path, mode: &str, dry_run: bool) -> ModeResult {
    let mut r = ModeResult {
        mode: mode.to_string(),
        ..Default::default()
    };
    let mut cmd = Command::new(exe);
    cmd.args(["--render", mode]);
    if dry_run {
        cmd.arg("--dry-run");
    }
    let mut child = match cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(x) => {
            r.error = Some(format!("자식을 띄우지 못함: {x}"));
            return r;
        }
    };
    let mut stdin = child.stdin.take();
    let (tx, rx) = mpsc::channel();
    let stdout = child.stdout.take().expect("piped");
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });

    if let Err(x) = drive(&mut r, &child, &mut stdin, &rx, dry_run) {
        r.error = Some(x);
    }
    // 정리: 표준 입력을 닫아 자식이 스스로 끝나게 하고, 3초 안에 안 끝나면 끝낸다.
    drop(stdin);
    finish_child(&mut child);
    r
}

fn finish_child(child: &mut Child) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(3) {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn absorb_ready(r: &mut ModeResult, body: &str) {
    let raw = |k: &str| json_raw(body, k).map(str::to_string);
    r.volume = raw("volume");
    r.mute = raw("mute");
    r.second = raw("secondClientVolume");
    r.sessions = raw("sessions");
    r.verified = json_raw(body, "verified") == Some("true");
}

fn drive(
    r: &mut ModeResult,
    child: &Child,
    stdin: &mut Option<std::process::ChildStdin>,
    rx: &Lines,
    dry_run: bool,
) -> Res<()> {
    let (kind, body) = wait_line(rx, Instant::now() + CHILD_TIMEOUT)?;
    absorb_ready(r, &body);
    if kind != "READY" {
        return Err(json_string(&body, "error").unwrap_or(format!("자식이 {kind}를 보냄: {body}")));
    }
    if !r.verified {
        return Err("자식이 확인되지 않은 채 READY를 보냄".into());
    }
    if dry_run {
        return Ok(());
    }

    let pid = child.id();
    let probe = ToneProbe::start(ProbeTarget::Process(pid), DEFAULT_THRESHOLD)
        .map_err(|x| format!("루프백 시작 실패: {x}"))?;
    std::thread::sleep(Duration::from_millis(200));
    let w = stdin.as_mut().ok_or("자식 표준 입력이 없음")?;
    writeln!(w, "GO")
        .and_then(|_| w.flush())
        .map_err(|x| format!("GO 전송 실패: {x}"))?;

    let (kind, body) = wait_line(rx, Instant::now() + CHILD_TIMEOUT)?;
    if kind != "DONE" {
        // 재생 중 볼륨이 달라져 자식이 멈춘 경우 등.
        return Err(json_string(&body, "error").unwrap_or(format!("자식이 {kind}를 보냄: {body}")));
    }
    r.rendered = json_u64(&body, "renderedFrames");
    std::thread::sleep(Duration::from_millis(300));
    let take = probe.take();
    probe.stop();
    r.samples = Some(take.samples);
    r.peak = Some(take.peak);
    r.beeps = Some(take.beeps.len());
    r.probe_error = take.last_error;
    Ok(())
}

fn verdict(results: &[ModeResult], dry_run: bool) {
    let done: Vec<&ModeResult> = results.iter().filter(|r| r.error.is_none()).collect();
    let errors: Vec<String> = results
        .iter()
        .filter_map(|r| r.error.as_ref().map(|e| format!("{}: {e}", r.mode)))
        .collect();
    let line = if dry_run {
        if results.iter().all(|r| r.error.is_none() && r.verified) {
            "DRY_RUN (두 모드 모두 볼륨·음소거 확인까지 통과, Start하지 않음)".to_string()
        } else {
            "ERROR".to_string()
        }
    } else if done.is_empty() {
        "ERROR".to_string()
    } else {
        let names = |f: &dyn Fn(&ModeResult) -> bool| {
            done.iter()
                .filter(|r| f(r))
                .map(|r| r.mode.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let peak = |r: &ModeResult| r.peak.unwrap_or(0.0);
        let samples = |r: &ModeResult| r.samples.unwrap_or(0);
        if done.iter().any(|r| peak(r) > 0.05) {
            format!("PRE_VOLUME (peak > 0.05: {})", names(&|r| peak(r) > 0.05))
        } else if done.iter().all(|r| samples(r) == 0) {
            "NO_SAMPLES".to_string()
        } else if done.iter().all(|r| samples(r) > 0 && peak(r) < 0.001) {
            "POST_VOLUME".to_string()
        } else {
            format!(
                "INCONCLUSIVE (peak 0.001~0.05 또는 모드마다 다름: {})",
                done.iter()
                    .map(|r| format!("{} samples={} peak={}", r.mode, samples(r), jf(peak(r))))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        }
    };
    for e in &errors {
        println!("ERROR_DETAIL {e}");
    }
    println!("VERDICT: {line}");
}

fn parent_main(dry_run: bool) -> i32 {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(x) => {
            println!("ERROR_DETAIL 실행 파일 경로를 모름: {x}");
            println!("VERDICT: ERROR");
            return 1;
        }
    };
    let mut results = Vec::new();
    for (mode, _) in MODES {
        let r = run_mode(&exe, mode, dry_run);
        println!("{}", r.line());
        let _ = std::io::stdout().flush();
        results.push(r);
    }
    verdict(&results, dry_run);
    0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut render: Option<String> = None;
    let mut dry_run = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--render" => {
                i += 1;
                render = args.get(i).cloned();
                if render.is_none() {
                    eprintln!("--render에는 모드가 필요합니다(vol0mute, vol0)");
                    std::process::exit(2);
                }
            }
            "--dry-run" => dry_run = true,
            "-h" | "--help" => {
                println!(
                    "silent_loopback [--dry-run]\n  세션 볼륨 0 + 프로세스 루프백 확인(머리 문서 참고)\n  \
                     --dry-run   자식이 READY까지만 하고 Start하지 않음"
                );
                return;
            }
            other => {
                eprintln!("알 수 없는 인자: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let code = match render {
        Some(mode) => child_main(&mode, dry_run),
        None => parent_main(dry_run),
    };
    std::process::exit(code);
}
