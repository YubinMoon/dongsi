//! 인코더 출력 비트스트림 정리.
//!
//! WebRTC 패킷화기가 바로 쓸 수 있게 인코더마다 다른 출력을 한 형태로 맞춘다.
//!
//! - H.264: Annex B(4바이트 시작 코드). 키프레임마다 SPS/PPS를 앞에 둔다. 인코더가 빠뜨리면
//!   마지막으로 본 값을 끼워 넣는다. AUD·filler NAL은 뺀다.
//! - AV1: low-overhead 형식(모든 OBU에 `obu_has_size_field = 1`). temporal delimiter·padding
//!   OBU는 빼고, 키프레임에는 sequence header OBU가 반드시 들어가게 한다.

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

pub(crate) const OBU_SEQUENCE_HEADER: u8 = 1;
pub(crate) const OBU_TEMPORAL_DELIMITER: u8 = 2;
pub(crate) const OBU_TILE_LIST: u8 = 8;
pub(crate) const OBU_PADDING: u8 = 15;

/// 파싱한 OBU 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Obu<'a> {
    pub obu_type: u8,
    /// 확장 헤더 바이트(temporal_id, spatial_id). 없으면 `None`.
    pub extension: Option<u8>,
    pub payload: &'a [u8],
}

pub(crate) fn read_leb128(data: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for i in 0..8 {
        let b = *data.get(i)?;
        value |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

pub(crate) fn write_leb128(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

/// OBU 열을 파싱한다. 크기 필드가 없는 OBU는 남은 바이트 전부를 차지한다.
pub(crate) fn parse_obus(mut data: &[u8]) -> Result<Vec<Obu<'_>>, String> {
    let mut out = Vec::new();
    while !data.is_empty() {
        let h = data[0];
        if h & 0x80 != 0 {
            return Err("OBU forbidden bit가 1입니다".into());
        }
        let obu_type = (h >> 3) & 0x0f;
        let has_ext = h & 0x04 != 0;
        let has_size = h & 0x02 != 0;
        let mut pos = 1;
        let extension = if has_ext {
            let e = *data.get(1).ok_or("OBU 확장 헤더가 잘렸습니다")?;
            pos += 1;
            Some(e)
        } else {
            None
        };
        let size = if has_size {
            let (s, n) = read_leb128(&data[pos..]).ok_or("OBU 크기(leb128)가 잘못되었습니다")?;
            pos += n;
            s as usize
        } else {
            data.len() - pos
        };
        let end = pos.checked_add(size).filter(|&e| e <= data.len());
        let end = end.ok_or_else(|| format!("OBU 크기 {size}가 남은 길이를 넘습니다"))?;
        out.push(Obu {
            obu_type,
            extension,
            payload: &data[pos..end],
        });
        data = &data[end..];
    }
    Ok(out)
}

/// OBU를 low-overhead 형식(크기 필드 있음)으로 쓴다.
pub(crate) fn write_obu(out: &mut Vec<u8>, obu: &Obu<'_>) {
    let mut h = (obu.obu_type << 3) | 0x02;
    if obu.extension.is_some() {
        h |= 0x04;
    }
    out.push(h);
    if let Some(e) = obu.extension {
        out.push(e);
    }
    write_leb128(out, obu.payload.len() as u64);
    out.extend_from_slice(obu.payload);
}

/// AV1 출력 정리기. sequence header OBU를 기억한다.
#[derive(Debug, Default)]
pub(crate) struct Av1Fixer {
    seq_header: Option<Vec<u8>>,
}

impl Av1Fixer {
    /// 인코더 extradata(av1C 또는 OBU 열)에서 sequence header를 기억한다.
    pub fn learn(&mut self, extradata: &[u8]) {
        // av1C: marker(1) version(7) = 0x81 로 시작하는 4바이트 헤더 뒤에 configOBUs가 온다.
        let obus = if extradata.len() >= 4 && extradata[0] == 0x81 {
            &extradata[4..]
        } else {
            extradata
        };
        if let Ok(list) = parse_obus(obus) {
            for o in list.iter().filter(|o| o.obu_type == OBU_SEQUENCE_HEADER) {
                let mut v = Vec::new();
                write_obu(&mut v, o);
                self.seq_header = Some(v);
            }
        }
    }

    /// 시간 단위(temporal unit) 하나를 정리한다.
    pub fn process(&mut self, data: &[u8], keyframe: bool) -> Result<Vec<u8>, String> {
        let obus = parse_obus(data)?;
        let mut out = Vec::with_capacity(data.len() + 32);
        let has_seq = obus.iter().any(|o| o.obu_type == OBU_SEQUENCE_HEADER);
        if keyframe
            && !has_seq
            && let Some(seq) = &self.seq_header
        {
            out.extend_from_slice(seq);
        }
        for o in &obus {
            match o.obu_type {
                OBU_TEMPORAL_DELIMITER | OBU_PADDING | OBU_TILE_LIST => continue,
                OBU_SEQUENCE_HEADER => {
                    let mut v = Vec::new();
                    write_obu(&mut v, o);
                    out.extend_from_slice(&v);
                    self.seq_header = Some(v);
                }
                _ => write_obu(&mut out, o),
            }
        }
        Ok(out)
    }
}

/// AV1 시간 단위에 들어 있는 OBU 유형 번호들(검증·테스트용).
pub fn obu_types(data: &[u8]) -> Result<Vec<u8>, String> {
    Ok(parse_obus(data)?.iter().map(|o| o.obu_type).collect())
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

    #[test]
    fn leb128_roundtrip() {
        for v in [
            0u64,
            1,
            127,
            128,
            300,
            16_383,
            16_384,
            1 << 28,
            (1 << 35) + 5,
        ] {
            let mut buf = Vec::new();
            write_leb128(&mut buf, v);
            assert_eq!(read_leb128(&buf), Some((v, buf.len())));
        }
        assert_eq!(read_leb128(&[0x80, 0x80]), None);
    }

    fn obu(t: u8, ext: Option<u8>, payload: &[u8], with_size: bool) -> Vec<u8> {
        let mut h = t << 3;
        if ext.is_some() {
            h |= 0x04;
        }
        if with_size {
            h |= 0x02;
        }
        let mut v = vec![h];
        if let Some(e) = ext {
            v.push(e);
        }
        if with_size {
            write_leb128(&mut v, payload.len() as u64);
        }
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn av1_strips_td_and_adds_size_fields() {
        let mut f = Av1Fixer::default();
        // TD + sequence header + frame(크기 필드 없음, 마지막)
        let mut tu = obu(OBU_TEMPORAL_DELIMITER, None, &[], true);
        tu.extend(obu(OBU_SEQUENCE_HEADER, None, &[1, 2, 3], true));
        tu.extend(obu(6, Some(0x08), &[9; 200], false));
        let out = f.process(&tu, true).unwrap();
        let parsed = parse_obus(&out).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].obu_type, OBU_SEQUENCE_HEADER);
        assert_eq!(parsed[1].obu_type, 6);
        assert_eq!(parsed[1].extension, Some(0x08));
        assert_eq!(parsed[1].payload.len(), 200);
        // 모든 OBU에 크기 필드가 있다
        assert!(out[0] & 0x02 != 0);

        // 다음 키프레임에 sequence header가 없으면 앞에 붙인다
        let mut tu2 = obu(OBU_TEMPORAL_DELIMITER, None, &[], true);
        tu2.extend(obu(6, None, &[7; 10], true));
        let out2 = f.process(&tu2, true).unwrap();
        assert_eq!(obu_types(&out2).unwrap(), vec![OBU_SEQUENCE_HEADER, 6]);
        // 키프레임이 아니면 붙이지 않는다
        let out3 = f.process(&tu2, false).unwrap();
        assert_eq!(obu_types(&out3).unwrap(), vec![6]);
    }

    #[test]
    fn av1_learns_from_av1c() {
        let mut f = Av1Fixer::default();
        let mut extra = vec![0x81, 0x00, 0x0c, 0x00];
        extra.extend(obu(OBU_SEQUENCE_HEADER, None, &[5, 6], true));
        f.learn(&extra);
        let out = f.process(&obu(6, None, &[1], true), true).unwrap();
        assert_eq!(obu_types(&out).unwrap(), vec![OBU_SEQUENCE_HEADER, 6]);
    }

    #[test]
    fn av1_rejects_garbage() {
        assert!(parse_obus(&[0x80]).is_err());
        assert!(parse_obus(&[0x0a, 0x05, 1]).is_err());
    }
}
