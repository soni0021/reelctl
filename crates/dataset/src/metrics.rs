//! The metrics, defined in `docs/specs/p3-dataset-eval.md` ("Matching, ties, degenerate
//! cases"). Every function is pure. None returns NaN: a score that is undefined (no
//! truth, no answer, no pairs) is `None`, and a NaN or infinite input is an error.

use serde::Serialize;

use crate::error::MetricError;

/// `value` if it is a finite number, else [`MetricError::NotFinite`] naming `what`.
pub fn finite(what: &'static str, value: f64) -> Result<f64, MetricError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(MetricError::NotFinite { what, value })
    }
}

/// `numerator / denominator`; `None` when the denominator is 0; an error when the
/// numerator is larger (that is not a share).
pub fn ratio(numerator: u64, denominator: u64) -> Result<Option<f64>, MetricError> {
    if numerator > denominator {
        return Err(MetricError::NotAShare {
            numerator,
            denominator,
        });
    }
    if denominator == 0 {
        return Ok(None);
    }
    Ok(Some(numerator as f64 / denominator as f64))
}

/// True positives, false positives and false negatives. Totals are added over
/// records (micro-averaging) before any rate is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Counts {
    pub tp: u64,
    pub fp: u64,
    pub fn_: u64,
}

impl Counts {
    /// `pairs` matches out of `truth` truth items and `predicted` answer items.
    pub fn from_matches(pairs: usize, truth: usize, predicted: usize) -> Self {
        Self {
            tp: pairs as u64,
            fp: (predicted - pairs) as u64,
            fn_: (truth - pairs) as u64,
        }
    }

    pub fn add(&mut self, other: Counts) {
        self.tp += other.tp;
        self.fp += other.fp;
        self.fn_ += other.fn_;
    }

    /// `tp / (tp + fp)`, or `None` when the answer holds nothing.
    pub fn precision(&self) -> Option<f64> {
        let denominator = self.tp + self.fp;
        (denominator > 0).then(|| self.tp as f64 / denominator as f64)
    }

    /// `tp / (tp + fn)`, or `None` when the truth holds nothing.
    pub fn recall(&self) -> Option<f64> {
        let denominator = self.tp + self.fn_;
        (denominator > 0).then(|| self.tp as f64 / denominator as f64)
    }

    /// `2tp / (2tp + fp + fn)`, or `None` when there is neither truth nor answer.
    pub fn f1(&self) -> Option<f64> {
        let denominator = 2 * self.tp + self.fp + self.fn_;
        (denominator > 0).then(|| (2 * self.tp) as f64 / denominator as f64)
    }

    pub fn scores(&self) -> Prf {
        Prf {
            tp: self.tp,
            fp: self.fp,
            fn_: self.fn_,
            precision: self.precision(),
            recall: self.recall(),
            f1: self.f1(),
        }
    }
}

/// Counts and the rates taken from them, as reported.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Prf {
    pub tp: u64,
    pub fp: u64,
    #[serde(rename = "fn")]
    pub fn_: u64,
    pub precision: Option<f64>,
    pub recall: Option<f64>,
    pub f1: Option<f64>,
}

/// One accepted pair of a truth item and an answer item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimePair {
    pub truth: usize,
    pub predicted: usize,
    /// `|truth time - answer time|` in milliseconds.
    pub distance_ms: u64,
}

/// One-to-one matching of timed items: a candidate pair has equal labels and times at
/// most `tolerance_ms` apart (inclusive). Candidates are taken in order of distance,
/// then truth index, then answer index, and a pair is accepted when neither side is
/// used. The pairs come back in acceptance order.
pub fn match_times<K: PartialEq>(
    truth: &[(u64, K)],
    predicted: &[(u64, K)],
    tolerance_ms: u64,
) -> Vec<TimePair> {
    let mut candidates = Vec::new();
    for (i, (truth_time, truth_label)) in truth.iter().enumerate() {
        for (j, (predicted_time, predicted_label)) in predicted.iter().enumerate() {
            let distance_ms = truth_time.abs_diff(*predicted_time);
            if distance_ms <= tolerance_ms && truth_label == predicted_label {
                candidates.push(TimePair {
                    truth: i,
                    predicted: j,
                    distance_ms,
                });
            }
        }
    }
    candidates.sort_by_key(|pair| (pair.distance_ms, pair.truth, pair.predicted));
    accept_unused(candidates, truth.len(), predicted.len(), |pair| {
        (pair.truth, pair.predicted)
    })
}

