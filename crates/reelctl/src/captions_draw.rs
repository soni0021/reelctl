//! Caption pictures for `render`: each caption state (a group of words, with one
//! word highlighted or none) drawn with a real font into a transparent picture the
//! size of the output, and an image sequence with one picture per timeline frame
//! (hard links, so a long reel costs one file per state). This ffmpeg build has no
//! `drawtext`, so the text is drawn here with `ab_glyph`. No font is bundled: the
//! style's font, or the first of [`DEFAULT_FONTS`] that exists.

use std::fs;
use std::path::{Path, PathBuf};

use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use anyhow::{Context, Result};
use image::{Rgba, RgbaImage};
use timeline::{CaptionPlan, CaptionPosition, CaptionStyle, RenderSize};

use crate::timeline_cmd::usage;

/// Bold sans fonts that ship with the system, tried in order.
pub const DEFAULT_FONTS: [&str; 3] = [
    "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
    "/Library/Fonts/Arial Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
];
/// Line height as a multiple of the font size.
const LINE_HEIGHT: f32 = 1.2;
/// Box padding as a share of the font size.
const BOX_PADDING: f32 = 0.35;
/// The shadow's opacity.
const SHADOW_ALPHA: f32 = 0.6;

/// The font file for `style`: its own (which must exist), else the first default
/// that does. Exit 2 when there is none, naming what was tried.
pub fn find_font(style: &CaptionStyle) -> Result<PathBuf> {
    if let Some(font) = &style.font {
        let path = PathBuf::from(font);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(usage(format!("the caption font {font} is not a file")))
        };
    }
    DEFAULT_FONTS
        .iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
        .ok_or_else(|| {
            usage(format!(
                "no caption font found (tried {}); give one with --font <path.ttf>",
                DEFAULT_FONTS.join(", ")
            ))
        })
}

pub fn load_font(path: &Path) -> Result<FontVec> {
    let bytes = fs::read(path).with_context(|| format!("read font {}", path.display()))?;
    FontVec::try_from_vec(bytes).map_err(|_| {
        usage(format!(
            "{} is not a font this tool can read (.ttf or .otf)",
            path.display()
        ))
    })
}

fn rgb(text: &str) -> [u8; 3] {
    let hex = text.trim_start_matches('#');
    let channel =
        |at: usize| u8::from_str_radix(hex.get(at..at + 2).unwrap_or("00"), 16).unwrap_or(0);
    [channel(0), channel(2), channel(4)]
}

/// A coverage mask the size of the picture.
struct Mask {
    width: usize,
    height: usize,
    data: Vec<f32>,
}

impl Mask {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height],
        }
    }

    fn add(&mut self, x: i64, y: i64, coverage: f32) {
        if x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height {
            let cell = &mut self.data[y as usize * self.width + x as usize];
            *cell = cell.max(coverage.clamp(0.0, 1.0));
        }
    }

    /// The mask grown by `radius` pixels (a square max filter, in two passes).
    fn dilate(&self, radius: usize) -> Mask {
        if radius == 0 {
            return Mask {
                width: self.width,
                height: self.height,
                data: self.data.clone(),
            };
        }
        let mut across = vec![0.0f32; self.data.len()];
        for y in 0..self.height {
            let row = &self.data[y * self.width..(y + 1) * self.width];
            for x in 0..self.width {
                let from = x.saturating_sub(radius);
                let to = (x + radius + 1).min(self.width);
                across[y * self.width + x] = row[from..to].iter().fold(0.0, |a: f32, &b| a.max(b));
            }
        }
        let mut data = vec![0.0f32; self.data.len()];
        for x in 0..self.width {
            for y in 0..self.height {
                let from = y.saturating_sub(radius);
                let to = (y + radius + 1).min(self.height);
                let mut best = 0.0f32;
                for k in from..to {
                    best = best.max(across[k * self.width + x]);
                }
                data[y * self.width + x] = best;
            }
        }
        Mask {
            width: self.width,
            height: self.height,
            data,
        }
    }
}

