//! Easing curves for the zoom moves the motion detector measures.
//!
//! A curve is `cubic-bezier(1/3, y1, 2/3, y2)`: with the time control points at
//! one third and two thirds the x coordinate of the Bézier is the parameter
//! itself, so progress is a cubic polynomial in normalised time,
//! `3(1-t)²t·y1 + 3(1-t)t²·y2 + t³`, which is linear in `y1` and `y2`. A direct
//! least-squares solve fits the two numbers; the move is far too short (12
//! frames at most) to tell four free control values apart.

use crate::motion::ScalePoint;

/// The fixed time control points of the curve.
pub(crate) const X1: f64 = 1.0 / 3.0;
pub(crate) const X2: f64 = 2.0 / 3.0;
/// `y1` and `y2` stay inside this range: overshoot is real, a number outside
/// it comes from noise.
pub(crate) const Y_MIN: f64 = -0.5;
pub(crate) const Y_MAX: f64 = 1.5;
/// Fewer samples than this (a one-frame snap) have no curve to fit.
pub(crate) const MIN_SAMPLES: usize = 3;
/// The two columns of the fit are the same column to this relative accuracy
/// when the samples sit at one time or only at the ends and one point.
const SINGULAR: f64 = 1e-9;
/// A track point this close to the start (as a share of the total change) is
/// still the start, and one this close to the total is already the end. Measured
/// on the planted clips the scale fit is good to about 0.001 of a scale, which
/// is 0.5% of a move of 0.18, so these sit at that noise: tighter would trim a
/// frame by chance, looser (2% was tried) cuts the slow tail of an ease-out and
/// stretches the time axis, which moved its second control value by 0.09.
const START_TOL: f64 = 0.005;
const END_TOL: f64 = 0.01;
/// A track whose total change is smaller than this has nothing to normalise by.
const MIN_CHANGE: f64 = 1e-6;
/// A fitted curve and how far the samples lie from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BezierFit {
    pub y1: f64,
    pub y2: f64,
    /// Root mean square of (sample - curve) in units of progress.
    pub rms: f64,
}

/// Progress of the curve at normalised time `t` in `0..=1`.
pub(crate) fn progress_at(y1: f64, y2: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    3.0 * u * u * t * y1 + 3.0 * u * t * t * y2 + t * t * t
}

/// Fits `y1` and `y2` to `(t, progress)` samples, `t` in `0..=1`. `None` for
/// fewer than [`MIN_SAMPLES`] samples, or samples that cannot tell the two
/// numbers apart (all at the same time, or only at the ends).
pub(crate) fn fit_bezier(samples: &[(f64, f64)]) -> Option<BezierFit> {
    if samples.len() < MIN_SAMPLES
        || samples
            .iter()
            .any(|&(t, p)| !t.is_finite() || !p.is_finite())
    {
        return None;
    }
    // Progress minus the t^3 term is a(t)*y1 + b(t)*y2: normal equations.
    let (mut saa, mut sab, mut sbb, mut sar, mut sbr) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for &(t, p) in samples {
        let u = 1.0 - t;
        let (a, b) = (3.0 * u * u * t, 3.0 * u * t * t);
        let r = p - t * t * t;
        saa += a * a;
        sab += a * b;
        sbb += b * b;
        sar += a * r;
        sbr += b * r;
    }
    let det = saa * sbb - sab * sab;
    if !det.is_finite() || det <= SINGULAR * saa * sbb {
        return None;
    }
    let y1 = ((sar * sbb - sbr * sab) / det).clamp(Y_MIN, Y_MAX);
    let y2 = ((sbr * saa - sar * sab) / det).clamp(Y_MIN, Y_MAX);
    let sum: f64 = samples
        .iter()
        .map(|&(t, p)| (p - progress_at(y1, y2, t)).powi(2))
        .sum();
    Some(BezierFit {
        y1,
        y2,
        rms: (sum / samples.len() as f64).sqrt(),
    })
}

