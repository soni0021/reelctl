//! The curve that moves one channel's distribution onto another's.

use crate::histogram::{Histogram, LEVELS};

/// One output per input level, on a 0 to 1 scale: `curve[v]` is what level `v`
/// (0 to 255) becomes, divided by 255.
pub type Curve = [f64; LEVELS];

/// The curve that changes nothing.
pub fn identity_curve() -> Curve {
    let mut curve = [0.0; LEVELS];
    for (level, slot) in curve.iter_mut().enumerate() {
        *slot = level as f64 / 255.0;
    }
    curve
}

/// A populated level and where it sits in its histogram: the cumulative share of
/// pixels below it, plus half its own.
type Point = (usize, f64);

/// The populated levels of `histogram` with their mid-cumulative share, strictly
/// increasing in share. `None` when no pixel was counted.
fn points(histogram: &Histogram) -> Option<Vec<Point>> {
    let total: u64 = histogram.iter().sum();
    if total == 0 {
        return None;
    }
    let total = total as f64;
    let mut below = 0.0;
    let mut points = Vec::new();
    for (level, &count) in histogram.iter().enumerate() {
        if count > 0 {
            let count = count as f64;
            points.push((level, (below + count / 2.0) / total));
            below += count;
        }
    }
    Some(points)
}

/// The level (between populated ones, linearly) below which a share `share` of the
/// reference's pixels lie. Shares outside the first and last populated level's go
/// to those levels.
fn quantile(reference: &[Point], share: f64) -> f64 {
    let (Some(&(first_level, first_share)), Some(&(last_level, last_share))) =
        (reference.first(), reference.last())
    else {
        return share * 255.0;
    };
    if share <= first_share {
        return first_level as f64;
    }
    if share >= last_share {
        return last_level as f64;
    }
    let above = reference.partition_point(|&(_, at)| at <= share);
    match (reference.get(above.wrapping_sub(1)), reference.get(above)) {
        (Some(&(low, low_share)), Some(&(high, high_share))) if high_share > low_share => {
            let t = (share - low_share) / (high_share - low_share);
            low as f64 + t * (high as f64 - low as f64)
        }
        _ => first_level as f64,
    }
}

