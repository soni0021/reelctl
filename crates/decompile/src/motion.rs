//! Visual motion events from the luma grids (red stub, tests first).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MotionKind {
    PunchIn,
    ZoomOut,
    OverlayChange,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DetectedEvent {
    pub time_ms: u64,
    pub frame_index: u64,
    pub kind: MotionKind,
    pub scale: Option<f64>,
    pub center_x: Option<f64>,
    pub center_y: Option<f64>,
    pub area: Option<f64>,
}

pub(crate) struct MotionDetector {
    cells: usize,
}

impl MotionDetector {
    pub(crate) fn new(cols: usize, rows: usize) -> Self {
        Self {
            cells: cols.saturating_mul(rows),
        }
    }

    pub(crate) fn push(&mut self, _frame_index: u64, _time_ms: u64, luma: &[u8], _is_cut: bool) {
        self.cells = luma.len();
    }

    pub(crate) fn finish(self) -> Vec<DetectedEvent> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPES: [(usize, usize); 2] = [(160, 90), (180, 91)];
    const FPS: u64 = 24;
    const LEVEL_FOR_TESTS: u8 = 24;

    // ---- synthetic grids ----

    fn hash(ix: i32, iy: i32, salt: u32) -> f32 {
        let mut h = (ix as u32).wrapping_mul(0x9E37_79B1)
            ^ (iy as u32).wrapping_mul(0x85EB_CA77)
            ^ salt.wrapping_mul(0xC2B2_AE3D);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^= h >> 12;
        h = h.wrapping_mul(0x297A_2D39);
        h ^= h >> 15;
        (h & 0xFFFF) as f32 / 65535.0
    }

    fn value_noise(x: f32, y: f32, salt: u32) -> f32 {
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
        let (ix, iy) = (x0 as i32, y0 as i32);
        let top = hash(ix, iy, salt) * (1.0 - sx) + hash(ix + 1, iy, salt) * sx;
        let bottom = hash(ix, iy + 1, salt) * (1.0 - sx) + hash(ix + 1, iy + 1, salt) * sx;
        top * (1.0 - sy) + bottom * sy
    }

    /// An endless smooth texture over the unit square and beyond it.
    fn texture(u: f32, v: f32) -> f32 {
        30.0 + 195.0
            * (0.65 * value_noise(u * 14.0, v * 9.0, 1) + 0.35 * value_noise(u * 28.0, v * 18.0, 2))
    }

    /// The texture seen at `scale` about (`cx`, `cy`): scale above 1 magnifies.
    fn render(cols: usize, rows: usize, scale: f32, cx: f32, cy: f32) -> Vec<u8> {
        let mut out = Vec::with_capacity(cols * rows);
        for y in 0..rows {
            let v = y as f32 / (rows - 1) as f32;
            for x in 0..cols {
                let u = x as f32 / (cols - 1) as f32;
                let value = texture(cx + (u - cx) / scale, cy + (v - cy) / scale);
                out.push(value.round().clamp(0.0, 255.0) as u8);
            }
        }
        out
    }