/// Lay `colour` at `alpha * coverage` over the picture.
fn paint(image: &mut RgbaImage, mask: &Mask, colour: [u8; 3], alpha: f32, dx: i64, dy: i64) {
    for y in 0..mask.height {
        for x in 0..mask.width {
            let coverage = mask.data[y * mask.width + x] * alpha;
            if coverage <= 0.0 {
                continue;
            }
            let (tx, ty) = (x as i64 + dx, y as i64 + dy);
            if tx < 0 || ty < 0 || tx >= i64::from(image.width()) || ty >= i64::from(image.height())
            {
                continue;
            }
            let pixel = image.get_pixel_mut(tx as u32, ty as u32);
            let under = pixel.0;
            let under_alpha = f32::from(under[3]) / 255.0;
            let out_alpha = coverage + under_alpha * (1.0 - coverage);
            let mix = |top: u8, bottom: u8| {
                if out_alpha <= 0.0 {
                    0
                } else {
                    ((f32::from(top) * coverage
                        + f32::from(bottom) * under_alpha * (1.0 - coverage))
                        / out_alpha)
                        .round()
                        .clamp(0.0, 255.0) as u8
                }
            };
            *pixel = Rgba([
                mix(colour[0], under[0]),
                mix(colour[1], under[1]),
                mix(colour[2], under[2]),
                (out_alpha * 255.0).round().clamp(0.0, 255.0) as u8,
            ]);
        }
    }
}

/// Draw one caption: `lines` of words, the word at `highlight` (counting across
/// lines) in the highlight colour, placed and styled by `style`, on a transparent
/// `size` picture.
pub fn draw_caption(
    font: &FontVec,
    style: &CaptionStyle,
    lines: &[Vec<String>],
    highlight: Option<usize>,
    size: RenderSize,
) -> RgbaImage {
    let (width, height) = (size.width as usize, size.height as usize);
    let mut image = RgbaImage::new(size.width, size.height);
    let unit = size.width as f32 / 1080.0;
    let px = (style.font_size as f32 / 100.0 * size.height as f32).max(4.0);
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    let space = scaled.h_advance(font.glyph_id(' '));
    let line_height = px * LINE_HEIGHT;
    let block = line_height * lines.len() as f32;
    let margin = style.margin as f32 / 100.0 * size.height as f32;
    let top = match style.position {
        CaptionPosition::Top => margin,
        CaptionPosition::Middle => (size.height as f32 - block) / 2.0,
        CaptionPosition::Bottom => size.height as f32 - margin - block,
    };

    let word_width = |word: &str| {
        let mut total = 0.0;
        let mut previous = None;
        for c in word.chars() {
            let id = font.glyph_id(c);
            if let Some(before) = previous {
                total += scaled.kern(before, id);
            }
            total += scaled.h_advance(id);
            previous = Some(id);
        }
        total
    };

    let mut all = Mask::new(width, height);
    let mut lit = Mask::new(width, height);
    let mut bounds = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    let mut index = 0;
    for (row, words) in lines.iter().enumerate() {
        let line_width: f32 = words.iter().map(|word| word_width(word)).sum::<f32>()
            + space * words.len().saturating_sub(1) as f32;
        let mut x = (size.width as f32 - line_width) / 2.0;
        let baseline = top + line_height * row as f32 + (line_height - px) / 2.0 + scaled.ascent();
        bounds.0 = bounds.0.min(x);
        bounds.2 = bounds.2.max(x + line_width);
        bounds.1 = bounds.1.min(baseline - scaled.ascent());
        bounds.3 = bounds.3.max(baseline - scaled.descent());
        for word in words {
            let target_is_lit = highlight == Some(index);
            let mut previous = None;
            for c in word.chars() {
                let id = font.glyph_id(c);
                if let Some(before) = previous {
                    x += scaled.kern(before, id);
                }
                let glyph = id.with_scale_and_position(scale, point(x, baseline));
                if let Some(outlined) = font.outline_glyph(glyph) {
                    let origin = outlined.px_bounds().min;
                    outlined.draw(|gx, gy, coverage| {
                        let (tx, ty) = (origin.x as i64 + gx as i64, origin.y as i64 + gy as i64);
                        all.add(tx, ty, coverage);
                        if target_is_lit {
                            lit.add(tx, ty, coverage);
                        }
                    });
                }
                x += scaled.h_advance(id);
                previous = Some(id);
            }
            x += space;
            index += 1;
        }
    }

    if style.box_opacity > 0.0 && bounds.0 < bounds.2 {
        let pad = BOX_PADDING * px;
        let colour = rgb(&style.box_color);
        let alpha = (style.box_opacity * 255.0).round() as u8;
        let (x0, y0) = (
            (bounds.0 - pad).max(0.0) as u32,
            (bounds.1 - pad).max(0.0) as u32,
        );
        let x1 = ((bounds.2 + pad) as u32).min(size.width);
        let y1 = ((bounds.3 + pad) as u32).min(size.height);
        for y in y0..y1 {
            for x in x0..x1 {
                image.put_pixel(x, y, Rgba([colour[0], colour[1], colour[2], alpha]));
            }
        }
    }
    let outline = (style.outline_width as f32 * unit).round() as usize;
    let outlined = all.dilate(outline);
    if style.shadow > 0 {
        let offset = (style.shadow as f32 * unit).round().max(1.0) as i64;
        paint(
            &mut image,
            &outlined,
            [0, 0, 0],
            SHADOW_ALPHA,
            offset,
            offset,
        );
    }
    if outline > 0 {
        paint(&mut image, &outlined, rgb(&style.outline_color), 1.0, 0, 0);
    }
    paint(&mut image, &all, rgb(&style.color), 1.0, 0, 0);
    if let Some(highlight) = &style.highlight_color {
        paint(&mut image, &lit, rgb(highlight), 1.0, 0, 0);
    }
    image
}