/// Intersection over union of the half-open intervals `[start, end)`; `None` when an
/// interval ends before it starts or the union is empty.
pub fn iou(a: (u64, u64), b: (u64, u64)) -> Option<f64> {
    if a.1 < a.0 || b.1 < b.0 {
        return None;
    }
    let start = a.0.max(b.0);
    let end = a.1.min(b.1);
    let intersection = end.saturating_sub(start);
    let union = (a.1 - a.0) + (b.1 - b.0) - intersection;
    if union == 0 {
        return None;
    }
    Some(intersection as f64 / union as f64)
}

/// One accepted pair of intervals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IntervalPair {
    pub truth: usize,
    pub predicted: usize,
    pub iou: f64,
}

/// One-to-one matching of intervals whose IoU is at least `threshold` (inclusive),
/// best IoU first, then truth index, then answer index.
pub fn match_intervals(
    truth: &[(u64, u64)],
    predicted: &[(u64, u64)],
    threshold: f64,
) -> Result<Vec<IntervalPair>, MetricError> {
    finite("the IoU threshold", threshold)?;
    let mut candidates = Vec::new();
    for (i, truth_interval) in truth.iter().enumerate() {
        for (j, predicted_interval) in predicted.iter().enumerate() {
            if let Some(value) = iou(*truth_interval, *predicted_interval) {
                if value >= threshold {
                    candidates.push(IntervalPair {
                        truth: i,
                        predicted: j,
                        iou: value,
                    });
                }
            }
        }
    }
    candidates.sort_by(|a, b| {
        b.iou
            .total_cmp(&a.iou)
            .then(a.truth.cmp(&b.truth))
            .then(a.predicted.cmp(&b.predicted))
    });
    Ok(accept_unused(
        candidates,
        truth.len(),
        predicted.len(),
        |pair| (pair.truth, pair.predicted),
    ))
}

/// Walk sorted candidates and keep each whose two sides are both still unused.
fn accept_unused<T: Copy>(
    candidates: Vec<T>,
    truth_len: usize,
    predicted_len: usize,
    sides: impl Fn(&T) -> (usize, usize),
) -> Vec<T> {
    let mut truth_used = vec![false; truth_len];
    let mut predicted_used = vec![false; predicted_len];
    let mut accepted = Vec::new();
    for candidate in candidates {
        let (i, j) = sides(&candidate);
        if !truth_used[i] && !predicted_used[j] {
            truth_used[i] = true;
            predicted_used[j] = true;
            accepted.push(candidate);
        }
    }
    accepted
}

/// A running mean that refuses non-finite values.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Mean {
    count: u64,
    sum: f64,
}

