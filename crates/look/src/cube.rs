//! An n^3 colour cube built from three per-channel curves, and the Adobe/Resolve
//! `.cube` text format.

use crate::curve::Curve;
use crate::error::LookError;
use crate::histogram::{Histograms, LEVELS};

/// The cube edges `reelctl look` offers.
pub const CUBE_SIZES: [usize; 3] = [17, 33, 65];

/// A 3D LUT over the unit RGB cube. Entries are in `.cube` order: the red index
/// varies fastest, then green, then blue.
#[derive(Clone, Debug, PartialEq)]
pub struct Cube {
    title: String,
    size: usize,
    entries: Vec<[f64; 3]>,
}

/// A value inside the domain 0 to 1, with NaN and negative zero as plain zero.
fn within_domain(value: f64) -> f64 {
    if value.is_nan() || value <= 0.0 {
        0.0
    } else if value >= 1.0 {
        1.0
    } else {
        value
    }
}

/// The curve at `x` (0 to 1), between its 256 levels linearly.
fn curve_at(curve: &Curve, x: f64) -> f64 {
    let position = within_domain(x) * (LEVELS - 1) as f64;
    let low = (position.floor() as usize).min(LEVELS - 1);
    let high = (low + 1).min(LEVELS - 1);
    let at = |index: usize| curve.get(index).copied().unwrap_or(0.0);
    let t = position - low as f64;
    at(low) + t * (at(high) - at(low))
}