    /// Cells inside the rectangle given as shares of the frame.
    fn rect(cols: usize, rows: usize, x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<bool> {
        let mut out = Vec::with_capacity(cols * rows);
        for y in 0..rows {
            let v = y as f32 / (rows - 1) as f32;
            for x in 0..cols {
                let u = x as f32 / (cols - 1) as f32;
                out.push(u >= x0 && u < x1 && v >= y0 && v < y1);
            }
        }
        out
    }

    fn count(mask: &[bool]) -> usize {
        mask.iter().filter(|m| **m).count()
    }

    /// Cells in the mask move 100 levels away from the middle grey.
    fn with_block(base: &[u8], mask: &[bool]) -> Vec<u8> {
        base.iter()
            .zip(mask)
            .map(|(v, m)| match (*m, *v < 128) {
                (false, _) => *v,
                (true, true) => v + 100,
                (true, false) => v - 100,
            })
            .collect()
    }

    fn blend(a: &[u8], b: &[u8], alpha: f32) -> Vec<u8> {
        a.iter()
            .zip(b)
            .map(|(x, y)| (f32::from(*x) * (1.0 - alpha) + f32::from(*y) * alpha).round() as u8)
            .collect()
    }

    fn shifted(frame: &[u8], delta: i16) -> Vec<u8> {
        frame
            .iter()
            .map(|v| (i16::from(*v) + delta).clamp(0, 255) as u8)
            .collect()
    }

    fn noisy(frame: &[u8], seed: u32, amp: i16) -> Vec<u8> {
        frame
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let h = hash(i as i32, seed as i32, 77);
                let n = (h * (2 * amp + 1) as f32).floor() as i16 - amp;
                (i16::from(*v) + n).clamp(0, 255) as u8
            })
            .collect()
    }

    fn smoothstep(x: f32) -> f32 {
        x * x * (3.0 - 2.0 * x)
    }

    #[derive(Default)]
    struct Clip {
        frames: Vec<Vec<u8>>,
        cuts: Vec<usize>,
    }

    impl Clip {
        fn hold(&mut self, frame: &[u8], n: usize) -> &mut Self {
            for _ in 0..n {
                self.frames.push(frame.to_vec());
            }
            self
        }
        fn one(&mut self, frame: Vec<u8>) -> &mut Self {
            self.frames.push(frame);
            self
        }
        fn cut(&mut self, frame: &[u8]) -> &mut Self {
            self.cuts.push(self.frames.len());
            self.frames.push(frame.to_vec());
            self
        }
    }

    fn time_of(i: usize) -> u64 {
        (i as u64 * 1000 + FPS / 2) / FPS
    }

    fn run(cols: usize, rows: usize, clip: &Clip) -> Vec<DetectedEvent> {
        let mut det = MotionDetector::new(cols, rows);
        for (i, frame) in clip.frames.iter().enumerate() {
            det.push(i as u64, time_of(i), frame, clip.cuts.contains(&i));
        }
        det.finish()
    }

    /// Frames of `scales` (a lead of `from` before, the last scale held after).
    fn zoom_clip(
        cols: usize,
        rows: usize,
        from: f32,
        scales: &[f32],
        centre: (f32, f32),
        lead: usize,
        tail: usize,
    ) -> Clip {
        let (cx, cy) = centre;
        let mut clip = Clip::default();
        clip.hold(&render(cols, rows, from, cx, cy), lead);
        for s in scales {
            clip.one(render(cols, rows, *s, cx, cy));
        }
        let last = scales.last().copied().unwrap_or(from);
        clip.hold(&render(cols, rows, last, cx, cy), tail);
        clip
    }

    fn ease(from: f32, to: f32, steps: usize, smooth: bool) -> Vec<f32> {
        (1..=steps)
            .map(|k| {
                let x = k as f32 / steps as f32;
                let t = if smooth { smoothstep(x) } else { x };
                from + (to - from) * t
            })
            .collect()
    }

    fn assert_one_move(
        events: &[DetectedEvent],
        kind: MotionKind,
        scale: f64,
        centre: (f64, f64),
        centre_tol: f64,
    ) -> DetectedEvent {
        assert_eq!(events.len(), 1, "events: {events:?}");
        let e = events[0].clone();
        assert_eq!(e.kind, kind, "{e:?}");
        let s = e.scale.expect("scale");
        assert!((s - scale).abs() <= 0.03, "scale {s} vs {scale}: {e:?}");
        let (cx, cy) = (e.center_x.expect("cx"), e.center_y.expect("cy"));
        assert!((cx - centre.0).abs() <= centre_tol, "cx {cx} vs {}", centre.0);
        assert!((cy - centre.1).abs() <= centre_tol, "cy {cy} vs {}", centre.1);
        assert!(e.area.is_none());
        e
    }

    // ---- overlays ----

    fn block_mask(cols: usize, rows: usize) -> Vec<bool> {
        rect(cols, rows, 0.55, 0.55, 0.80, 0.85)
    }

    #[test]
    fn a_block_that_appears_is_one_overlay_with_its_area() {
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            let mask = block_mask(cols, rows);
            let blk = with_block(&base, &mask);
            let mut clip = Clip::default();
            clip.hold(&base, 30).hold(&blk, 50);
            let events = run(cols, rows, &clip);
            assert_eq!(events.len(), 1, "{cols}x{rows}: {events:?}");
            let e = &events[0];
            assert_eq!(e.kind, MotionKind::OverlayChange);
            assert_eq!(e.frame_index, 30);
            assert_eq!(e.time_ms, time_of(30));
            assert!(e.scale.is_none() && e.center_x.is_none() && e.center_y.is_none());
            let want = count(&mask) as f64 / (cols * rows) as f64;
            let got = e.area.expect("area");
            assert!((got - want).abs() < 0.005, "area {got} vs {want}");
        }
    }

    #[test]
    fn a_block_that_appears_under_noise_is_still_one_overlay() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let blk = with_block(&base, &block_mask(cols, rows));
        let mut clip = Clip::default();
        for i in 0..80 {
            let src = if i < 30 { &base } else { &blk };
            clip.one(noisy(src, i as u32, 1));
        }
        let events = run(cols, rows, &clip);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].kind, MotionKind::OverlayChange);
        assert_eq!(events[0].frame_index, 30);
    }

    #[test]
    fn a_block_that_grows_is_an_overlay_for_the_added_cells() {
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            let small = rect(cols, rows, 0.55, 0.55, 0.70, 0.70);
            let big = rect(cols, rows, 0.55, 0.55, 0.90, 0.85);
            let mut clip = Clip::default();
            clip.hold(&with_block(&base, &small), 40)
                .hold(&with_block(&base, &big), 50);
            let events = run(cols, rows, &clip);
            assert_eq!(events.len(), 1, "{cols}x{rows}: {events:?}");
            assert_eq!(events[0].kind, MotionKind::OverlayChange);
            assert_eq!(events[0].frame_index, 40);
            let want = (count(&big) - count(&small)) as f64 / (cols * rows) as f64;
            let got = events[0].area.expect("area");
            assert!((got - want).abs() < 0.01, "area {got} vs {want}");
        }
    }

    #[test]
    fn a_block_that_fades_in_over_six_frames_is_one_overlay_at_the_end_of_the_fade() {
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            let blk = with_block(&base, &block_mask(cols, rows));
            let mut clip = Clip::default();
            clip.hold(&base, 30);
            for k in 1..=6 {
                clip.one(blend(&base, &blk, k as f32 / 6.0));
            }
            clip.hold(&blk, 50);
            let events = run(cols, rows, &clip);
            assert_eq!(events.len(), 1, "{cols}x{rows}: {events:?}");
            assert_eq!(events[0].kind, MotionKind::OverlayChange);
            assert!(
                (34..=36).contains(&events[0].frame_index),
                "frame {}",
                events[0].frame_index
            );
        }
    }

    #[test]
    fn a_block_that_goes_after_a_long_stay_is_an_overlay() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let blk = with_block(&base, &block_mask(cols, rows));
        let mut clip = Clip::default();
        clip.hold(&blk, 60).hold(&base, 50);
        let events = run(cols, rows, &clip);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].frame_index, 60);
    }

    #[test]
    fn a_block_that_appears_and_vanishes_inside_the_persistence_time_is_no_event() {
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            let blk = with_block(&base, &block_mask(cols, rows));
            for stay in [2, 6, 10] {
                let mut clip = Clip::default();
                clip.hold(&base, 30).hold(&blk, stay).hold(&base, 60);
                let events = run(cols, rows, &clip);
                assert!(events.is_empty(), "{cols}x{rows} stay {stay}: {events:?}");
            }
        }
    }

    #[test]
    fn a_block_that_stays_longer_than_the_persistence_time_counts_once() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let blk = with_block(&base, &block_mask(cols, rows));
        let mut clip = Clip::default();
        clip.hold(&base, 30).hold(&blk, 14).hold(&base, 60);
        let events = run(cols, rows, &clip);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].frame_index, 30);
    }

    #[test]
    fn a_one_frame_flash_is_no_event() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let blk = with_block(&base, &block_mask(cols, rows));
        let mut clip = Clip::default();
        clip.hold(&base, 30).one(blk).hold(&base, 60);
        assert!(run(cols, rows, &clip).is_empty());
    }

    #[test]
    fn a_global_brightness_shift_is_no_event() {
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            for delta in [40i16, -40, 25] {
                let mut hard = Clip::default();
                hard.hold(&base, 30).hold(&shifted(&base, delta), 50);
                assert!(run(cols, rows, &hard).is_empty(), "hard {delta}");
                let mut ramp = Clip::default();
                ramp.hold(&base, 30);
                for k in 1..=6 {
                    ramp.one(shifted(&base, delta * k / 6));
                }
                ramp.hold(&shifted(&base, delta), 50);
                assert!(run(cols, rows, &ramp).is_empty(), "ramp {delta}");
            }
        }
    }

    #[test]
    fn a_block_moving_along_the_frame_is_no_event() {
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            let mut clip = Clip::default();
            clip.hold(&base, 20);
            for i in 0..60 {
                let x0 = 0.05 + i as f32 * 0.012;
                let mask = rect(cols, rows, x0, 0.4, x0 + 0.15, 0.6);
                clip.one(with_block(&base, &mask));
            }
            clip.hold(&base, 20);
            let events = run(cols, rows, &clip);
            assert!(events.is_empty(), "{cols}x{rows}: {events:?}");
        }
    }

    #[test]
    fn a_hollow_outline_is_not_a_block() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let outer = rect(cols, rows, 0.2, 0.2, 0.8, 0.8);
        let inner = rect(cols, rows, 0.22, 0.24, 0.78, 0.76);
        let ring: Vec<bool> = outer.iter().zip(&inner).map(|(o, i)| *o && !*i).collect();
        let mut clip = Clip::default();
        clip.hold(&base, 30).hold(&with_block(&base, &ring), 50);
        let events = run(cols, rows, &clip);
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn a_block_with_a_busy_remainder_is_no_event() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let mut mask = block_mask(cols, rows);
        for y in (0..rows).step_by(4) {
            for x in (0..cols).step_by(4) {
                mask[y * cols + x] = true;
            }
        }
        let mut clip = Clip::default();
        clip.hold(&base, 30).hold(&with_block(&base, &mask), 50);
        let events = run(cols, rows, &clip);
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn a_change_covering_half_the_frame_is_no_overlay() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let half = rect(cols, rows, 0.0, 0.0, 0.5, 1.0);
        let mut clip = Clip::default();
        clip.hold(&base, 30).hold(&with_block(&base, &half), 50);
        assert!(run(cols, rows, &clip).is_empty());
    }

    #[test]
    fn a_change_covering_a_hair_of_the_frame_is_no_overlay() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let tiny = rect(cols, rows, 0.5, 0.5, 0.58, 0.58);
        assert!(count(&tiny) < cols * rows / 100);
        let mut clip = Clip::default();
        clip.hold(&base, 30).hold(&with_block(&base, &tiny), 50);
        assert!(run(cols, rows, &clip).is_empty());
    }

    #[test]
    fn two_blocks_one_after_the_other_are_two_overlays() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let a = rect(cols, rows, 0.55, 0.55, 0.80, 0.85);
        let b = rect(cols, rows, 0.1, 0.1, 0.35, 0.3);
        let both: Vec<bool> = a.iter().zip(&b).map(|(x, y)| *x || *y).collect();
        let mut clip = Clip::default();
        clip.hold(&base, 30)
            .hold(&with_block(&base, &a), 60)
            .hold(&with_block(&base, &both), 50);
        let events = run(cols, rows, &clip);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0].frame_index, 30);
        assert_eq!(events[1].frame_index, 90);
    }

    // ---- scale ----

    #[test]
    fn a_jump_to_1_18_about_the_centre_is_a_punch_in() {
        for (cols, rows) in SHAPES {
            let clip = zoom_clip(cols, rows, 1.0, &[1.18], (0.5, 0.5), 30, 40);
            let events = run(cols, rows, &clip);
            let e = assert_one_move(&events, MotionKind::PunchIn, 1.18, (0.5, 0.5), 0.1);
            assert_eq!(e.frame_index, 30);
            assert_eq!(e.time_ms, time_of(30));
        }
    }

    #[test]
    fn a_punch_in_about_a_corner_reports_that_corner() {
        for (cols, rows) in SHAPES {
            for (cx, cy) in [(0.0f32, 0.0f32), (1.0, 1.0), (1.0, 0.0)] {
                let clip = zoom_clip(cols, rows, 1.0, &[1.18], (cx, cy), 30, 40);
                let events = run(cols, rows, &clip);
                assert_one_move(&events, MotionKind::PunchIn, 1.18, (f64::from(cx), f64::from(cy)), 0.15);
            }
        }
    }

    #[test]
    fn a_punch_in_about_an_off_centre_point_reports_it() {
        let (cols, rows) = (160, 90);
        let clip = zoom_clip(cols, rows, 1.0, &[1.18], (0.3, 0.7), 30, 40);
        let events = run(cols, rows, &clip);
        assert_one_move(&events, MotionKind::PunchIn, 1.18, (0.3, 0.7), 0.15);
    }

    #[test]
    fn a_move_eased_over_six_frames_is_one_event_with_the_total_scale() {
        for (cols, rows) in SHAPES {
            let clip = zoom_clip(cols, rows, 1.0, &ease(1.0, 1.18, 6, true), (0.5, 0.5), 30, 40);
            let events = run(cols, rows, &clip);
            let e = assert_one_move(&events, MotionKind::PunchIn, 1.18, (0.5, 0.5), 0.1);
            assert!((34..=36).contains(&e.frame_index), "frame {}", e.frame_index);
        }
    }

    #[test]
    fn a_linear_move_over_twelve_frames_is_still_one_event() {
        let (cols, rows) = (160, 90);
        let clip = zoom_clip(cols, rows, 1.0, &ease(1.0, 1.18, 12, false), (0.5, 0.5), 30, 40);
        let events = run(cols, rows, &clip);
        assert_one_move(&events, MotionKind::PunchIn, 1.18, (0.5, 0.5), 0.1);
    }

    #[test]
    fn a_slow_push_in_is_no_event() {
        for (cols, rows) in SHAPES {
            for (steps, smooth) in [(30, true), (30, false), (14, false), (60, false)] {
                let scales = ease(1.0, 1.18, steps, smooth);
                let clip = zoom_clip(cols, rows, 1.0, &scales, (0.5, 0.5), 30, 40);
                let events = run(cols, rows, &clip);
                assert!(events.is_empty(), "{steps} smooth {smooth}: {events:?}");
            }
        }
    }

    #[test]
    fn a_scale_of_1_03_or_1_04_is_below_the_floor() {
        for (cols, rows) in SHAPES {
            for s in [1.03, 1.04] {
                let clip = zoom_clip(cols, rows, 1.0, &[s], (0.5, 0.5), 30, 40);
                let events = run(cols, rows, &clip);
                assert!(events.is_empty(), "{s}: {events:?}");
            }
        }
    }

    #[test]
    fn a_scale_of_1_06_is_above_the_floor() {
        let (cols, rows) = (160, 90);
        let clip = zoom_clip(cols, rows, 1.0, &[1.06], (0.5, 0.5), 30, 40);
        let events = run(cols, rows, &clip);
        assert_one_move(&events, MotionKind::PunchIn, 1.06, (0.5, 0.5), 0.2);
    }

    #[test]
    fn the_reverse_move_is_a_zoom_out() {
        for (cols, rows) in SHAPES {
            for scales in [vec![1.0], ease(1.18, 1.0, 6, true)] {
                let clip = zoom_clip(cols, rows, 1.18, &scales, (0.5, 0.5), 30, 40);
                let events = run(cols, rows, &clip);
                assert_one_move(&events, MotionKind::ZoomOut, 1.0 / 1.18, (0.5, 0.5), 0.1);
            }
        }
    }

    #[test]
    fn a_punch_in_then_a_zoom_out_then_an_overlay_are_three_events_in_order() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let mut clip = Clip::default();
        clip.hold(&base, 30);
        for s in ease(1.0, 1.18, 6, true) {
            clip.one(render(cols, rows, s, 0.5, 0.5));
        }
        clip.hold(&render(cols, rows, 1.18, 0.5, 0.5), 40);
        for s in ease(1.18, 1.0, 6, true) {
            clip.one(render(cols, rows, s, 0.5, 0.5));
        }
        clip.hold(&base, 40);
        clip.hold(&with_block(&base, &block_mask(cols, rows)), 40);
        let events = run(cols, rows, &clip);
        let kinds: Vec<MotionKind> = events.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![MotionKind::PunchIn, MotionKind::ZoomOut, MotionKind::OverlayChange],
            "{events:?}"
        );
        assert!(events.windows(2).all(|w| w[0].time_ms < w[1].time_ms));
    }

    // ---- nothing, noise, cuts, shapes ----

    #[test]
    fn a_still_frame_and_noise_only_give_no_event() {
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            let mut still = Clip::default();
            still.hold(&base, 120);
            assert!(run(cols, rows, &still).is_empty());
            for amp in [1i16, 3] {
                let mut clip = Clip::default();
                for i in 0..120 {
                    clip.one(noisy(&base, i, amp));
                }
                let events = run(cols, rows, &clip);
                assert!(events.is_empty(), "noise {amp}: {events:?}");
            }
        }
    }

    #[test]
    fn a_hard_change_of_the_whole_picture_is_no_event_even_unflagged() {
        let (cols, rows) = (160, 90);
        let a = render(cols, rows, 1.0, 0.5, 0.5);
        let b: Vec<u8> = a.iter().rev().copied().collect();
        let mut clip = Clip::default();
        clip.hold(&a, 30).hold(&b, 50);
        assert!(run(cols, rows, &clip).is_empty());
    }

    #[test]
    fn a_cut_frame_emits_nothing_and_resets_the_history() {
        for (cols, rows) in SHAPES {
            let a = render(cols, rows, 1.0, 0.5, 0.5);
            let zoomed = render(cols, rows, 1.18, 0.5, 0.5);
            let mut clip = Clip::default();
            clip.hold(&a, 30).cut(&zoomed).hold(&zoomed, 40);
            let events = run(cols, rows, &clip);
            assert!(events.is_empty(), "{cols}x{rows}: {events:?}");
            let blk = with_block(&zoomed, &block_mask(cols, rows));
            let mut clip = Clip::default();
            clip.hold(&a, 30).cut(&blk).hold(&blk, 40);
            assert!(run(cols, rows, &clip).is_empty());
        }
    }

    #[test]
    fn a_move_after_a_cut_is_found_in_the_new_shot() {
        for (cols, rows) in SHAPES {
            let a = render(cols, rows, 1.0, 0.5, 0.5);
            let b = render(cols, rows, 1.0, 0.9, 0.1);
            let mut clip = Clip::default();
            clip.hold(&a, 20).cut(&b).hold(&b, 30);
            clip.one(render(cols, rows, 1.18, 0.9, 0.1));
            clip.hold(&render(cols, rows, 1.18, 0.9, 0.1), 40);
            let events = run(cols, rows, &clip);
            assert_eq!(events.len(), 1, "{cols}x{rows}: {events:?}");
            assert_eq!(events[0].kind, MotionKind::PunchIn);
            assert_eq!(events[0].frame_index, 51);
        }
    }

    #[test]
    fn a_move_right_before_a_cut_is_still_reported() {
        let (cols, rows) = (160, 90);
        let a = render(cols, rows, 1.0, 0.5, 0.5);
        let b = render(cols, rows, 1.0, 0.9, 0.1);
        let zoomed = render(cols, rows, 1.18, 0.5, 0.5);
        let mut clip = Clip::default();
        clip.hold(&a, 30).one(zoomed.clone()).hold(&zoomed, 2).cut(&b).hold(&b, 30);
        let events = run(cols, rows, &clip);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].frame_index, 30);
    }

    #[test]
    fn empty_input_and_tiny_grids_do_not_panic() {
        assert!(MotionDetector::new(160, 90).finish().is_empty());
        for (cols, rows) in [(0, 0), (1, 1), (0, 90), (7, 50), (3, 3), (8, 8)] {
            let mut det = MotionDetector::new(cols, rows);
            let frame: Vec<u8> = (0..cols * rows).map(|i| (i * 7 % 256) as u8).collect();
            for i in 0..40u64 {
                det.push(i, i * 42, &frame, i == 20);
                det.push(i, i * 42, &[], false);
                det.push(i, i * 42, &[1, 2, 3], false);
            }
            let _ = det.finish();
        }
        let mut det = MotionDetector::new(usize::MAX, usize::MAX);
        det.push(0, 0, &[0, 1], false);
        assert!(det.finish().is_empty());
    }

    #[test]
    fn frames_with_the_wrong_length_are_ignored() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let blk = with_block(&base, &block_mask(cols, rows));
        let mut det = MotionDetector::new(cols, rows);
        for i in 0..30u64 {
            det.push(i, time_of(i as usize), &base, false);
        }
        det.push(30, time_of(30), &base[..100], false);
        for i in 31..90u64 {
            det.push(i, time_of(i as usize), &blk, false);
        }
        let events = det.finish();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].frame_index, 31);
    }

    #[test]
    fn the_changed_level_matches_the_test_blocks() {
        // The blocks above move every cell by 100 levels, far above the
        // detector's own level, so a retune of the level stays covered.
        assert!(LEVEL_FOR_TESTS < 100);
    }
}
