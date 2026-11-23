//! GPU 테스트 소스: 화면 캡처 없이 GPU 경로([`Converter::ingest`](crate::convert::Converter::ingest))를 타는 BGRA 텍스처.
//!
//! 매 프레임 큰 BGRA를 CPU에서 올리지 않는다. 바탕 텍스처를 만들 때 한 번만 올리고, 프레임마다 작은 조각(막대, 번호 블록)을
//! [`CopySubresourceRegion`]으로 GPU 안에서 덮는다.
//!
//! ## 그림 배치
//!
//! 가로를 [`COLUMNS`]칸으로 나눈 단위 길이 u = 너비 / 20(최소 32px)를 기준으로 한다. 크기가 줄어도(인코더 입력이
//! 더 작아도) 같은 비율로 읽을 수 있게 위치를 u의 배수로 둔다.
//!
//! - 맨 윗줄(높이 u): 칸 0 흰색, 칸 1 검정(동기), 칸 2~17 프레임 번호 16비트(LSB 먼저, 1 = 흰색 / 0 = 검정), 칸 18 흰색, 칸 19
//!   검정(동기). 번호는 `n & 0xFFFF`다(60fps에서 약 18분마다 되돌아간다).
//! - 높이 2u~3u 줄: 어두운 바탕 위에서 프레임마다 오른쪽으로 움직이는 흰 막대(너비 u, 높이 u). 위치 x = (n × max(u/8, 1)) mod
//!   (너비 − u). 끝에 닿으면 처음으로 돌아간다.
//! - 나머지: [`GpuPatternKind::Bars`]는 고정한 어두운 색 막대 바탕, [`GpuPatternKind::Noise`]는 프레임마다 화면 전체가 바뀌는
//!   무작위 그림이다.

use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_SUBRESOURCE_DATA, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use crate::MediaError;
use crate::convert::tex_desc;
use crate::d3d::D3d;

/// 가로 칸 수.
pub(crate) const COLUMNS: u32 = 20;
/// 프레임 번호 비트 수.
const BITS: u32 = 16;
/// 번호 비트가 시작하는 칸.
const FIRST_BIT_COL: u32 = 2;
/// 만들 수 있는 최소 너비(블록 한 칸이 32px 이상이 되게).
const MIN_WIDTH: u32 = COLUMNS * 32;
/// Noise 조각 판(atlas)의 한 변과 한 번에 덮는 조각의 한 변.
const ATLAS: u32 = 1024;
const TILE: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GpuPatternKind {
    Bars,
    Noise,
}

fn unit_of(width: u32) -> u32 {
    width / COLUMNS
}