/// A title that fits on one line inside quotes.
fn one_line(title: &str) -> String {
    title
        .chars()
        .map(|c| match c {
            '"' => '\'',
            '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

impl Cube {
    /// The cube whose node `(r, g, b)` holds `(curves[0](r), curves[1](g), curves[2](b))`,
    /// each curve read between its 256 levels by linear interpolation. Values are
    /// kept inside 0 to 1.
    pub fn from_curves(title: &str, size: usize, curves: &[Curve; 3]) -> Result<Self, LookError> {
        if size < 2 {
            return Err(LookError::Size(size));
        }
        let [red, green, blue] = curves;
        let last = (size - 1) as f64;
        // One value per index along an edge, for each channel.
        let along = |curve: &Curve| -> Vec<f64> {
            (0..size)
                .map(|i| within_domain(curve_at(curve, i as f64 / last)))
                .collect()
        };
        let (reds, greens, blues) = (along(red), along(green), along(blue));
        let mut entries = Vec::with_capacity(size * size * size);
        // Blue is the slowest index and red the fastest, as `.cube` wants.
        for &b in &blues {
            for &g in &greens {
                for &r in &reds {
                    entries.push([r, g, b]);
                }
            }
        }
        Ok(Self {
            title: title.to_string(),
            size,
            entries,
        })
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    /// Nodes along each edge.
    pub fn size(&self) -> usize {
        self.size
    }

    /// All `size^3` nodes in `.cube` order.
    pub fn entries(&self) -> &[[f64; 3]] {
        &self.entries
    }

    /// The node at red, green and blue indices `r`, `g`, `b`.
    pub fn entry(&self, r: usize, g: usize, b: usize) -> Option<[f64; 3]> {
        if r >= self.size || g >= self.size || b >= self.size {
            return None;
        }
        self.entries
            .get(r + self.size * (g + self.size * b))
            .copied()
    }

    /// The colour a pixel `rgb` (each 0 to 1) becomes: trilinear interpolation
    /// between the eight nodes around it, as `ffmpeg -vf lut3d` does by default.
    /// Inputs outside 0 to 1 are clamped.
    pub fn apply(&self, rgb: [f64; 3]) -> [f64; 3] {
        let last = self.size.saturating_sub(1);
        // For each channel: the lower node, the upper node, and the weight of the upper.
        let axis = |x: f64| -> (usize, usize, f64) {
            let position = within_domain(x) * last as f64;
            let low = (position.floor() as usize).min(last);
            (low, (low + 1).min(last), position - low as f64)
        };
        let [(r0, r1, rt), (g0, g1, gt), (b0, b1, bt)] = rgb.map(axis);
        let node = |r, g, b| self.entry(r, g, b).unwrap_or([0.0; 3]);
        let mut out = [0.0; 3];
        for (bi, bw) in [(b0, 1.0 - bt), (b1, bt)] {
            for (gi, gw) in [(g0, 1.0 - gt), (g1, gt)] {
                for (ri, rw) in [(r0, 1.0 - rt), (r1, rt)] {
                    let weight = rw * gw * bw;
                    for (slot, value) in out.iter_mut().zip(node(ri, gi, bi)) {
                        *slot += weight * value;
                    }
                }
            }
        }
        out.map(within_domain)
    }

    /// How much the cube moves the mean of each channel, in levels on a 0 to 255
    /// scale, for pixels with these histograms. The cube is read along the grey
    /// diagonal, which is exact for a cube from [`Cube::from_curves`] (each output
    /// depends on one input) and not for a cube that mixes channels. `None` when no
    /// pixel was counted.
    pub fn mean_shift(&self, histograms: &Histograms) -> Option<[f64; 3]> {
        let total = histograms.pixels();
        if total == 0 {
            return None;
        }
        let mut shift = [0.0; 3];
        for (channel, (slot, histogram)) in shift.iter_mut().zip(&histograms.channels).enumerate() {
            for (level, &count) in histogram.iter().enumerate() {
                if count > 0 {
                    let x = level as f64 / 255.0;
                    let out = self.apply([x, x, x]).get(channel).copied().unwrap_or(x);
                    *slot += count as f64 * (out * 255.0 - level as f64);
                }
            }
            *slot /= total as f64;
        }
        Some(shift)
    }

    /// The `.cube` file: `TITLE`, `LUT_3D_SIZE`, `DOMAIN_MIN`, `DOMAIN_MAX`, then one
    /// line of three floats with 6 decimals per node, red fastest.
    pub fn to_cube_string(&self) -> String {
        let mut text = String::with_capacity(64 + self.entries.len() * 27);
        text.push_str(&format!("TITLE \"{}\"\n", one_line(&self.title)));
        text.push_str(&format!("LUT_3D_SIZE {}\n", self.size));
        text.push_str("DOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n");
        for [r, g, b] in &self.entries {
            text.push_str(&format!(
                "{:.6} {:.6} {:.6}\n",
                within_domain(*r),
                within_domain(*g),
                within_domain(*b)
            ));
        }
        text
    }

    /// Read a `.cube` file as this crate writes it: a 3D LUT over the domain 0 to 1.
    /// Comments (`#`) and blank lines are skipped.
    pub fn parse(text: &str) -> Result<Self, LookError> {
        let mut title = String::new();
        let mut size: Option<usize> = None;
        let mut entries: Vec<[f64; 3]> = Vec::new();
        let fail = |line: usize, reason: &str| LookError::Parse {
            line,
            reason: reason.to_string(),
        };
        for (index, raw) in text.lines().enumerate() {
            let number = index + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut words = line.split_whitespace();
            let Some(first) = words.next() else {
                continue;
            };
            let rest: Vec<&str> = words.collect();
            let floats = |words: &[&str]| -> Option<Vec<f64>> {
                words
                    .iter()
                    .map(|word| word.parse::<f64>().ok().filter(|v| v.is_finite()))
                    .collect()
            };
            match first {
                "TITLE" => {
                    let value = line.strip_prefix("TITLE").unwrap_or(line).trim();
                    title = value
                        .strip_prefix('"')
                        .and_then(|inner| inner.strip_suffix('"'))
                        .unwrap_or(value)
                        .to_string();
                }
                "LUT_3D_SIZE" => {
                    let n = match rest.as_slice() {
                        [one] => one.parse::<usize>().ok().filter(|&n| n >= 2),
                        _ => None,
                    };
                    size =
                        Some(n.ok_or_else(|| {
                            fail(number, "LUT_3D_SIZE needs a size of 2 or more")
                        })?);
                }
                "LUT_1D_SIZE" => return Err(fail(number, "a 1D LUT is not supported")),
                "DOMAIN_MIN" | "DOMAIN_MAX" => {
                    let want = if first == "DOMAIN_MIN" { 0.0 } else { 1.0 };
                    if floats(&rest).as_deref() != Some(&[want; 3]) {
                        return Err(fail(number, "only the domain 0 to 1 is supported"));
                    }
                }
                _ => match floats(std::slice::from_ref(&first)) {
                    Some(_) => {
                        if size.is_none() {
                            return Err(fail(number, "a value before LUT_3D_SIZE"));
                        }
                        let all: Vec<&str> = std::iter::once(first).chain(rest).collect();
                        match floats(&all).as_deref() {
                            Some(&[r, g, b]) => entries.push([r, g, b]),
                            _ => return Err(fail(number, "a node needs three numbers")),
                        }
                    }
                    None => return Err(fail(number, "unknown keyword")),
                },
            }
        }
        let size = size.ok_or_else(|| fail(0, "no LUT_3D_SIZE"))?;
        if entries.len() != size * size * size {
            return Err(fail(
                0,
                &format!(
                    "LUT_3D_SIZE {size} needs {} nodes, found {}",
                    size * size * size,
                    entries.len()
                ),
            ));
        }
        Ok(Self {
            title,
            size,
            entries,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::identity_curve;

    /// out = x * gain, per channel.
    fn gain_curve(gain: f64) -> Curve {
        let mut curve = [0.0; LEVELS];
        for (v, slot) in curve.iter_mut().enumerate() {
            *slot = v as f64 / 255.0 * gain;
        }
        curve
    }

    fn linear_cube(size: usize) -> Cube {
        Cube::from_curves(
            "t",
            size,
            &[gain_curve(1.0), gain_curve(0.5), gain_curve(0.25)],
        )
        .expect("cube")
    }

    #[test]
    fn the_file_has_the_header_and_one_line_per_node() {
        for size in CUBE_SIZES {
            let text = linear_cube(size).to_cube_string();
            let lines: Vec<&str> = text.lines().collect();
            assert_eq!(lines[0], "TITLE \"t\"");
            assert_eq!(lines[1], format!("LUT_3D_SIZE {size}"));
            assert_eq!(lines[2], "DOMAIN_MIN 0 0 0");
            assert_eq!(lines[3], "DOMAIN_MAX 1 1 1");
            assert_eq!(lines.len(), 4 + size * size * size, "size {size}");
            assert!(text.ends_with('\n'));
        }
    }

    #[test]
    fn red_varies_fastest_then_green_then_blue() {
        let n = 17;
        let text = linear_cube(n).to_cube_string();
        let data: Vec<Vec<f64>> = text
            .lines()
            .skip(4)
            .map(|line| line.split(' ').map(|v| v.parse().expect("float")).collect())
            .collect();
        let step = 1.0 / (n - 1) as f64;
        // The first node is black; the next moves red only.
        assert_eq!(data[0], [0.0, 0.0, 0.0]);
        assert!((data[1][0] - step).abs() < 1e-6 && data[1][1] == 0.0 && data[1][2] == 0.0);
        // After n nodes, green steps (this cube halves green) and red is back to 0.
        assert!(data[n][0] == 0.0 && (data[n][1] - step * 0.5).abs() < 1e-6 && data[n][2] == 0.0);
        // After n^2 nodes, blue steps (quartered).
        assert!(
            data[n * n][0] == 0.0
                && data[n * n][1] == 0.0
                && (data[n * n][2] - step * 0.25).abs() < 1e-6
        );
        // The last node is white through the three gains.
        assert!((data[n * n * n - 1][0] - 1.0).abs() < 1e-6);
        assert!((data[n * n * n - 1][1] - 0.5).abs() < 1e-6);
        assert!((data[n * n * n - 1][2] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn entries_are_found_by_red_green_blue_index() {
        let cube = linear_cube(33);
        assert_eq!(cube.size(), 33);
        assert_eq!(cube.entries().len(), 33 * 33 * 33);
        let e = cube.entry(32, 16, 8).expect("node");
        assert!((e[0] - 1.0).abs() < 1e-12);
        assert!((e[1] - 0.25).abs() < 1e-12);
        assert!((e[2] - 0.25 * 0.25).abs() < 1e-12);
        assert_eq!(cube.entry(33, 0, 0), None);
        assert_eq!(cube.entry(0, 33, 0), None);
        assert_eq!(cube.entry(0, 0, 33), None);
        assert_eq!(
            cube.entries()[5 + 33 * (7 + 33 * 9)],
            cube.entry(5, 7, 9).expect("node")
        );
    }

    #[test]
    fn floats_have_six_decimals_and_stay_inside_the_domain() {
        // Curves that leave 0 to 1 are clamped.
        let cube = Cube::from_curves(
            "t",
            17,
            &[gain_curve(2.0), gain_curve(-1.0), identity_curve()],
        )
        .expect("cube");
        let text = cube.to_cube_string();
        for line in text.lines().skip(4) {
            for value in line.split(' ') {
                let (whole, fraction) = value.split_once('.').expect("a decimal point");
                assert!(whole == "0" || whole == "1", "{value}");
                assert_eq!(fraction.len(), 6, "{value}");
            }
        }
        assert!(!text.contains('-'), "a negative zero or value was written");
        assert!(cube
            .entries()
            .iter()
            .flatten()
            .all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn a_written_cube_reads_back() {
        let cube = linear_cube(17);
        let back = Cube::parse(&cube.to_cube_string()).expect("parse");
        assert_eq!(back.title(), "t");
        assert_eq!(back.size(), 17);
        assert_eq!(back.entries().len(), cube.entries().len());
        for (a, b) in back.entries().iter().zip(cube.entries()) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 5e-7);
            }
        }
        // And what was read writes the same bytes.
        assert_eq!(back.to_cube_string(), cube.to_cube_string());
    }

    #[test]
    fn the_title_cannot_break_the_file() {
        let cube = Cube::from_curves("a \"b\"\nTITLE x", 17, &[gain_curve(1.0); 3]).expect("cube");
        let text = cube.to_cube_string();
        assert_eq!(text.lines().filter(|l| l.starts_with("TITLE")).count(), 1);
        assert_eq!(Cube::parse(&text).expect("parse").size(), 17);
    }

    #[test]
    fn a_size_below_two_is_refused() {
        for size in [0, 1] {
            assert_eq!(
                Cube::from_curves("t", size, &[gain_curve(1.0); 3]),
                Err(LookError::Size(size))
            );
        }
        assert!(Cube::from_curves("t", 2, &[gain_curve(1.0); 3]).is_ok());
    }

    #[test]
    fn the_reader_refuses_what_it_cannot_use() {
        let good = linear_cube(17).to_cube_string();
        // A line short.
        let short: String = good
            .lines()
            .take(good.lines().count() - 1)
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(Cube::parse(&short).is_err());
        // No size.
        let no_size: String = good
            .lines()
            .filter(|l| !l.starts_with("LUT_3D_SIZE"))
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(Cube::parse(&no_size).is_err());
        // A 1D LUT, another domain, and a value that is not a number.
        assert!(Cube::parse("LUT_1D_SIZE 4\n0 0 0\n").is_err());
        assert!(Cube::parse(&good.replace("DOMAIN_MAX 1 1 1", "DOMAIN_MAX 2 2 2")).is_err());
        assert!(Cube::parse(&good.replacen("0.000000", "x", 1)).is_err());
        assert!(Cube::parse("").is_err());
        // A line with two values.
        assert!(Cube::parse("LUT_3D_SIZE 2\n0 0\n").is_err());
    }

    #[test]
    fn the_reader_skips_comments_blank_lines_and_a_bare_title() {
        let mut text = String::from("# made by hand\n\nTITLE plain\nLUT_3D_SIZE 2\n\n");
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    text.push_str(&format!("{r}.0 {g}.0 {b}.0\n"));
                }
            }
        }
        let cube = Cube::parse(&text).expect("parse");
        assert_eq!(cube.title(), "plain");
        assert_eq!(cube.entry(1, 0, 1), Some([1.0, 0.0, 1.0]));
        assert_eq!(cube.entry(0, 1, 0), Some([0.0, 1.0, 0.0]));
    }

    #[test]
    fn apply_is_trilinear_and_exact_on_nodes() {
        let cube = linear_cube(17);
        // On a node.
        let on = cube.apply([4.0 / 16.0, 8.0 / 16.0, 16.0 / 16.0]);
        assert!(
            (on[0] - 0.25).abs() < 1e-12
                && (on[1] - 0.25).abs() < 1e-12
                && (on[2] - 0.25).abs() < 1e-12
        );
        // Between nodes, a linear cube is linear.
        let mid = cube.apply([0.3, 0.7, 0.9]);
        assert!((mid[0] - 0.3).abs() < 1e-9);
        assert!((mid[1] - 0.35).abs() < 1e-9);
        assert!((mid[2] - 0.225).abs() < 1e-9);
        // Outside the domain is clamped, not extrapolated.
        let out = cube.apply([-1.0, 2.0, f64::NAN]);
        assert_eq!(out[0], 0.0);
        assert!((out[1] - 0.5).abs() < 1e-12);
        assert!(out[2].is_finite());
    }

    #[test]
    fn a_nonlinear_curve_is_followed_between_nodes_of_a_small_cube() {
        // out = x^2: a 17-node cube is piecewise linear in it.
        let mut curve = [0.0; LEVELS];
        for (v, slot) in curve.iter_mut().enumerate() {
            let x = v as f64 / 255.0;
            *slot = x * x;
        }
        let cube = Cube::from_curves("t", 17, &[curve; 3]).expect("cube");
        for k in 0..=100 {
            let x = k as f64 / 100.0;
            let y = cube.apply([x, x, x]);
            assert!((y[0] - x * x).abs() < 0.002, "x {x} gave {}", y[0]);
        }
    }

    #[test]
    fn the_mean_shift_is_the_change_in_each_channels_mean() {
        // Half the pixels at 100, half at 200, in every channel; the cube halves G
        // and quarters B (levels 50, 100 and 25, 50).
        let pixels: Vec<u8> = [100u8, 100, 100, 200, 200, 200].to_vec();
        let histograms = Histograms::from_rgb(&pixels);
        let shift = linear_cube(33).mean_shift(&histograms).expect("shift");
        assert!(shift[0].abs() < 0.5, "{shift:?}");
        assert!((shift[1] - (-75.0)).abs() < 0.5, "{shift:?}");
        assert!((shift[2] - (-112.5)).abs() < 0.5, "{shift:?}");
        assert_eq!(linear_cube(33).mean_shift(&Histograms::default()), None);
    }

    #[test]
    fn the_same_inputs_write_the_same_bytes() {
        assert_eq!(
            linear_cube(33).to_cube_string(),
            linear_cube(33).to_cube_string()
        );
    }
}