/// Draw every caption state of `plan` into `dir` and lay out the image sequence
/// `render` reads: `cap_000000.png` and on, one per timeline frame. Returns the
/// sequence's `%06d` pattern.
pub fn write_caption_sequence(plan: &CaptionPlan, size: RenderSize, dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let style = &plan.captions.style;
    let font = load_font(&find_font(style)?)?;
    let mut states = Vec::with_capacity(plan.states.len());
    for (index, state) in plan.states.iter().enumerate() {
        let group = &plan.captions.groups[state.group];
        let image = draw_caption(&font, style, &group.lines(style), state.highlight, size);
        let path = dir.join(format!("state{index}.png"));
        image
            .save(&path)
            .with_context(|| format!("write {}", path.display()))?;
        states.push(path);
    }
    let blank = dir.join("blank.png");
    RgbaImage::new(size.width, size.height)
        .save(&blank)
        .with_context(|| format!("write {}", blank.display()))?;
    for (frame, shown) in plan.shown.iter().enumerate() {
        let source = shown.map_or(&blank, |state| &states[state]);
        let link = dir.join(format!("cap_{frame:06}.png"));
        fs::hard_link(source, &link)
            .or_else(|_| fs::copy(source, &link).map(|_| ()))
            .with_context(|| format!("write {}", link.display()))?;
    }
    Ok(dir.join("cap_%06d.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Option<FontVec> {
        DEFAULT_FONTS
            .iter()
            .map(Path::new)
            .find(|path| path.is_file())
            .map(|path| load_font(path).expect("font"))
    }

    fn lines(words: &[&str]) -> Vec<Vec<String>> {
        vec![words.iter().map(|word| word.to_string()).collect()]
    }

    const SIZE: RenderSize = RenderSize {
        width: 360,
        height: 640,
    };

    #[test]
    fn the_text_is_drawn_in_its_band_with_the_highlight_on_one_word() {
        let Some(font) = font() else {
            eprintln!("no system font; skipped");
            return;
        };
        let style = timeline::caption_preset("bold").unwrap();
        let image = draw_caption(&font, &style, &lines(&["HELLO", "THERE"]), Some(1), SIZE);
        let opaque: Vec<(u32, u32)> = image
            .enumerate_pixels()
            .filter(|(_, _, pixel)| pixel.0[3] > 200)
            .map(|(x, y, _)| (x, y))
            .collect();
        assert!(!opaque.is_empty());
        // Bottom preset with a 22% margin: the text sits in the lower part, above
        // the margin.
        let (top, bottom) = (
            opaque.iter().map(|p| p.1).min().unwrap(),
            opaque.iter().map(|p| p.1).max().unwrap(),
        );
        assert!(
            top > 640 / 2 && bottom < 640 - (0.22 * 640.0) as u32 + 10,
            "{top}..{bottom}"
        );
        let colours = |want: [u8; 3]| {
            image
                .pixels()
                .filter(|pixel| pixel.0[3] == 255 && pixel.0[..3] == want)
                .count()
        };
        // White, yellow and the black outline are all there.
        assert!(colours([255, 255, 255]) > 20);
        assert!(colours([255, 214, 10]) > 20);
        assert!(colours([0, 0, 0]) > 20);
        // The highlight is on the second word: right of centre.
        let yellow_x: Vec<u32> = image
            .enumerate_pixels()
            .filter(|(_, _, pixel)| pixel.0[3] == 255 && pixel.0[..3] == [255, 214, 10])
            .map(|(x, _, _)| x)
            .collect();
        assert!(
            yellow_x.iter().all(|x| *x > 360 / 2 - 10),
            "{:?}",
            yellow_x.iter().min()
        );
    }

    #[test]
    fn a_box_style_fills_behind_the_text_and_nothing_is_drawn_elsewhere() {
        let Some(font) = font() else {
            eprintln!("no system font; skipped");
            return;
        };
        let style = timeline::caption_preset("boxed").unwrap();
        let image = draw_caption(&font, &style, &lines(&["HI"]), None, SIZE);
        let boxed = image.get_pixel(180, image.height() - (0.22 * 640.0) as u32 - 5);
        assert!(boxed.0[3] > 100, "{boxed:?}");
        assert_eq!(image.get_pixel(5, 5).0[3], 0);
        let top = timeline::caption_preset("bold")
            .unwrap()
            .with(&timeline::StyleOverrides {
                position: Some(CaptionPosition::Top),
                ..Default::default()
            })
            .unwrap();
        let image = draw_caption(&font, &top, &lines(&["HI"]), None, SIZE);
        let lowest = image
            .enumerate_pixels()
            .filter(|(_, _, pixel)| pixel.0[3] > 0)
            .map(|(_, y, _)| y)
            .max()
            .unwrap();
        assert!(lowest < 640 / 2, "{lowest}");
    }

    #[test]
    fn a_missing_font_is_a_usage_error_that_names_it() {
        let mut style = timeline::caption_preset("bold").unwrap();
        style.font = Some("/no/such/font.ttf".to_string());
        let error = find_font(&style).unwrap_err();
        assert!(error.to_string().contains("/no/such/font.ttf"));
        let code = error
            .downcast_ref::<crate::understand_cmd::ExitWith>()
            .map(|exit| exit.code);
        assert_eq!(code, Some(2));
    }

    #[test]
    fn dilation_grows_a_point_into_a_square() {
        let mut mask = Mask::new(9, 9);
        mask.add(4, 4, 1.0);
        let grown = mask.dilate(2);
        let set: usize = grown.data.iter().filter(|value| **value > 0.0).count();
        assert_eq!(set, 25);
    }
}