/// n번째 프레임의 막대 왼쪽 x(원본 픽셀).
pub(crate) fn bar_x(n: u64, width: u32) -> u32 {
    let u = unit_of(width).max(1);
    let step = (u / 8).max(1) as u64;
    let travel = (width - u).max(1) as u64;
    (n * step % travel) as u32
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// 크기 w×h의 BGRA 텍스처를 `pixels`(w*h*4바이트)로 채워 만든다.
fn upload(
    d3d: &D3d,
    w: u32,
    h: u32,
    pixels: &[u8],
    what: &str,
) -> Result<ID3D11Texture2D, MediaError> {
    debug_assert_eq!(pixels.len(), w as usize * h as usize * 4);
    let init = D3D11_SUBRESOURCE_DATA {
        pSysMem: pixels.as_ptr() as *const _,
        SysMemPitch: w * 4,
        SysMemSlicePitch: 0,
    };
    let mut t = None;
    // SAFETY: 초기 데이터는 호출 동안 살아 있다.
    unsafe {
        d3d.device.CreateTexture2D(
            &tex_desc(
                w,
                h,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                false,
                D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ),
            Some(&init),
            Some(&mut t),
        )
    }
    .map_err(|e| MediaError::graphics(what, e))?;
    t.ok_or_else(|| MediaError::Graphics(format!("{what}가 없습니다")))
}

fn solid(d3d: &D3d, w: u32, h: u32, v: u8, what: &str) -> Result<ID3D11Texture2D, MediaError> {
    let mut px = vec![v; w as usize * h as usize * 4];
    for a in px.as_chunks_mut::<4>().0.iter_mut() {
        a[3] = 255;
    }
    upload(d3d, w, h, &px, what)
}

/// Bars 바탕: 어두운 색 세로 띠(휘도 100 이하) 위에 옅은 사선 무늬. 전체가 어두워야 흰 막대·블록이 읽힌다.
fn bars_background(w: u32, h: u32) -> Vec<u8> {
    const BANDS: [(u8, u8, u8); 8] = [
        (96, 32, 32),
        (32, 96, 32),
        (32, 32, 96),
        (80, 80, 24),
        (24, 80, 80),
        (80, 24, 80),
        (64, 64, 64),
        (40, 40, 40),
    ];
    let mut px = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h {
        // 위에서 아래로 조금 어두워진다(0.7~1.0)
        let shade = 1.0 - 0.3 * (y as f32 / h as f32);
        for x in 0..w {
            let (r, g, b) = BANDS[(x * BANDS.len() as u32 / w) as usize];
            let stripe = if (x + y) / 16 % 2 == 0 { 8 } else { 0 };
            let i = ((y * w + x) * 4) as usize;
            px[i] = (b as f32 * shade) as u8 + stripe;
            px[i + 1] = (g as f32 * shade) as u8 + stripe;
            px[i + 2] = (r as f32 * shade) as u8 + stripe;
            px[i + 3] = 255;
        }
    }
    px
}

/// 화면 캡처 없이 GPU 경로([`Converter::ingest`](crate::convert::Converter::ingest))를 타는 테스트 소스.
/// 바탕 BGRA 텍스처를 한 번 만들고 프레임마다 작은 조각만 GPU 안에서 덮는다.
pub(crate) struct GpuPattern {
    d3d: D3d,
    kind: GpuPatternKind,
    width: u32,
    height: u32,
    unit: u32,
    /// 내보내는 텍스처(매 프레임 이 텍스처를 고쳐 쓴다. 쓰는 쪽은 [`Converter::ingest`]가 GPU 안에서 곧바로 복사한다).
    frame: ID3D11Texture2D,
    /// Bars: 손대지 않은 바탕(막대가 지나간 자리를 되살린다).
    background: Option<ID3D11Texture2D>,
    /// Noise: 무작위 조각 판.
    atlas: Option<ID3D11Texture2D>,
    /// Noise: 막대 줄을 검게 덮는 너비×u 판.
    black_band: Option<ID3D11Texture2D>,
    white_block: ID3D11Texture2D,
    black_block: ID3D11Texture2D,
    prev_bar: Option<u32>,
}

impl GpuPattern {
    /// `width`는 640 이상, `height`는 3u(= 3 × 너비 / 20) 이상이어야 한다.
    pub fn new(
        d3d: &D3d,
        width: u32,
        height: u32,
        kind: GpuPatternKind,
    ) -> Result<Self, MediaError> {
        let unit = unit_of(width);
        if width < MIN_WIDTH || height < 3 * unit {
            return Err(MediaError::Graphics(format!(
                "GPU 테스트 소스 크기가 맞지 않습니다: {width}×{height} (너비 {MIN_WIDTH} 이상, 높이 {} 이상)",
                3 * unit
            )));
        }
        let white_block = solid(d3d, unit, unit, 255, "흰 블록")?;
        let black_block = solid(d3d, unit, unit, 0, "검은 블록")?;
        let (frame, background, atlas, black_band) = match kind {
            GpuPatternKind::Bars => {
                let px = bars_background(width, height);
                (
                    upload(d3d, width, height, &px, "Bars 프레임")?,
                    Some(upload(d3d, width, height, &px, "Bars 바탕")?),
                    None,
                    None,
                )
            }
            GpuPatternKind::Noise => {
                let mut rng = 0x646f_6e67_7369_u64;
                let mut px = vec![0u8; (ATLAS * ATLAS * 4) as usize];
                for chunk in px.as_chunks_mut::<8>().0.iter_mut() {
                    chunk.copy_from_slice(&splitmix64(&mut rng).to_le_bytes());
                }
                for a in px.as_chunks_mut::<4>().0.iter_mut() {
                    a[3] = 255;
                }
                let frame = {
                    let mut t = None;
                    // SAFETY: 설명자와 출력 포인터가 유효하다.
                    unsafe {
                        d3d.device.CreateTexture2D(
                            &tex_desc(
                                width,
                                height,
                                DXGI_FORMAT_B8G8R8A8_UNORM,
                                false,
                                D3D11_BIND_SHADER_RESOURCE.0 as u32,
                            ),
                            None,
                            Some(&mut t),
                        )
                    }
                    .map_err(|e| MediaError::graphics("Noise 프레임", e))?;
                    t.ok_or_else(|| MediaError::Graphics("Noise 프레임이 없습니다".into()))?
                };
                (
                    frame,
                    None,
                    Some(upload(d3d, ATLAS, ATLAS, &px, "Noise 조각 판")?),
                    Some(solid(d3d, width, unit, 0, "검은 띠")?),
                )
            }
        };
        Ok(GpuPattern {
            d3d: d3d.clone(),
            kind,
            width,
            height,
            unit,
            frame,
            background,
            atlas,
            black_band,
            white_block,
            black_block,
            prev_bar: None,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// `dst`의 (dx, dy)에 `src`의 (sx, sy)부터 w×h를 GPU 안에서 복사한다.
    #[allow(clippy::too_many_arguments)]
    fn copy(
        &self,
        dst: &ID3D11Texture2D,
        dx: u32,
        dy: u32,
        src: &ID3D11Texture2D,
        sx: u32,
        sy: u32,
        w: u32,
        h: u32,
    ) {
        let region = D3D11_BOX {
            left: sx,
            top: sy,
            front: 0,
            right: sx + w,
            bottom: sy + h,
            back: 1,
        };
        // SAFETY: 두 텍스처 모두 같은 장치의 것이고 영역이 범위 안이다(호출한 쪽이 지킨다).
        unsafe {
            self.d3d
                .context
                .CopySubresourceRegion(dst, 0, dx, dy, 0, src, 0, Some(&region));
        }
    }

    /// n번째 프레임의 BGRA 텍스처(D3D11 기본 사용, [`Converter::ingest`]에 넣을 수 있다). 같은 텍스처를 고쳐 써서
    /// 돌려주므로 `ingest`가 곧바로 복사해 가야 한다.
    pub fn frame(&mut self, n: u64) -> Result<&ID3D11Texture2D, MediaError> {
        let u = self.unit;
        if self.kind == GpuPatternKind::Noise {
            self.draw_noise(n);
        }
        match (self.kind, self.prev_bar, &self.background) {
            // 지난 막대 자리를 바탕으로 되살린다.
            (GpuPatternKind::Bars, Some(px), Some(bg)) => {
                self.copy(&self.frame, px, 2 * u, bg, px, 2 * u, u, u);
            }
            (GpuPatternKind::Noise, _, _) => {
                if let Some(band) = &self.black_band {
                    self.copy(&self.frame, 0, 2 * u, band, 0, 0, self.width, u);
                }
            }
            _ => {}
        }
        // 번호 줄: 동기 칸 4개와 번호 비트 16개.
        for col in 0..COLUMNS {
            let high = match col {
                0 | 18 => true,
                1 | 19 => false,
                c => n >> (c - FIRST_BIT_COL) & 1 == 1,
            };
            let blk = if high {
                &self.white_block
            } else {
                &self.black_block
            };
            self.copy(&self.frame, col * u, 0, blk, 0, 0, u, u);
        }
        let x = bar_x(n, self.width);
        self.copy(&self.frame, x, 2 * u, &self.white_block, 0, 0, u, u);
        self.prev_bar = Some(x);
        Ok(&self.frame)
    }

    /// Noise: 만들 때 올려 둔 조각 판에서 무작위 창을 무작위 위치로 복사해 화면 전체를 덮는다.
    fn draw_noise(&self, n: u64) {
        let Some(atlas) = &self.atlas else { return };
        let mut rng = n ^ 0x006e_6f69_7365;
        let ox = (splitmix64(&mut rng) % TILE as u64) as i64;
        let oy = (splitmix64(&mut rng) % TILE as u64) as i64;
        let (w, h) = (self.width as i64, self.height as i64);
        let t = TILE as i64;
        let mut ty = -oy;
        while ty < h {
            let mut tx = -ox;
            while tx < w {
                let (x0, y0) = (tx.max(0), ty.max(0));
                let (x1, y1) = ((tx + t).min(w), (ty + t).min(h));
                // 조각 판 안의 창: 앞에서 잘린 만큼 안쪽에서 시작해도 판을 넘지 않게 TILE만큼 여유를 둔다.
                let sx = (splitmix64(&mut rng) % (ATLAS - TILE + 1) as u64) as i64 + (x0 - tx);
                let sy = (splitmix64(&mut rng) % (ATLAS - TILE + 1) as u64) as i64 + (y0 - ty);
                self.copy(
                    &self.frame,
                    x0 as u32,
                    y0 as u32,
                    atlas,
                    sx as u32,
                    sy as u32,
                    (x1 - x0) as u32,
                    (y1 - y0) as u32,
                );
                tx += t;
            }
            ty += t;
        }
    }
}

/// 휘도 평면 `y`(행 간격 `stride`, `width`×`height`)에서 프레임 번호(하위 16비트)와 막대 왼쪽 x(읽는 평면의 픽셀)를 읽는다.
/// 평면이 원본보다 줄어 있어도(비율이 같으면) 읽힌다. 동기 칸이 맞지 않거나 막대가 없으면 `None`.
/// 흰색/검정은 휘도 128을 경계로 가른다(limited 235/16, 전범위 255/0 모두 맞다).
#[allow(dead_code)] // 시험과 T2-3·T3b에서 쓴다
pub(crate) fn read_marks(y: &[u8], stride: usize, width: u32, height: u32) -> Option<(u64, u32)> {
    let u = width as f64 / COLUMNS as f64;
    if u < 4.0 || 3.0 * u > height as f64 || stride < width as usize {
        return None;
    }
    if y.len() < stride * (height as usize - 1) + width as usize {
        return None;
    }
    let r = ((u / 4.0) as usize).max(1);
    let mean_at = |cx: usize, cy: usize, rx: usize, ry: usize| -> f64 {
        let (x0, x1) = (cx.saturating_sub(rx), (cx + rx + 1).min(width as usize));
        let (y0, y1) = (cy.saturating_sub(ry), (cy + ry + 1).min(height as usize));
        let mut sum = 0u64;
        for row in y0..y1 {
            sum += y[row * stride + x0..row * stride + x1]
                .iter()
                .map(|&v| v as u64)
                .sum::<u64>();
        }
        sum as f64 / ((x1 - x0) * (y1 - y0)) as f64
    };
    let high = |col: u32| -> bool {
        mean_at(((col as f64 + 0.5) * u) as usize, (u * 0.5) as usize, r, r) > 128.0
    };
    if !high(0) || high(1) || !high(18) || high(19) {
        return None;
    }
    let mut n = 0u64;
    for bit in 0..BITS {
        if high(FIRST_BIT_COL + bit) {
            n |= 1 << bit;
        }
    }
    // 막대: 2u~3u 줄의 가운데(±u/4) 평균이 밝은 첫 구간의 시작.
    let cy = (2.5 * u) as usize;
    let mut start = None;
    for x in 0..width as usize {
        if mean_at(x, cy, 0, r) > 128.0 {
            let s = *start.get_or_insert(x);
            // 막대 너비(u)의 절반 이상 이어지면 막대로 본다.
            if (x - s + 1) as f64 >= u * 0.5 {
                return Some((n, s as u32));
            }
        } else {
            start = None;
        }
    }
    None
}

#[cfg(test)]
pub(crate) mod test_util {
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_RENDER_TARGET, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_QUERY_DESC,
        D3D11_QUERY_EVENT, D3D11_TEXTURE2D_DESC,
    };
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;
    use windows::core::Interface;

    use super::*;

    /// 시험에서 NV12 RENDER_TARGET 출력 텍스처를 직접 만든다(hw 풀 텍스처와 같은 설명).
    pub fn create_nv12_target(d3d: &D3d, w: u32, h: u32) -> ID3D11Texture2D {
        let mut t = None;
        // SAFETY: 설명자와 출력 포인터가 유효하다.
        unsafe {
            d3d.device.CreateTexture2D(
                &tex_desc(
                    w,
                    h,
                    DXGI_FORMAT_NV12,
                    false,
                    D3D11_BIND_RENDER_TARGET.0 as u32,
                ),
                None,
                Some(&mut t),
            )
        }
        .expect("NV12 텍스처");
        t.unwrap()
    }

    fn staging_copy(d3d: &D3d, tex: &ID3D11Texture2D) -> (ID3D11Texture2D, D3D11_TEXTURE2D_DESC) {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: 유효한 텍스처.
        unsafe { tex.GetDesc(&mut desc) };
        let mut st = None;
        // SAFETY: 설명자와 출력 포인터가 유효하다.
        unsafe {
            d3d.device.CreateTexture2D(
                &tex_desc(desc.Width, desc.Height, desc.Format, true, 0),
                None,
                Some(&mut st),
            )
        }
        .expect("스테이징");
        let st = st.unwrap();
        // SAFETY: 같은 크기·형식의 텍스처 둘이다.
        unsafe { d3d.context.CopyResource(&st, tex) };
        (st, desc)
    }

    /// NV12 텍스처의 Y 평면을 (꽉 찬 너비 행, stride = 너비)로 읽는다.
    pub fn read_nv12_y(d3d: &D3d, tex: &ID3D11Texture2D) -> (Vec<u8>, usize) {
        let (st, desc) = staging_copy(d3d, tex);
        let (w, h) = (desc.Width as usize, desc.Height as usize);
        let mut y = vec![0u8; w * h];
        // SAFETY: Map한 동안만 포인터를 쓰고 행 간격을 지킨다.
        unsafe {
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            d3d.context
                .Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .expect("Map");
            for row in 0..h {
                let src = std::slice::from_raw_parts(
                    (m.pData as *const u8).add(row * m.RowPitch as usize),
                    w,
                );
                y[row * w..(row + 1) * w].copy_from_slice(src);
            }
            d3d.context.Unmap(&st, 0);
        }
        (y, w)
    }

    /// BGRA 텍스처를 (꽉 찬 행 BGRA, 너비, 높이)로 읽는다.
    pub fn read_bgra(d3d: &D3d, tex: &ID3D11Texture2D) -> (Vec<u8>, u32, u32) {
        let (st, desc) = staging_copy(d3d, tex);
        let (w, h) = (desc.Width as usize, desc.Height as usize);
        let mut px = vec![0u8; w * h * 4];
        // SAFETY: Map한 동안만 포인터를 쓰고 행 간격을 지킨다.
        unsafe {
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            d3d.context
                .Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m))
                .expect("Map");
            for row in 0..h {
                let src = std::slice::from_raw_parts(
                    (m.pData as *const u8).add(row * m.RowPitch as usize),
                    w * 4,
                );
                px[row * w * 4..(row + 1) * w * 4].copy_from_slice(src);
            }
            d3d.context.Unmap(&st, 0);
        }
        (px, desc.Width, desc.Height)
    }

    /// BGRA에서 전범위 휘도(BT.601) 평면을 뽑는다.
    pub fn luma_of_bgra(px: &[u8]) -> Vec<u8> {
        px.as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                (0.114 * p[0] as f32 + 0.587 * p[1] as f32 + 0.299 * p[2] as f32).round() as u8
            })
            .collect()
    }

    /// 지금까지 낸 GPU 명령이 끝날 때까지 기다린다(시험의 시간 재기용. 제품 코드는 쓰지 않는다).
    pub fn wait_gpu(d3d: &D3d) {
        let mut q = None;
        // SAFETY: 설명자와 출력 포인터가 유효하다.
        unsafe {
            d3d.device
                .CreateQuery(
                    &D3D11_QUERY_DESC {
                        Query: D3D11_QUERY_EVENT,
                        MiscFlags: 0,
                    },
                    Some(&mut q),
                )
                .expect("쿼리");
            let q = q.unwrap();
            d3d.context.End(&q);
            d3d.context.Flush();
            // GetData는 아직 안 끝났으면 S_FALSE(성공 코드)를 돌려 주므로 HRESULT를 그대로 받는다.
            let vt = Interface::vtable(&d3d.context);
            while (vt.GetData)(
                Interface::as_raw(&d3d.context),
                Interface::as_raw(&q),
                std::ptr::null_mut(),
                0,
                0,
            )
            .0 != 0
            {
                std::hint::spin_loop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;
    use crate::d3d::create_d3d;

    /// 원본 해상도 BGRA에서 번호·막대를 읽는다.
    fn marks_of(d3d: &D3d, p: &mut GpuPattern, n: u64) -> Option<(u64, u32)> {
        let tex = p.frame(n).unwrap().clone();
        let (px, w, h) = read_bgra(d3d, &tex);
        read_marks(&luma_of_bgra(&px), w as usize, w, h)
    }

    #[test]
    fn rejects_sizes_that_cannot_hold_the_marks() {
        let d3d = create_d3d().unwrap();
        assert!(GpuPattern::new(&d3d, 639, 400, GpuPatternKind::Bars).is_err());
        assert!(GpuPattern::new(&d3d, 640, 90, GpuPatternKind::Bars).is_err());
        assert!(GpuPattern::new(&d3d, 640, 360, GpuPatternKind::Noise).is_ok());
    }

    #[test]
    fn bars_marks_roundtrip_and_bar_moves_right() {
        let d3d = create_d3d().unwrap();
        let mut p = GpuPattern::new(&d3d, 1280, 720, GpuPatternKind::Bars).unwrap();
        assert_eq!(p.size(), (1280, 720));
        let mut last_x = None;
        // 순서대로, 건너뛰어서(되돌아가는 막대 자리 되살리기 포함), 16비트 경계
        for n in [0u64, 1, 2, 3, 4, 100, 99, 101, 65_535, 65_536, 65_537] {
            let (num, x) =
                marks_of(&d3d, &mut p, n).unwrap_or_else(|| panic!("프레임 {n}을 읽지 못함"));
            assert_eq!(num, n & 0xFFFF, "프레임 번호");
            assert_eq!(x, bar_x(n, 1280), "프레임 {n}의 막대 위치");
            if n < 5 {
                if let Some(prev) = last_x {
                    assert!(x > prev, "막대가 오른쪽으로 움직여야 한다: {prev} → {x}");
                }
                last_x = Some(x);
            }
        }
        // 한 바퀴를 돌아 처음으로 돌아간다.
        let travel = 1280 - 64;
        let wrap_n = (0..)
            .find(|&n| bar_x(n, 1280) < bar_x(n.saturating_sub(1), 1280))
            .unwrap();
        assert!(
            wrap_n as u32 * 8 >= travel - 8,
            "주기가 너무 짧다: {wrap_n}프레임"
        );
        let (num, x) = marks_of(&d3d, &mut p, wrap_n).unwrap();
        assert_eq!((num, x), (wrap_n & 0xFFFF, bar_x(wrap_n, 1280)));
    }

    #[test]
    fn noise_changes_whole_frame_and_keeps_marks() {
        let d3d = create_d3d().unwrap();
        let mut p = GpuPattern::new(&d3d, 1920, 1080, GpuPatternKind::Noise).unwrap();
        let read = |p: &mut GpuPattern, n: u64| {
            let t = p.frame(n).unwrap().clone();
            read_bgra(&d3d, &t)
        };
        let (a, w, h) = read(&mut p, 10);
        let (b, _, _) = read(&mut p, 11);
        // 번호·막대
        for (px, n) in [(&a, 10u64), (&b, 11)] {
            let (num, x) = read_marks(&luma_of_bgra(px), w as usize, w, h).expect("번호 읽기");
            assert_eq!((num, x), (n, bar_x(n, w)));
        }
        // 번호·막대 줄(위쪽 3u)을 빼고 바뀐 화소 비율. 위쪽 3줄 높이를 뺀다.
        let skip = (3 * w / COLUMNS) as usize * w as usize * 4;
        let (ra, rb) = (&a[skip..], &b[skip..]);
        let differing = ra
            .as_chunks::<4>()
            .0
            .iter()
            .zip(rb.as_chunks::<4>().0.iter())
            .filter(|(p, q)| p[..3] != q[..3])
            .count();
        let frac = differing as f64 / (ra.len() / 4) as f64;
        let mean_abs = ra
            .iter()
            .zip(rb)
            .map(|(&p, &q)| (p as i32 - q as i32).unsigned_abs() as u64)
            .sum::<u64>() as f64
            / ra.len() as f64;
        eprintln!(
            "noise: 연속 두 프레임에서 달라진 화소 {:.1}%, 평균 절대 차이 {mean_abs:.1}",
            frac * 100.0
        );
        assert!(frac > 0.95, "화면 전체가 바뀌어야 한다: {frac}");
        assert!(mean_abs > 40.0, "{mean_abs}");
        // 알파는 항상 불투명이다.
        assert!(a.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn read_marks_rejects_flat_and_misaligned_planes() {
        let flat = vec![16u8; 640 * 360];
        assert_eq!(read_marks(&flat, 640, 640, 360), None);
        let white = vec![235u8; 640 * 360];
        assert_eq!(read_marks(&white, 640, 640, 360), None);
        assert_eq!(read_marks(&flat[..100], 640, 640, 360), None);
    }
}