/// A zoom move as one curve entry, before it is mapped to `edit_dna.json`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MoveCurve {
    /// The frame before the move (the anchor, or the last frame still within
    /// the tolerance of it), and the frame where the move is complete.
    pub start_ms: u64,
    pub end_ms: u64,
    /// Frames from `start_ms` to `end_ms`, both included.
    pub samples: usize,
    /// `[x1, y1, x2, y2]`, `None` when there are too few samples to fit.
    pub bezier: Option<[f64; 4]>,
    pub fit_rms: Option<f64>,
}

/// The curve of a move from its measured scale track: the anchor at scale 1
/// first, then one point per frame. `None` for a track with no change to
/// normalise by.
pub(crate) fn curve_from_track(track: &[ScalePoint]) -> Option<MoveCurve> {
    let anchor = track.first()?;
    let change = track.last()?.scale - anchor.scale;
    if track.len() < 2 || change.is_nan() || change.abs() < MIN_CHANGE {
        return None;
    }
    let progress: Vec<f64> = track
        .iter()
        .map(|p| (p.scale - anchor.scale) / change)
        .collect();
    // The move runs from the last frame still at the start to the first frame
    // that stays at the end; the frames the anchor leaves out of it (it is
    // taken a few frames early) and the frames the track holds after it are
    // not part of its duration.
    let first_moving = progress.iter().position(|p| p.abs() > START_TOL)?;
    let start = first_moving.saturating_sub(1);
    let last_open = progress.iter().rposition(|p| (p - 1.0).abs() > END_TOL)?;
    let end = (last_open + 1).min(track.len() - 1).max(start + 1);
    let (begin, finish) = (track.get(start)?, track.get(end)?);
    let samples = end - start + 1;
    let span = finish.time_ms.saturating_sub(begin.time_ms) as f64;
    let fit = if span > 0.0 {
        let points: Vec<(f64, f64)> = track[start..=end]
            .iter()
            .zip(&progress[start..=end])
            .map(|(p, share)| {
                (
                    p.time_ms.saturating_sub(begin.time_ms) as f64 / span,
                    *share,
                )
            })
            .collect();
        fit_bezier(&points)
    } else {
        None
    };
    Some(MoveCurve {
        start_ms: begin.time_ms,
        end_ms: finish.time_ms,
        samples,
        bezier: fit.map(|f| [X1, f.y1, X2, f.y2]),
        fit_rms: fit.map(|f| f.rms),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(t, progress)` for `n` points from 0 to 1 on the exact curve.
    fn on_curve(y1: f64, y2: f64, n: usize) -> Vec<(f64, f64)> {
        (0..n)
            .map(|i| {
                let t = i as f64 / (n - 1) as f64;
                (t, progress_at(y1, y2, t))
            })
            .collect()
    }

    fn assert_fit(samples: &[(f64, f64)], y1: f64, y2: f64, tol: f64) -> BezierFit {
        let fit = fit_bezier(samples).expect("a fit");
        assert!((fit.y1 - y1).abs() <= tol, "y1 {} vs {y1}", fit.y1);
        assert!((fit.y2 - y2).abs() <= tol, "y2 {} vs {y2}", fit.y2);
        fit
    }

    #[test]
    fn the_thirds_make_time_the_curve_parameter() {
        // y1 = 1/3, y2 = 2/3 is the straight line.
        for k in 0..=10 {
            let t = k as f64 / 10.0;
            assert!((progress_at(X1, X2, t) - t).abs() < 1e-12);
        }
        assert_eq!(progress_at(0.2, 0.9, 0.0), 0.0);
        assert!((progress_at(0.2, 0.9, 1.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_linear_track_gives_the_thirds() {
        let fit = assert_fit(&on_curve(X1, X2, 7), X1, X2, 1e-9);
        assert!(fit.rms < 1e-9);
    }

    #[test]
    fn an_ease_in_cubic_gives_its_control_points() {
        // t^3 is y1 = 0, y2 = 0.
        let t3: Vec<(f64, f64)> = (0..=12)
            .map(|i| {
                let t = i as f64 / 12.0;
                (t, t * t * t)
            })
            .collect();
        assert_fit(&t3, 0.0, 0.0, 0.02);
    }

    #[test]
    fn an_ease_out_and_a_smoothstep_give_their_control_points() {
        // 1 - (1-t)^3 is y1 = y2 = 1; smoothstep 3t^2 - 2t^3 is y1 = 0, y2 = 1.
        let out: Vec<(f64, f64)> = (0..=12)
            .map(|i| {
                let t = i as f64 / 12.0;
                (t, 1.0 - (1.0 - t).powi(3))
            })
            .collect();
        assert_fit(&out, 1.0, 1.0, 0.02);
        let smooth: Vec<(f64, f64)> = (0..=6)
            .map(|i| {
                let t = i as f64 / 6.0;
                (t, t * t * (3.0 - 2.0 * t))
            })
            .collect();
        assert_fit(&smooth, 0.0, 1.0, 0.02);
    }

    #[test]
    fn an_overshoot_gives_a_second_control_point_above_one() {
        let samples = on_curve(0.1, 1.4, 13);
        assert!(
            samples.iter().any(|(_, p)| *p > 1.05),
            "plants an overshoot"
        );
        let fit = assert_fit(&samples, 0.1, 1.4, 0.02);
        assert!(fit.y2 > 1.0);
    }

    #[test]
    fn a_noisy_track_still_fits_close() {
        let mut samples = on_curve(0.0, 0.3, 13);
        for (i, s) in samples.iter_mut().enumerate() {
            // +-0.01 of progress, alternating.
            s.1 += if i % 2 == 0 { 0.01 } else { -0.01 };
        }
        let fit = assert_fit(&samples, 0.0, 0.3, 0.1);
        assert!(fit.rms > 0.0 && fit.rms < 0.02, "rms {}", fit.rms);
    }

    #[test]
    fn fewer_than_three_samples_have_no_curve() {
        assert!(fit_bezier(&[]).is_none());
        assert!(fit_bezier(&[(0.0, 0.0)]).is_none());
        // Two samples that could be fitted exactly are still not a curve.
        assert!(fit_bezier(&[(0.3, 0.1), (0.7, 0.9)]).is_none());
        // Three are.
        assert!(fit_bezier(&[(0.2, 0.1), (0.5, 0.5), (0.8, 0.9)]).is_some());
    }

    #[test]
    fn samples_that_cannot_tell_the_two_numbers_apart_have_no_curve() {
        // The ends and one point between them: a one-dimensional family.
        assert!(fit_bezier(&[(0.0, 0.0), (0.5, 0.5), (1.0, 1.0)]).is_none());
        // All at one time.
        assert!(fit_bezier(&[(0.4, 0.2), (0.4, 0.3), (0.4, 0.25)]).is_none());
    }

    #[test]
    fn the_control_points_are_clamped() {
        // A step: progress jumps to 1 right away. The unclamped solution lies
        // far outside the range.
        let step: Vec<(f64, f64)> = (0..=8)
            .map(|i| (i as f64 / 8.0, if i == 0 { 0.0 } else { 1.0 }))
            .collect();
        let fit = fit_bezier(&step).expect("a fit");
        assert!(fit.y1 <= Y_MAX && fit.y1 >= Y_MIN, "y1 {}", fit.y1);
        assert!(fit.y2 <= Y_MAX && fit.y2 >= Y_MIN, "y2 {}", fit.y2);
        // The documented range, written out: docs/edit-dna.md and the schema state it.
        assert_eq!(fit.y1, 1.5, "the step wants more than the range allows");
        // The other way: a move that dips far below its start first.
        let dip: Vec<(f64, f64)> = (0..=8)
            .map(|i| (i as f64 / 8.0, if i == 8 { 1.0 } else { -2.0 }))
            .collect();
        let fit = fit_bezier(&dip).expect("a fit");
        assert_eq!(fit.y1, -0.5);
        assert!(fit.y2 >= Y_MIN && fit.y2 <= Y_MAX, "y2 {}", fit.y2);
    }

    #[test]
    fn the_rms_is_measured_against_the_clamped_curve() {
        let step: Vec<(f64, f64)> = (0..=8)
            .map(|i| (i as f64 / 8.0, if i == 0 { 0.0 } else { 1.0 }))
            .collect();
        let fit = fit_bezier(&step).expect("a fit");
        let by_hand = (step
            .iter()
            .map(|(t, p)| (p - progress_at(fit.y1, fit.y2, *t)).powi(2))
            .sum::<f64>()
            / step.len() as f64)
            .sqrt();
        assert!((fit.rms - by_hand).abs() < 1e-12);
        assert!(fit.rms > 0.05, "a step is a poor cubic: {}", fit.rms);
    }

    // ---- from a scale track ----

    fn point(time_ms: u64, scale: f64) -> ScalePoint {
        ScalePoint { time_ms, scale }
    }

    /// A track at 24 fps whose scale is `1 + total * progress(t)` over `frames`
    /// frames, then `hold` frames at the end. The anchor is at 1000 ms.
    fn planted(y1: f64, y2: f64, total: f64, frames: usize, hold: usize) -> Vec<ScalePoint> {
        let mut out = vec![point(1000, 1.0)];
        for k in 1..=frames + hold {
            let t = (k.min(frames)) as f64 / frames as f64;
            let ms = 1000 + (k as u64 * 1000 + 12) / 24;
            out.push(point(ms, 1.0 + total * progress_at(y1, y2, t)));
        }
        out
    }

    #[test]
    fn a_planted_track_gives_back_its_curve_and_times() {
        let track = planted(0.0, 0.3, 0.18, 12, 3);
        let curve = curve_from_track(&track).expect("curve");
        let [x1, y1, x2, y2] = curve.bezier.expect("a fit");
        assert!((x1 - X1).abs() < 1e-9 && (x2 - X2).abs() < 1e-9);
        assert!(
            (y1 - 0.0).abs() < 0.1 && (y2 - 0.3).abs() < 0.1,
            "{y1} {y2}"
        );
        assert!(curve.fit_rms.expect("rms") < 0.05);
        // A slow start leaves the first frame or two out of the move.
        assert!(
            (1000..=1084).contains(&curve.start_ms),
            "{}",
            curve.start_ms
        );
        assert!((1460..=1520).contains(&curve.end_ms), "{}", curve.end_ms);
        assert!(curve.samples >= 11, "{}", curve.samples);
    }

    #[test]
    fn progress_is_normalised_by_the_total_change() {
        // The same shape at a total of 0.09 and 0.36 gives the same control
        // points: they are shares of the move, not scale units.
        let small = curve_from_track(&planted(0.6, 1.0, 0.09, 12, 3)).expect("small");
        let large = curve_from_track(&planted(0.6, 1.0, 0.36, 12, 3)).expect("large");
        let (a, b) = (small.bezier.expect("a"), large.bezier.expect("b"));
        assert!(
            (a[1] - b[1]).abs() < 0.03 && (a[3] - b[3]).abs() < 0.03,
            "{a:?} {b:?}"
        );
        assert!(
            (a[1] - 0.6).abs() < 0.1 && (a[3] - 1.0).abs() < 0.1,
            "{a:?}"
        );
    }

    #[test]
    fn a_zoom_out_is_normalised_the_same_way() {
        // Scale falls from 1 to 0.85: change over total change is still 0..1.
        let track = planted(0.1, 1.4, -0.15, 12, 3);
        let curve = curve_from_track(&track).expect("curve");
        let [_, y1, _, y2] = curve.bezier.expect("a fit");
        assert!(
            (y1 - 0.1).abs() < 0.1 && (y2 - 1.4).abs() < 0.1,
            "{y1} {y2}"
        );
    }

    #[test]
    fn a_track_that_waits_before_the_move_starts_at_the_move() {
        // Three still frames after the anchor, then the move.
        let mut track = vec![point(1000, 1.0)];
        for k in 1..=3 {
            track.push(point(1000 + k * 42, 1.0));
        }
        let moved = planted(0.0, 1.0, 0.18, 6, 2);
        for (k, p) in moved.iter().enumerate().skip(1) {
            track.push(point(1125 + k as u64 * 42, p.scale));
        }
        let curve = curve_from_track(&track).expect("curve");
        assert!(curve.start_ms >= 1100, "starts at {}", curve.start_ms);
        let [_, y1, _, y2] = curve.bezier.expect("a fit");
        assert!(y1.abs() < 0.15 && (y2 - 1.0).abs() < 0.15, "{y1} {y2}");
    }

    #[test]
    fn a_one_frame_snap_keeps_its_times_and_has_no_curve() {
        let track = vec![point(1000, 1.0), point(1042, 1.18), point(1083, 1.18)];
        let curve = curve_from_track(&track).expect("an entry");
        assert_eq!((curve.start_ms, curve.end_ms), (1000, 1042));
        assert_eq!(curve.samples, 2);
        assert!(curve.bezier.is_none() && curve.fit_rms.is_none());
    }

    #[test]
    fn a_flat_track_does_not_divide_by_zero() {
        let flat = vec![point(1000, 1.0), point(1042, 1.0), point(1083, 1.0)];
        assert!(curve_from_track(&flat).is_none());
        let tiny = vec![point(1000, 1.0), point(1042, 1.0 + 1e-12)];
        assert!(curve_from_track(&tiny).is_none());
        assert!(curve_from_track(&[]).is_none());
        assert!(curve_from_track(&[point(1000, 1.0)]).is_none());
    }

    #[test]
    fn a_sample_that_is_not_a_number_has_no_curve() {
        let samples = [(0.0, 0.0), (0.25, 0.1), (0.5, f64::NAN), (1.0, 1.0)];
        assert!(fit_bezier(&samples).is_none());
        let infinite = [(0.0, 0.0), (0.5, f64::INFINITY), (0.75, 0.9), (1.0, 1.0)];
        assert!(fit_bezier(&infinite).is_none());
    }

    /// A track at 24 fps (42 ms steps from 1000 ms) whose scale is
    /// `1 + 0.2 * progress`, one point per entry.
    fn track_of(progress: &[f64]) -> Vec<ScalePoint> {
        progress
            .iter()
            .enumerate()
            .map(|(k, p)| point(1000 + k as u64 * 42, 1.0 + 0.2 * p))
            .collect()
    }

    fn span_of(progress: &[f64]) -> (u64, u64) {
        let curve = curve_from_track(&track_of(progress)).expect("an entry");
        (curve.start_ms, curve.end_ms)
    }

    #[test]
    fn the_trim_tolerances_sit_where_the_comments_say() {
        // 0.4% of the move is still the start (START_TOL is 0.5%), 0.8% has begun.
        assert_eq!(span_of(&[0.0, 0.004, 0.3, 0.7, 0.995, 1.0]), (1042, 1168));
        assert_eq!(span_of(&[0.0, 0.008, 0.3, 0.7, 0.995, 1.0]), (1000, 1168));
        // 0.5% short of the total is already the end (END_TOL is 1%), 1.5% is not.
        assert_eq!(span_of(&[0.0, 0.3, 0.7, 0.995, 1.0, 1.0]), (1000, 1126));
        assert_eq!(span_of(&[0.0, 0.3, 0.7, 0.985, 1.0, 1.0]), (1000, 1168));
    }
}