/// The curve that gives a channel with histogram `source` the distribution of one with
/// histogram `reference`.
///
/// Each populated source level goes to the reference level at the same cumulative
/// share (mid-bin, so a level that holds half the pixels lands in the middle of the
/// reference's range, not at its edge), reading between the reference's populated
/// levels linearly. Source levels between populated ones are interpolated between
/// their neighbours' results, and those beyond the first and last populated level
/// carry on with a slope of 1 from the nearest result, so two equal histograms give
/// exactly the identity, and a brightness offset gives exactly that offset. The curve
/// never decreases and stays in 0 to 1. If either histogram is empty there is nothing
/// to match and the curve is the identity.
pub fn match_curve(source: &Histogram, reference: &Histogram) -> Curve {
    let (Some(source_points), Some(reference_points)) = (points(source), points(reference)) else {
        return identity_curve();
    };
    let knots: Vec<(usize, f64)> = source_points
        .iter()
        .map(|&(level, share)| (level, quantile(&reference_points, share)))
        .collect();
    let (Some(&(first_level, first_out)), Some(&(last_level, last_out))) =
        (knots.first(), knots.last())
    else {
        return identity_curve();
    };

    let mut curve = [0.0; LEVELS];
    // The pair of knots around the level, advanced as the level rises.
    let mut upper = 0;
    for (level, slot) in curve.iter_mut().enumerate() {
        let out = if level <= first_level {
            first_out - (first_level - level) as f64
        } else if level >= last_level {
            last_out + (level - last_level) as f64
        } else {
            while knots.get(upper).is_some_and(|&(at, _)| at < level) {
                upper += 1;
            }
            match (knots.get(upper.wrapping_sub(1)), knots.get(upper)) {
                (Some(&(low, low_out)), Some(&(high, high_out))) if high > low => {
                    low_out + (level - low) as f64 / (high - low) as f64 * (high_out - low_out)
                }
                _ => level as f64,
            }
        };
        *slot = (out / 255.0).clamp(0.0, 1.0);
    }
    curve
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(v: usize) -> f64 {
        v as f64 / 255.0
    }

    fn uniform(lo: usize, hi: usize) -> Histogram {
        let mut h = [0; LEVELS];
        for slot in &mut h[lo..=hi] {
            *slot = 10;
        }
        h
    }

    /// A deterministic pseudo-random histogram with `gaps` empty levels.
    fn random_hist(seed: u64, gaps: bool) -> Histogram {
        let mut state = seed;
        let mut h = [0u64; LEVELS];
        for slot in &mut h {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let draw = (state >> 33) % 100;
            *slot = if gaps && draw < 35 { 0 } else { draw };
        }
        h
    }

    fn cdf(h: &Histogram) -> Vec<f64> {
        let total: u64 = h.iter().sum();
        let mut running = 0u64;
        h.iter()
            .map(|&count| {
                running += count;
                running as f64 / total as f64
            })
            .collect()
    }

    /// The histogram of `source` after every level `v` goes to round(curve[v]*255).
    fn mapped(source: &Histogram, curve: &Curve) -> Histogram {
        let mut out = [0u64; LEVELS];
        for (v, &count) in source.iter().enumerate() {
            let to = (curve[v] * 255.0).round() as usize;
            out[to] += count;
        }
        out
    }

    fn ks(a: &Histogram, b: &Histogram) -> f64 {
        cdf(a)
            .iter()
            .zip(cdf(b))
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f64::max)
    }

    #[test]
    fn the_identity_curve_maps_every_level_to_itself() {
        let curve = identity_curve();
        for (v, &out) in curve.iter().enumerate() {
            assert!((out - level(v)).abs() < 1e-12);
        }
    }

    #[test]
    fn equal_distributions_give_the_identity() {
        for h in [
            uniform(0, 255),
            uniform(40, 200),
            random_hist(1, false),
            random_hist(2, true),
        ] {
            let curve = match_curve(&h, &h);
            for (v, &out) in curve.iter().enumerate() {
                assert!(
                    (out - level(v)).abs() < 1e-9,
                    "level {v} went to {}",
                    out * 255.0
                );
            }
        }
    }

    #[test]
    fn a_brightness_offset_is_that_offset() {
        let mut source = random_hist(7, true);
        for slot in &mut source[LEVELS - 30..] {
            *slot = 0;
        }
        let mut reference = [0u64; LEVELS];
        reference[30..].copy_from_slice(&source[..LEVELS - 30]);
        let curve = match_curve(&source, &reference);
        for v in 0..LEVELS - 30 {
            if source[v] > 0 {
                assert!(
                    (curve[v] * 255.0 - (v as f64 + 30.0)).abs() < 1e-6,
                    "level {v} went to {}",
                    curve[v] * 255.0
                );
            }
        }
        // Outside the source's range the shift carries on, up to white.
        assert!((curve[LEVELS - 1] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_narrow_distribution_stretches_to_the_references_range() {
        let curve = match_curve(&uniform(100, 120), &uniform(0, 255));
        assert!(curve[100] < 0.06, "{}", curve[100]);
        assert!(curve[120] > 0.94, "{}", curve[120]);
        assert!((curve[110] - 0.5).abs() < 0.03, "{}", curve[110]);
        // And the other way: a wide one is squeezed.
        let squeeze = match_curve(&uniform(0, 255), &uniform(100, 120));
        assert!(squeeze[0] * 255.0 >= 100.0 - 1e-9);
        assert!(squeeze[255] * 255.0 <= 120.0 + 1e-9);
    }

    #[test]
    fn the_matched_source_has_the_references_distribution() {
        for (a, b) in [(11, 12), (13, 14), (15, 16)] {
            let source = random_hist(a, false);
            let reference = random_hist(b, false);
            let curve = match_curve(&source, &reference);
            let distance_after = ks(&mapped(&source, &curve), &reference);
            let distance_before = ks(&source, &reference);
            assert!(
                distance_after < 0.05 && distance_after < distance_before / 2.0,
                "before {distance_before}, after {distance_after}"
            );
        }
    }

    #[test]
    fn the_curve_is_monotonic_and_inside_zero_to_one() {
        for seed in 0..40u64 {
            let source = random_hist(seed, seed % 2 == 0);
            let reference = random_hist(seed + 1000, seed % 3 == 0);
            let curve = match_curve(&source, &reference);
            assert!(curve.iter().all(|v| (0.0..=1.0).contains(v)), "seed {seed}");
            assert!(
                curve.windows(2).all(|pair| pair[0] <= pair[1]),
                "seed {seed} is not monotonic"
            );
        }
    }

    #[test]
    fn an_empty_histogram_on_either_side_gives_the_identity() {
        let some = random_hist(3, false);
        let none = [0u64; LEVELS];
        for curve in [
            match_curve(&none, &some),
            match_curve(&some, &none),
            match_curve(&none, &none),
        ] {
            for (v, &out) in curve.iter().enumerate() {
                assert!((out - level(v)).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn a_single_valued_channel_does_not_divide_by_zero() {
        let mut flat = [0u64; LEVELS];
        flat[100] = 5_000;
        let wide = uniform(0, 255);
        let mut other = [0u64; LEVELS];
        other[50] = 7;
        for (source, reference) in [
            (&flat, &wide),
            (&wide, &flat),
            (&flat, &other),
            (&flat, &flat),
        ] {
            let curve = match_curve(source, reference);
            assert!(curve
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
            assert!(curve.windows(2).all(|pair| pair[0] <= pair[1]));
        }
        // A flat source goes to the middle of the reference, a flat reference
        // takes every source level that has pixels.
        assert!((match_curve(&flat, &wide)[100] - 0.5).abs() < 0.02);
        assert!((match_curve(&wide, &flat)[200] * 255.0 - 100.0).abs() < 1e-9);
        assert!((match_curve(&flat, &other)[100] * 255.0 - 50.0).abs() < 1e-9);
        // Beyond the source's one level the shift is carried on.
        assert!((match_curve(&flat, &other)[110] * 255.0 - 60.0).abs() < 1e-9);
        assert!((match_curve(&flat, &other)[90] * 255.0 - 40.0).abs() < 1e-9);
    }

    #[test]
    fn a_share_between_two_reference_levels_lands_between_them() {
        // A third of the pixels at each of 0, 100 and 200 (shares 1/6, 1/2, 5/6)
        // onto a reference of two levels, 10 and 200 (shares 1/4 and 3/4): the
        // outer ones are clamped to the reference's range, the middle one is halfway.
        let mut source = [0u64; LEVELS];
        let mut reference = [0u64; LEVELS];
        for v in [0, 100, 200] {
            source[v] = 1;
        }
        for v in [10, 200] {
            reference[v] = 1;
        }
        let curve = match_curve(&source, &reference);
        assert!(
            (curve[0] * 255.0 - 10.0).abs() < 1e-6,
            "{}",
            curve[0] * 255.0
        );
        assert!(
            (curve[100] * 255.0 - 105.0).abs() < 1e-6,
            "{}",
            curve[100] * 255.0
        );
        assert!(
            (curve[200] * 255.0 - 200.0).abs() < 1e-6,
            "{}",
            curve[200] * 255.0
        );
        // Two source levels holding 3/8 and 5/8 of the pixels have shares 3/16 and
        // 11/16: the first is below the reference's first share, the second is
        // (11/16 - 1/4) / (1/2) of the way from 10 to 200.
        let mut source = [0u64; LEVELS];
        source[50] = 3;
        source[150] = 5;
        let curve = match_curve(&source, &reference);
        assert!(
            (curve[50] * 255.0 - 10.0).abs() < 1e-6,
            "{}",
            curve[50] * 255.0
        );
        assert!(
            (curve[150] * 255.0 - (10.0 + (11.0 / 16.0 - 0.25) / 0.5 * 190.0)).abs() < 1e-6,
            "{}",
            curve[150] * 255.0
        );
    }

    #[test]
    fn levels_between_populated_ones_are_interpolated() {
        // Source at 0, 100, 200 (a third each) onto a reference at 10, 60, 250.
        let mut source = [0u64; LEVELS];
        let mut reference = [0u64; LEVELS];
        for (v, w) in [(0, 10), (100, 60), (200, 250)] {
            source[v] = 1;
            reference[w] = 1;
        }
        let curve = match_curve(&source, &reference);
        assert!((curve[0] * 255.0 - 10.0).abs() < 1e-6);
        assert!((curve[100] * 255.0 - 60.0).abs() < 1e-6);
        assert!((curve[200] * 255.0 - 250.0).abs() < 1e-6);
        assert!((curve[50] * 255.0 - 35.0).abs() < 1e-6);
        assert!((curve[150] * 255.0 - 155.0).abs() < 1e-6);
    }
}
