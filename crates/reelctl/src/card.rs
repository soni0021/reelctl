//! The bridge card: the target URL in a 5x7 bitmap font on a plain background, as a
//! PNG the size of the output. The font is compiled in, so the card needs no font
//! file and no ffmpeg `drawtext` (not every ffmpeg build has it).

use anyhow::{bail, Result};
use image::{Rgb, RgbImage};

/// Glyph columns and rows, and the gap between glyphs, in font pixels.
const GLYPH_W: u32 = 5;
const GLYPH_H: u32 = 7;
const GAP: u32 = 1;
/// Text may use this share of the width.
const MARGIN: f64 = 0.88;
const MAX_SCALE: u32 = 10;

/// `#rrggbb` as a colour.
pub fn parse_color(text: &str) -> Result<Rgb<u8>> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("\"{text}\" is not a #rrggbb colour");
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16);
    match (channel(0), channel(2), channel(4)) {
        (Ok(r), Ok(g), Ok(b)) => Ok(Rgb([r, g, b])),
        _ => bail!("\"{text}\" is not a #rrggbb colour"),
    }
}

/// Rows of a glyph, top to bottom; bit 4 is the leftmost column.
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        ' ' => [0; 7],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        ',' => [0, 0, 0, 0, 0b01100, 0b00100, 0b01000],
        ':' => [0, 0b01100, 0b01100, 0, 0b01100, 0b01100, 0],
        '/' => [
            0b00001, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b10000,
        ],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 0b11111],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
        '=' => [0, 0, 0b11111, 0, 0b11111, 0, 0],
        '&' => [
            0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101,
        ],
        '%' => [
            0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011,
        ],
        '#' => [
            0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010,
        ],
        '@' => [
            0b01110, 0b10001, 0b10111, 0b10101, 0b10111, 0b10000, 0b01110,
        ],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        '~' => [0, 0, 0b01000, 0b10101, 0b00010, 0, 0],
        // A box for anything else, so a character is never silently dropped.
        _ => [
            0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111,
        ],
    }
}

fn fill(image: &mut RgbImage, x: u32, y: u32, size: u32, color: Rgb<u8>) {
    for dy in 0..size {
        for dx in 0..size {
            if x + dx < image.width() && y + dy < image.height() {
                image.put_pixel(x + dx, y + dy, color);
            }
        }
    }
}

/// Pixel width of `chars` glyphs at `scale`.
fn text_width(chars: usize, scale: u32) -> u32 {
    if chars == 0 {
        return 0;
    }
    (chars as u32 * (GLYPH_W + GAP) - GAP) * scale
}

/// Split `text` into lines of at most `per_line` characters.
fn wrap(text: &str, per_line: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(per_line.max(1))
        .map(|chunk| chunk.iter().collect())
        .collect()
}

/// The card: `text` centred in `foreground` on `background`, `width` by `height`
/// pixels, with a bar under it. Text that does not fit on one line at the smallest
/// size is wrapped.
pub fn draw_card(
    text: &str,
    background: Rgb<u8>,
    foreground: Rgb<u8>,
    width: u32,
    height: u32,
) -> RgbImage {
    let mut image = RgbImage::from_pixel(width, height, background);
    let usable = (f64::from(width) * MARGIN) as u32;
    let chars = text.chars().count();
    // Largest scale at which the text is one line and not too tall.
    let single = (1..=MAX_SCALE)
        .rev()
        .find(|&scale| text_width(chars, scale) <= usable && GLYPH_H * scale <= height / 4);
    let (scale, lines) = match single {
        Some(scale) => (scale, vec![text.to_string()]),
        None => {
            let scale = 2;
            let per_line = (usable / ((GLYPH_W + GAP) * scale)) as usize;
            (scale, wrap(text, per_line))
        }
    };
    let line_height = (GLYPH_H + 2) * scale;
    let block = lines.len() as u32 * line_height;
    let mut y = height.saturating_sub(block) / 2;
    let mut widest = 0;
    for line in &lines {
        let line_width = text_width(line.chars().count(), scale);
        widest = widest.max(line_width);
        let mut x = width.saturating_sub(line_width) / 2;
        for c in line.chars() {
            for (row, bits) in glyph(c).iter().enumerate() {
                for column in 0..GLYPH_W {
                    if bits >> (GLYPH_W - 1 - column) & 1 == 1 {
                        fill(
                            &mut image,
                            x + column * scale,
                            y + row as u32 * scale,
                            scale,
                            foreground,
                        );
                    }
                }
            }
            x += (GLYPH_W + GAP) * scale;
        }
        y += line_height;
    }
    // A bar under the text.
    let bar_x = width.saturating_sub(widest) / 2;
    for step in 0..widest {
        fill(&mut image, bar_x + step, y + scale, scale, foreground);
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    const BG: Rgb<u8> = Rgb([58, 28, 113]);
    const FG: Rgb<u8> = Rgb([255, 255, 255]);

    fn count(image: &RgbImage, color: Rgb<u8>) -> usize {
        image.pixels().filter(|&&pixel| pixel == color).count()
    }

    #[test]
    fn a_colour_is_parsed_from_hex() {
        assert_eq!(parse_color("#3a1c71").expect("color"), BG);
        assert_eq!(parse_color("ffffff").expect("color"), FG);
        assert!(parse_color("#fff").is_err());
        assert!(parse_color("#gggggg").is_err());
    }

    #[test]
    fn the_card_is_the_output_size_with_the_background_in_the_corner() {
        let image = draw_card("example.com/go", BG, FG, 640, 360);
        assert_eq!(image.dimensions(), (640, 360));
        assert_eq!(*image.get_pixel(0, 0), BG);
        assert_eq!(*image.get_pixel(639, 359), BG);
    }

    #[test]
    fn the_text_is_drawn_in_the_foreground_near_the_centre() {
        let image = draw_card("example.com/go", BG, FG, 640, 360);
        let mut centre = 0;
        for y in 120..240 {
            for x in 80..560 {
                if *image.get_pixel(x, y) == FG {
                    centre += 1;
                }
            }
        }
        assert!(centre > 1_000, "{centre} foreground pixels");
        assert!(count(&image, BG) > count(&image, FG));
    }

    #[test]
    fn different_text_gives_a_different_card() {
        let a = draw_card("a.co/x", BG, FG, 320, 180);
        let b = draw_card("a.co/y", BG, FG, 320, 180);
        assert_ne!(a.as_raw(), b.as_raw());
    }

    #[test]
    fn a_long_url_wraps_and_stays_inside_the_frame() {
        let url = "https://example.com/a/very/long/path/that/does/not/fit/on/one/line?ref=reel&x=1";
        let image = draw_card(url, BG, FG, 320, 568);
        assert_eq!(image.dimensions(), (320, 568));
        assert!(count(&image, FG) > 0);
        // Nothing is drawn in the outer 4 % margin at the left and right.
        for y in 0..568 {
            assert_eq!(*image.get_pixel(0, y), BG);
            assert_eq!(*image.get_pixel(319, y), BG);
        }
    }

    #[test]
    fn a_character_the_font_lacks_is_a_box_not_a_gap() {
        assert_eq!(glyph('\u{00e9}'), glyph('\u{4e2d}'));
        assert_ne!(glyph('\u{4e2d}'), glyph(' '));
        // Lowercase uses the capital's shape.
        assert_eq!(glyph('a'), glyph('A'));
    }
}
