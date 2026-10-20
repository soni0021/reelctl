//! Composite `timeline_view` PNGs: filmstrip, waveform, and word ticks.
//!
//! Frames are emitted only for cut boundaries the decompiler already found.
//! The agent is expected to open those PNGs, not the source reel.

#![warn(clippy::all)]

use std::fs;
use std::path::{Path, PathBuf};

use edit_dna::{BoundaryView, Word};
use image::{ImageBuffer, Rgb};
use thiserror::Error;

const SCALE: u32 = 2;
const WAVE_H: u32 = 64;
const TICK_H: u32 = 24;
const GAP: u32 = 4;
const BG: [u8; 3] = [0x14, 0x16, 0x1c];
const WAVE: [u8; 3] = [0x3d, 0xdc, 0x97];
const AXIS: [u8; 3] = [0x2a, 0x30, 0x3a];
const CUT: [u8; 3] = [0xff, 0x5a, 0x36];
const TICK: [u8; 3] = [0xf5, 0xc1, 0x6c];

#[derive(Debug, Error)]
pub enum FilmstripError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("png: {0}")]
    Image(#[from] image::ImageError),
    #[error("{0}")]
    Message(String),
}

/// Directory next to `edit_dna.json` where cut-boundary PNGs are written.
pub fn default_dir_for_output(output: &Path) -> PathBuf {
    let stem = output
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("edit_dna");
    let mut dir = output.to_path_buf();
    dir.set_file_name(format!("{stem}_timeline"));
    dir
}

/// Write one PNG per boundary. Returns the paths in cut order.
pub fn write_boundary_views(
    dir: &Path,
    views: &[BoundaryView],
    words: &[Word],
) -> Result<Vec<PathBuf>, FilmstripError> {
    fs::create_dir_all(dir)?;
    let mut paths = Vec::with_capacity(views.len());
    for (index, view) in views.iter().enumerate() {
        let path = dir.join(format!(
            "timeline_view_cut_{index:02}_f{}.png",
            view.frame_index
        ));
        let image = render(view, words)?;
        image.save(&path)?;
        paths.push(path);
    }
    Ok(paths)
}

fn render(
    view: &BoundaryView,
    words: &[Word],
) -> Result<ImageBuffer<Rgb<u8>, Vec<u8>>, FilmstripError> {
    let thumb_w = view.thumb_width.max(1) * SCALE;
    let thumb_h = view.thumb_height.max(1) * SCALE;
    let columns = view.thumbs.len().max(1) as u32;
    let width = columns * thumb_w;
    let height = thumb_h + GAP + WAVE_H + GAP + TICK_H;
    let mut buffer = vec![0u8; (width * height * 3) as usize];
    for pixel in buffer.chunks_exact_mut(3) {
        pixel.copy_from_slice(&BG);
    }

    for (index, thumb) in view.thumbs.iter().enumerate() {
        blit_nearest(
            thumb,
            view.thumb_width,
            view.thumb_height,
            &mut buffer,
            width,
            index as u32 * thumb_w,
            0,
        );
    }
    if view.cut_thumb_index < view.thumbs.len() {
        let x = view.cut_thumb_index as u32 * thumb_w;
        vline(&mut buffer, width, height, x, 0, thumb_h, CUT);
    }

    let wave_y = thumb_h + GAP;
    draw_waveform(&mut buffer, width, wave_y, WAVE_H, &view.pcm);
    let tick_y = wave_y + WAVE_H + GAP;
    let window_end = pcm_end_ms(view);
    for word in words {
        if let Some(x) = x_for_ms(word.start_ms, view.pcm_start_ms, window_end, width) {
            vline(&mut buffer, width, height, x, tick_y, TICK_H, TICK);
        }
    }

    ImageBuffer::from_raw(width, height, buffer)
        .ok_or_else(|| FilmstripError::Message("png buffer size mismatch".to_string()))
}

fn pcm_end_ms(view: &BoundaryView) -> u64 {
    if view.sample_rate == 0 || view.pcm.is_empty() {
        return view.pcm_start_ms;
    }
    view.pcm_start_ms + (view.pcm.len() as u64 * 1_000) / u64::from(view.sample_rate)
}