impl Mean {
    pub fn push(&mut self, what: &'static str, value: f64) -> Result<(), MetricError> {
        finite(what, value)?;
        let sum = finite(what, self.sum + value)?;
        self.sum = sum;
        self.count += 1;
        Ok(())
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// `None` when nothing was pushed.
    pub fn value(&self) -> Option<f64> {
        (self.count > 0).then(|| self.sum / self.count as f64)
    }
}

/// `|a - b|` of two finite numbers; an error if either is not finite or the
/// difference overflows.
pub fn abs_error(what: &'static str, a: f64, b: f64) -> Result<f64, MetricError> {
    finite(what, a)?;
    finite(what, b)?;
    finite(what, (a - b).abs())
}

/// The median; the mean of the two middle values for an even count; `None` for no
/// values; an error if any value is not finite.
pub fn median(what: &'static str, values: &[f64]) -> Result<Option<f64>, MetricError> {
    for value in values {
        finite(what, *value)?;
    }
    if values.is_empty() {
        return Ok(None);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    let value = if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    };
    Ok(Some(finite(what, value)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn times(values: &[u64]) -> Vec<(u64, ())> {
        values.iter().map(|time| (*time, ())).collect()
    }

    fn pairs(truth: &[u64], predicted: &[u64], tolerance: u64) -> Vec<(usize, usize)> {
        match_times(&times(truth), &times(predicted), tolerance)
            .iter()
            .map(|pair| (pair.truth, pair.predicted))
            .collect()
    }

    #[test]
    fn precision_recall_and_f1_come_from_totals() {
        let counts = Counts {
            tp: 3,
            fp: 1,
            fn_: 2,
        };
        assert_eq!(counts.precision(), Some(0.75));
        assert_eq!(counts.recall(), Some(0.6));
        let f1 = counts.f1().expect("f1");
        assert!((f1 - 6.0 / 9.0).abs() < 1e-12, "{f1}");
    }

    #[test]
    fn nothing_to_score_is_none_not_one() {
        let counts = Counts::default();
        assert_eq!(counts.precision(), None);
        assert_eq!(counts.recall(), None);
        assert_eq!(counts.f1(), None);
    }

    #[test]
    fn an_empty_answer_has_no_precision_and_zero_recall() {
        let counts = Counts::from_matches(0, 4, 0);
        assert_eq!(counts.precision(), None);
        assert_eq!(counts.recall(), Some(0.0));
        assert_eq!(counts.f1(), Some(0.0));
    }

    #[test]
    fn answers_against_an_empty_truth_have_zero_precision_and_no_recall() {
        let counts = Counts::from_matches(0, 0, 3);
        assert_eq!(counts.precision(), Some(0.0));
        assert_eq!(counts.recall(), None);
        assert_eq!(counts.f1(), Some(0.0));
    }

    #[test]
    fn a_perfect_answer_scores_one_everywhere() {
        let counts = Counts::from_matches(5, 5, 5);
        assert_eq!(counts.scores().precision, Some(1.0));
        assert_eq!(counts.scores().recall, Some(1.0));
        assert_eq!(counts.scores().f1, Some(1.0));
    }

    #[test]
    fn counts_add_before_rates_are_taken() {
        // Record 1: 1 of 1 found. Record 2: 0 of 3 found, 1 wrong.
        let mut total = Counts::default();
        total.add(Counts::from_matches(1, 1, 1));
        total.add(Counts::from_matches(0, 3, 1));
        assert_eq!(
            total,
            Counts {
                tp: 1,
                fp: 1,
                fn_: 3
            }
        );
        // Micro recall 1/4, not the mean of 1.0 and 0.0.
        assert_eq!(total.recall(), Some(0.25));
        // A third record adds to every total, the false negatives included.
        total.add(Counts::from_matches(0, 2, 0));
        assert_eq!(
            total,
            Counts {
                tp: 1,
                fp: 1,
                fn_: 5
            }
        );
    }

    #[test]
    fn a_share_larger_than_one_is_an_error() {
        assert_eq!(ratio(1, 4).ok(), Some(Some(0.25)));
        assert_eq!(ratio(4, 4).ok(), Some(Some(1.0)));
        assert_eq!(ratio(0, 0).ok(), Some(None));
        assert_eq!(ratio(0, 5).ok(), Some(Some(0.0)));
        assert!(ratio(5, 4).is_err());
        assert!(ratio(1, 0).is_err());
    }

    #[test]
    fn matching_nothing_gives_no_pairs() {
        assert!(pairs(&[], &[], 100).is_empty());
        assert!(pairs(&[1_000], &[], 100).is_empty());
        assert!(pairs(&[], &[1_000], 100).is_empty());
    }

    #[test]
    fn the_tolerance_is_inclusive() {
        assert_eq!(pairs(&[1_000], &[1_100], 100), vec![(0, 0)]);
        assert_eq!(pairs(&[1_000], &[900], 100), vec![(0, 0)]);
        assert!(pairs(&[1_000], &[1_101], 100).is_empty());
        assert!(pairs(&[1_000], &[899], 100).is_empty());
        // A zero tolerance still matches equal times.
        assert_eq!(pairs(&[7], &[7], 0), vec![(0, 0)]);
        assert!(pairs(&[7], &[8], 0).is_empty());
    }

    #[test]
    fn an_answer_item_is_used_once() {
        // One answer between two truth items: it goes to the closer one.
        assert_eq!(pairs(&[1_000, 1_150], &[1_120], 200), vec![(1, 0)]);
    }

    #[test]
    fn the_closest_pair_wins_even_when_it_is_not_first_in_order() {
        // 0-6 is 6 apart, 10-6 is 4 apart: the later truth item takes the answer.
        assert_eq!(pairs(&[0, 10], &[6], 10), vec![(1, 0)]);
    }

    #[test]
    fn equal_distances_go_to_the_lower_truth_index_then_the_lower_answer_index() {
        // Two answers equally near one truth item: the lower answer index.
        assert_eq!(pairs(&[100], &[90, 110], 20), vec![(0, 0)]);
        // Two truth items equally near one answer: the lower truth index.
        assert_eq!(pairs(&[90, 110], &[100], 20), vec![(0, 0)]);
        // The index decides, not which time is earlier (110 is at index 0 here).
        assert_eq!(pairs(&[100], &[110, 90], 20), vec![(0, 0)]);
    }

    #[test]
    fn matching_is_one_to_one_with_several_pairs() {
        let found = pairs(&[100, 200, 300], &[205, 99, 500], 20);
        assert_eq!(found, vec![(0, 1), (1, 0)]);
    }

    #[test]
    fn the_label_must_agree() {
        let truth = vec![(1_000u64, "punch_in")];
        let wrong = vec![(1_000u64, "zoom_out")];
        let right = vec![(1_010u64, "punch_in")];
        assert!(match_times(&truth, &wrong, 100).is_empty());
        let matched = match_times(&truth, &right, 100);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].distance_ms, 10);
    }

    #[test]
    fn far_apart_times_do_not_overflow() {
        assert!(pairs(&[0], &[u64::MAX], u64::MAX - 1).is_empty());
        assert_eq!(pairs(&[0], &[u64::MAX], u64::MAX), vec![(0, 0)]);
    }

    #[test]
    fn iou_of_known_intervals() {
        assert_eq!(iou((0, 10), (0, 10)), Some(1.0));
        assert_eq!(iou((0, 10), (5, 15)), Some(5.0 / 15.0));
        assert_eq!(iou((0, 10), (10, 20)), Some(0.0));
        assert_eq!(iou((0, 10), (20, 30)), Some(0.0));
        assert_eq!(iou((0, 10), (2, 4)), Some(0.2));
        assert_eq!(iou((2, 4), (0, 10)), Some(0.2));
    }

    #[test]
    fn iou_is_undefined_for_an_empty_union_or_a_backwards_interval() {
        assert_eq!(iou((5, 5), (5, 5)), None);
        assert_eq!(iou((10, 5), (0, 20)), None);
        assert_eq!(iou((0, 20), (10, 5)), None);
        // An empty interval against a real one has an intersection of zero.
        assert_eq!(iou((5, 5), (0, 10)), Some(0.0));
    }

    #[test]
    fn intervals_match_at_the_threshold_inclusive_and_best_first() {
        // (0,10) vs (5,15) has IoU 1/3; (0,10) vs (0,5) has IoU 0.5.
        let matched = match_intervals(&[(0, 10)], &[(5, 15), (0, 5)], 0.5).expect("ok");
        assert_eq!(matched.len(), 1);
        assert_eq!((matched[0].truth, matched[0].predicted), (0, 1));
        assert_eq!(matched[0].iou, 0.5);
        assert!(match_intervals(&[(0, 10)], &[(5, 15)], 0.5)
            .expect("ok")
            .is_empty());
        assert_eq!(
            match_intervals(&[(0, 10)], &[(0, 5)], 0.5)
                .expect("ok")
                .len(),
            1
        );
    }

    #[test]
    fn the_better_overlap_is_taken_first_whatever_the_order_of_the_answers() {
        // IoU 0.6 at index 0 and 0.9 at index 1: the truth takes index 1.
        let matched = match_intervals(&[(0, 10)], &[(0, 6), (0, 9)], 0.5).expect("ok");
        assert_eq!(matched.len(), 1);
        assert_eq!((matched[0].truth, matched[0].predicted), (0, 1));
        assert_eq!(matched[0].iou, 0.9);
    }

    #[test]
    fn interval_ties_go_to_the_lower_indices_and_each_item_is_used_once() {
        let matched = match_intervals(&[(0, 10), (0, 10)], &[(0, 10), (0, 10)], 0.5).expect("ok");
        let found: Vec<_> = matched.iter().map(|p| (p.truth, p.predicted)).collect();
        assert_eq!(found, vec![(0, 0), (1, 1)]);
        let one = match_intervals(&[(0, 10)], &[(0, 10), (0, 10)], 0.5).expect("ok");
        assert_eq!(one.len(), 1);
        assert_eq!((one[0].truth, one[0].predicted), (0, 0));
    }

    #[test]
    fn a_non_finite_threshold_is_refused() {
        assert!(match_intervals(&[(0, 10)], &[(0, 10)], f64::NAN).is_err());
        assert!(match_intervals(&[(0, 10)], &[(0, 10)], f64::INFINITY).is_err());
    }

    #[test]
    fn a_mean_of_nothing_is_none() {
        assert_eq!(Mean::default().value(), None);
        assert_eq!(Mean::default().count(), 0);
    }

    #[test]
    fn a_mean_is_the_sum_over_the_count() {
        let mut mean = Mean::default();
        for value in [1.0, 2.0, 6.0] {
            mean.push("x", value).expect("finite");
        }
        assert_eq!(mean.count(), 3);
        assert_eq!(mean.value(), Some(3.0));
    }

    #[test]
    fn a_mean_refuses_nan_and_infinity_and_keeps_its_state() {
        let mut mean = Mean::default();
        mean.push("x", 4.0).expect("finite");
        assert!(mean.push("x", f64::NAN).is_err());
        assert!(mean.push("x", f64::INFINITY).is_err());
        assert!(mean.push("x", f64::NEG_INFINITY).is_err());
        assert_eq!(mean.count(), 1);
        assert_eq!(mean.value(), Some(4.0));
    }

    #[test]
    fn a_sum_that_overflows_is_refused_not_turned_into_infinity() {
        let mut mean = Mean::default();
        mean.push("x", f64::MAX).expect("finite");
        assert!(mean.push("x", f64::MAX).is_err());
        assert_eq!(mean.value(), Some(f64::MAX));
    }

    #[test]
    fn the_error_names_the_quantity() {
        let error = Mean::default().push("the scale", f64::NAN).unwrap_err();
        assert!(error.to_string().contains("the scale"), "{error}");
    }

    #[test]
    fn absolute_error_is_symmetric_and_refuses_non_finite_input() {
        assert_eq!(abs_error("x", 3.0, 5.5).ok(), Some(2.5));
        assert_eq!(abs_error("x", 5.5, 3.0).ok(), Some(2.5));
        assert_eq!(abs_error("x", 2.0, 2.0).ok(), Some(0.0));
        assert!(abs_error("x", f64::NAN, 1.0).is_err());
        assert!(abs_error("x", 1.0, f64::INFINITY).is_err());
        assert!(abs_error("x", f64::MAX, -f64::MAX).is_err());
    }

    #[test]
    fn the_median_of_odd_even_and_no_values() {
        assert_eq!(median("x", &[]).ok(), Some(None));
        assert_eq!(median("x", &[7.0]).ok(), Some(Some(7.0)));
        assert_eq!(median("x", &[9.0, 1.0, 5.0]).ok(), Some(Some(5.0)));
        assert_eq!(median("x", &[4.0, 1.0, 3.0, 2.0]).ok(), Some(Some(2.5)));
        assert_eq!(median("x", &[2.0, 2.0, 2.0, 2.0]).ok(), Some(Some(2.0)));
    }

    #[test]
    fn a_nan_that_sorts_past_the_middle_is_still_refused() {
        // The median of [1, 2, NaN] would be 2 if the NaN were not looked for.
        assert!(median("x", &[1.0, 2.0, f64::NAN]).is_err());
        assert!(median("x", &[f64::NEG_INFINITY, 2.0, 3.0]).is_err());
    }

    #[test]
    fn the_median_refuses_non_finite_values() {
        assert!(median("x", &[1.0, f64::NAN]).is_err());
        assert!(median("x", &[f64::INFINITY]).is_err());
    }

    #[test]
    fn the_middle_of_two_huge_values_is_refused_when_it_overflows() {
        assert!(median("x", &[f64::MAX, f64::MAX]).is_err());
    }
}
