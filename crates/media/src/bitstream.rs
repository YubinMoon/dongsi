//! 인코더 출력 비트스트림 정리.
//!
//! WebRTC 패킷화기가 바로 쓸 수 있게 인코더마다 다른 H.264 출력을 한 형태로 맞춘다(비디오는 H.264만, D-103).
//!
//! - Annex B(4바이트 시작 코드). 키프레임마다 SPS/PPS를 앞에 둔다. 인코더가 빠뜨리면
//!   마지막으로 본 값을 끼워 넣는다. AUD·filler NAL은 뺀다.

/// Annex B 바이트열에서 NAL 단위(시작 코드 제외)를 차례로 꺼낸다.
///
/// 시작 코드가 하나도 없으면 전체를 NAL 하나로 본다.
pub(crate) fn annexb_nals(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new(); // (시작 코드 위치, NAL 시작 위치)
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    if starts.is_empty() {
        return if data.is_empty() { vec![] } else { vec![data] };
    }
    let mut out = Vec::with_capacity(starts.len());
    for (k, &(_, nal_start)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map_or(data.len(), |&(sc, _)| sc);
        let mut nal = &data[nal_start..end];
        // 다음 4바이트 시작 코드의 앞 0이나 trailing_zero_8bits를 떼어 낸다.
        while let [rest @ .., 0] = nal {
            nal = rest;
        }
        if !nal.is_empty() {
            out.push(nal);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NalKind {
    Sps,
    Pps,
    /// 버릴 NAL(AUD, filler)
    Drop,
    /// IDR 슬라이스
    Idr,
    Other,
}

fn nal_kind(nal: &[u8]) -> NalKind {
    let Some(&b0) = nal.first() else {
        return NalKind::Drop;
    };
    match b0 & 0x1f {
        7 => NalKind::Sps,
        8 => NalKind::Pps,
        9 | 12 => NalKind::Drop,
        5 => NalKind::Idr,
        _ => NalKind::Other,
    }
}

/// H.264 출력 정리기. 매개변수 집합(SPS/PPS)을 기억한다.
#[derive(Debug, Default)]
pub(crate) struct H264Fixer {
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl H264Fixer {
    /// 인코더 extradata(Annex B)에 매개변수 집합이 있으면 기억해 둔다.
    pub fn learn(&mut self, data: &[u8]) {
        for nal in annexb_nals(data) {
            self.remember(nal);
        }
    }

    fn remember(&mut self, nal: &[u8]) {
        let slot = match nal_kind(nal) {
            NalKind::Sps => &mut self.sps,
            NalKind::Pps => &mut self.pps,
            _ => return,
        };
        *slot = Some(nal.to_vec());
    }

    /// 접근 단위 하나를 정리한다. 돌려주는 bool은 실제 키프레임(IDR 포함) 여부.
    pub fn process(&mut self, data: &[u8], keyframe_hint: bool) -> (Vec<u8>, bool) {
        let nals = annexb_nals(data);
        let (mut has_sps, mut has_pps, mut idr) = (false, false, false);
        for nal in &nals {
            match nal_kind(nal) {
                NalKind::Sps => has_sps = true,
                NalKind::Pps => has_pps = true,
                NalKind::Idr => idr = true,
                _ => {}
            }
            self.remember(nal);
        }
        let keyframe = idr || keyframe_hint;
        let mut out = Vec::with_capacity(data.len() + 64);
        if keyframe {
            for (need, ps) in [(!has_sps, &self.sps), (!has_pps, &self.pps)] {
                if let (true, Some(ps)) = (need, ps) {
                    push_nal(&mut out, ps);
                }
            }
        }
        for nal in nals {
            if nal_kind(nal) != NalKind::Drop {
                push_nal(&mut out, nal);
            }
        }
        (out, keyframe)
    }
}

fn push_nal(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
}

/// H.264 접근 단위에 들어 있는 NAL 유형 번호들(검증·테스트용).
pub fn nal_types(data: &[u8]) -> Vec<u8> {
    annexb_nals(data).into_iter().map(|n| n[0] & 0x1f).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_annexb() {
        let data = [
            0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 0, 0, 1, 0x65, 4, 5, 0,
        ];
        let nals = annexb_nals(&data);
        assert_eq!(
            nals,
            vec![&[0x67, 1, 2][..], &[0x68, 3][..], &[0x65, 4, 5][..]]
        );
        assert_eq!(nal_types(&data), vec![7, 8, 5]);
        assert!(annexb_nals(&[]).is_empty());
    }

    #[test]
    fn h264_inserts_missing_parameter_sets_and_drops_aud() {
        let mut f = H264Fixer::default();
        // 첫 키프레임: AUD + SPS + PPS + IDR
        let first = [
            0, 0, 0, 1, 0x09, 0xf0, 0, 0, 0, 1, 0x67, 0xaa, 0, 0, 0, 1, 0x68, 0xbb, 0, 0, 0, 1,
            0x65, 0x11,
        ];
        let (out, key) = f.process(&first, false);
        assert!(key);
        assert_eq!(nal_types(&out), vec![7, 8, 5]);
        // P 프레임: 그대로
        let (out, key) = f.process(&[0, 0, 1, 0x41, 0x22], false);
        assert!(!key);
        assert_eq!(out, vec![0, 0, 0, 1, 0x41, 0x22]);
        // SPS/PPS 없는 IDR: 앞에 끼워 넣는다
        let (out, key) = f.process(&[0, 0, 0, 1, 0x06, 0x05, 0, 0, 0, 1, 0x65, 0x33], true);
        assert!(key);
        assert_eq!(nal_types(&out), vec![7, 8, 6, 5]);
        assert_eq!(annexb_nals(&out)[0], &[0x67, 0xaa]);
    }
}