fn blit_nearest(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst: &mut [u8],
    dst_w: u32,
    origin_x: u32,
    origin_y: u32,
) {
    if src_w == 0 || src_h == 0 {
        return;
    }
    let expected = (src_w * src_h * 3) as usize;
    if src.len() < expected {
        return;
    }
    for y in 0..src_h {
        for x in 0..src_w {
            let si = ((y * src_w + x) * 3) as usize;
            let pixel = [src[si], src[si + 1], src[si + 2]];
            for oy in 0..SCALE {
                for ox in 0..SCALE {
                    put(
                        dst,
                        dst_w,
                        origin_x + x * SCALE + ox,
                        origin_y + y * SCALE + oy,
                        pixel,
                    );
                }
            }
        }
    }
}

fn draw_waveform(dst: &mut [u8], width: u32, origin_y: u32, height: u32, pcm: &[f32]) {
    let mid = origin_y + height / 2;
    hline(dst, width, mid, AXIS);
    if pcm.is_empty() || width == 0 {
        return;
    }
    for x in 0..width {
        let start = (x as usize * pcm.len()) / width as usize;
        let end = (((x + 1) as usize * pcm.len()) / width as usize).max(start + 1);
        let end = end.min(pcm.len());
        let mut lo = 0.0f32;
        let mut hi = 0.0f32;
        for sample in &pcm[start..end] {
            lo = lo.min(*sample);
            hi = hi.max(*sample);
        }
        let y0 = sample_to_y(hi, origin_y, height);
        let y1 = sample_to_y(lo, origin_y, height);
        let top = y0.min(y1);
        let bottom = y0.max(y1).max(top);
        for y in top..=bottom {
            put(dst, width, x, y, WAVE);
        }
    }
}

fn sample_to_y(sample: f32, origin_y: u32, height: u32) -> u32 {
    let clamped = sample.clamp(-1.0, 1.0);
    let unit = (1.0 - clamped) * 0.5;
    origin_y + (unit * (height.saturating_sub(1)) as f32) as u32
}

fn x_for_ms(ms: u64, start_ms: u64, end_ms: u64, width: u32) -> Option<u32> {
    if width == 0 || ms < start_ms || ms > end_ms || end_ms <= start_ms {
        return None;
    }
    let span = end_ms - start_ms;
    let pos = (u128::from(ms - start_ms) * u128::from(width)) / u128::from(span);
    Some(u32::try_from(pos).unwrap_or(width - 1).min(width - 1))
}

fn vline(dst: &mut [u8], width: u32, _height: u32, x: u32, y: u32, h: u32, color: [u8; 3]) {
    for row in y..y.saturating_add(h) {
        put(dst, width, x, row, color);
    }
}

fn hline(dst: &mut [u8], width: u32, y: u32, color: [u8; 3]) {
    for x in 0..width {
        put(dst, width, x, y, color);
    }
}

fn put(dst: &mut [u8], width: u32, x: u32, y: u32, color: [u8; 3]) {
    if x >= width {
        return;
    }
    let index = (y as usize * width as usize + x as usize) * 3;
    if index + 2 < dst.len() {
        dst[index] = color[0];
        dst[index + 1] = color[1];
        dst[index + 2] = color[2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_png_with_word_tick_band() {
        let dir = std::env::temp_dir().join(format!("reelctl-filmstrip-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let view = BoundaryView {
            frame_index: 90,
            time_ms: 3_000,
            thumb_width: 4,
            thumb_height: 2,
            thumbs: vec![
                vec![
                    220, 20, 20, 220, 20, 20, 20, 20, 220, 20, 20, 220, 240, 240, 240, 240, 240,
                    240, 240, 240, 240, 240, 240, 240
                ];
                2
            ],
            cut_thumb_index: 1,
            sample_rate: 8,
            pcm_start_ms: 2_000,
            pcm: vec![0.0, 0.5, -0.5, 0.25, -0.25, 0.1, -0.1, 0.0],
        };
        let words = vec![Word {
            text: "burst-0".to_string(),
            start_ms: 2_400,
            end_ms: 2_800,
        }];
        let paths = write_boundary_views(&dir, &[view], &words).expect("write");
        assert_eq!(paths.len(), 1);
        let bytes = fs::read(&paths[0]).expect("read png");
        assert!(bytes.starts_with(b"\x89PNG"));
        assert!(bytes.len() > 64);
        let _ = fs::remove_dir_all(&dir);
    }
}
