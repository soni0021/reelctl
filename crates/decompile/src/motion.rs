//! Visual motion events found from the luma grids the cut detector already
//! builds: a punch-in or zoom-out (a scale of the picture that completes
//! within half a second) and an overlay change (a block of the frame that
//! changes and stays changed).
//!
//! The detector is pure: it sees only `cols` x `rows` bytes of luma per frame,
//! the frame's index and time, and whether the cut detector calls the frame a
//! cut. It keeps about 1.6 s of grids, so a still frame costs two cheap passes
//! over one grid (the difference to the previous frame and to the frame 250 ms
//! earlier). A scale is fitted only when a run of frames moves, once at the
//! end of the run.

use std::collections::VecDeque;

// ---- overlay change ----
/// A cell counts as changed if it moved this many luma levels: well above
/// compression noise, well below a caption box on a picture.
const LEVEL: u8 = 24;
/// A block must cover at least this share of the frame: a hand or a cursor
/// is smaller.
const MIN_AREA: f64 = 0.01;
/// ...and at most this share: more is a cut, a fade, or a shot change.
const MAX_AREA: f64 = 0.40;
/// The block must fill this share of its bounding box: a box or card does, a
/// moving person's two silhouettes or an outline do not.
const MIN_FILL: f64 = 0.5;
/// Changed cells outside the block may be this share of the block...
const OTHER_SHARE: f64 = 0.25;
/// ...plus this share of the frame (the remainder must be quiet).
const OTHER_FLOOR: f64 = 0.005;
/// The comparison grid is the newest one at least this old, so a fade counts.
const LOOKBACK_MS: u64 = 250;
/// The change must still be there this long after it was seen.
const PERSIST_MS: u64 = 500;
/// The block must already have been changed this long before the check...
const STABLE_MS: u64 = 200;
/// ...in at least this share of its cells (a block that jumps elsewhere inside
/// the persistence time did not stay; what moves inside a card does not matter).
const HOLD_SHARE: f64 = 0.7;
/// A change that restores this share of the block to the grid from before
/// the change window is a flash coming back, not a new overlay.
const REVERT_SHARE: f64 = 0.7;
/// The event is the first frame holding this share of the final change (not
/// more, because the final grid is chosen for being changed and a card that
/// animates inside never repeats it).
const FULL_SHARE: f64 = 0.75;
/// The block must have been still before the comparison grid: no more than
/// `QUIET_SHARE` of its cells may have moved by `QUIET_LEVEL` in the
/// `QUIET_MS` before it. A fade that is not over, or a card that animates
/// inside, is one change, not one every half second.
const QUIET_MS: u64 = 150;
const QUIET_LEVEL: u8 = 8;
const QUIET_SHARE: f64 = 0.3;

// ---- scale ----
/// A frame is compared with the newest grid at least this old (three frames
/// at 24 fps), so a move eased over a few frames shows up whatever the frame
/// rate, and a slow drift does not.
const BASE_MS: u64 = 125;
/// Mean absolute difference (luma levels) to that grid above which a frame is
/// moving and a scale may be fitted. Below it a frame costs one comparison.
const SCALE_FLOOR: f32 = 1.5;
/// A run of moving frames may last this long and still be a move: a move of
/// 12 frames at 24 fps (0.5 s) plus the time the baseline lags behind it. A
/// longer run is a drift, a gesture or a shaky camera.
const MAX_RUN_MS: u64 = 600;
/// A real move is still before and after it; this much time is checked on
/// each side so a slow drift (which keeps going) is not an event.
const EXT_MS: u64 = 250;
/// An event's scale must differ from 1 by at least this much.
const MIN_SCALE_DELTA: f64 = 0.05;
/// The best coarse fit must beat the plain difference by this factor to be
/// refined (a cheap gate for a gesture or a flash).
const GATE_RATIO: f32 = 0.85;
/// A scale explains a change if its fit beats the plain difference by this
/// factor.
const BEST_RATIO: f32 = 0.6;
/// The scale over the run plus its margins must be this share of the run's.
const EXT_RATIO_MIN: f32 = 0.75;
/// ...and at most this.
const EXT_RATIO_MAX: f32 = 1.5;
/// ln of the largest scale tried (1.25; its inverse is 0.80).
const LN_MAX: f32 = 0.2232;
const COARSE_LN: [f32; 12] = [
    -0.2232, -0.186, -0.1488, -0.1116, -0.0744, -0.0372, 0.0372, 0.0744, 0.1116, 0.1488, 0.186,
    0.2232,
];
/// Refinement steps: (ln step, centre step), each repeated while it helps.
const REFINE: [(f32, f32); 3] = [(0.019, 0.2), (0.0093, 0.1), (0.0047, 0.05)];

// ---- storage ----
const HISTORY_MS: u64 = 1600;
const MAX_FRAMES: usize = 256;
/// Grids with a side shorter than this carry too little to measure.
const MIN_SIDE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MotionKind {
    PunchIn,
    ZoomOut,
    OverlayChange,
}

/// One event, ready to be mapped to the `edit_dna.json` type.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DetectedEvent {
    /// The first frame that shows the change in full.
    pub time_ms: u64,
    pub frame_index: u64,
    pub kind: MotionKind,
    /// The factor between the picture before and after (above 1 for a
    /// punch-in, below 1 for a zoom-out).
    pub scale: Option<f64>,
    /// The fixed point of the move, as a share of the frame.
    pub center_x: Option<f64>,
    pub center_y: Option<f64>,
    /// The changed share of the frame for an overlay.
    pub area: Option<f64>,
}

struct Frame {
    index: u64,
    time_ms: u64,
    luma: Vec<u8>,
}

/// A run of consecutive frames that each differ from the last by more than
/// the floor.
#[derive(Clone, Copy)]
struct Burst {
    /// The grid the first moving frame was compared with: before the move.
    anchor_index: u64,
    anchor_time: u64,
    first_time: u64,
    last_index: u64,
    last_time: u64,
}

/// A move whose margins are still to be checked.
#[derive(Clone, Copy)]
struct PendingMove {
    anchor_index: u64,
    anchor_time: u64,
    end_index: u64,
    end_time: u64,
    ln: f32,
    cx: f32,
    cy: f32,
    resolve_at: u64,
}

/// A change against an earlier grid that must still prove it persists.
#[derive(Clone, Copy)]
struct PendingOverlay {
    cand_index: u64,
    cand_time: u64,
    ref_index: u64,
    verify_at: u64,
}

struct Scratch {
    mask: Vec<u8>,
    stack: Vec<usize>,
}

pub(crate) struct MotionDetector {
    cols: usize,
    rows: usize,
    n: usize,
    enabled: bool,
    ring: VecDeque<Frame>,
    spare: Vec<Vec<u8>>,
    burst: Option<Burst>,
    /// A run of moving frames has outlasted the longest move.
    busy: bool,
    overlay: Option<PendingOverlay>,
    moves: Vec<PendingMove>,
    events: Vec<DetectedEvent>,
    scratch: Scratch,
}

impl MotionDetector {
    /// A detector for grids of `cols` x `rows` luma bytes, row-major. A grid
    /// with a side under 8 cells never produces an event.
    pub(crate) fn new(cols: usize, rows: usize) -> Self {
        let n = cols.checked_mul(rows).unwrap_or(0);
        Self {
            cols,
            rows,
            n,
            enabled: cols >= MIN_SIDE && rows >= MIN_SIDE && n > 0,
            ring: VecDeque::new(),
            spare: Vec::new(),
            burst: None,
            busy: false,
            overlay: None,
            moves: Vec::new(),
            events: Vec::new(),
            scratch: Scratch {
                mask: Vec::new(),
                stack: Vec::new(),
            },
        }
    }

    /// Feeds one frame. `luma` must hold `cols * rows` bytes (a frame with
    /// another length is ignored). `is_cut` marks a frame the cut detector
    /// calls a cut: the history restarts there and no event is reported on it.
    pub(crate) fn push(&mut self, frame_index: u64, time_ms: u64, luma: &[u8], is_cut: bool) {
        if !self.enabled || luma.len() != self.n {
            return;
        }
        if is_cut {
            self.end_shot();
            self.store(frame_index, time_ms, luma);
            return;
        }
        let Some(prev) = self.ring.back() else {
            self.store(frame_index, time_ms, luma);
            return;
        };
        let time_ms = time_ms.max(prev.time_ms);
        let base_pos = pos_at_or_before(&self.ring, time_ms.saturating_sub(BASE_MS)).unwrap_or(0);
        let base = self.ring.get(base_pos).unwrap_or(prev);
        let (base_index, base_time) = (base.index, base.time_ms);
        let diff = mean_abs_diff(luma, &base.luma);
        self.store(frame_index, time_ms, luma);
        self.track_burst(diff, (base_index, base_time), frame_index, time_ms);
        self.resolve_due(time_ms);
        if self.overlay.is_none() {
            self.look_for_overlay(frame_index, time_ms);
        }
    }

    /// Ends the clip and returns the events sorted by time. An overlay still
    /// waiting for its persistence time is dropped; a move waiting for its
    /// margins is judged on the frames there are.
    pub(crate) fn finish(mut self) -> Vec<DetectedEvent> {
        self.end_shot();
        let mut events = std::mem::take(&mut self.events);
        events.sort_by_key(|e| (e.time_ms, e.frame_index));
        events
    }

    // ---- bookkeeping ----

    fn store(&mut self, index: u64, time_ms: u64, luma: &[u8]) {
        let mut buf = self.spare.pop().unwrap_or_default();
        buf.clear();
        buf.extend_from_slice(luma);
        self.ring.push_back(Frame {
            index,
            time_ms,
            luma: buf,
        });
        while let Some(front) = self.ring.front() {
            if self.ring.len() > MAX_FRAMES || front.time_ms + HISTORY_MS < time_ms {
                if let Some(old) = self.ring.pop_front() {
                    self.spare.push(old.luma);
                }
            } else {
                break;
            }
        }
    }

    /// A cut (or the end): judge what is waiting on the frames there are, and
    /// forget the shot.
    fn end_shot(&mut self) {
        if let Some(burst) = self.burst.take() {
            self.close_burst(burst);
        }
        self.busy = false;
        for pending in std::mem::take(&mut self.moves) {
            self.resolve_move(pending);
        }
        self.overlay = None;
        while let Some(old) = self.ring.pop_front() {
            self.spare.push(old.luma);
        }
    }

    fn resolve_due(&mut self, now: u64) {
        if self.moves.iter().any(|m| m.resolve_at <= now) {
            let (due, waiting): (Vec<PendingMove>, Vec<PendingMove>) =
                std::mem::take(&mut self.moves)
                    .into_iter()
                    .partition(|m| m.resolve_at <= now);
            self.moves = waiting;
            for pending in due {
                self.resolve_move(pending);
            }
        }
        if let Some(pending) = self.overlay {
            if pending.verify_at <= now {
                self.overlay = None;
                self.verify_overlay(pending);
            }
        }
    }

    // ---- scale ----

    fn track_burst(&mut self, diff: f32, anchor: (u64, u64), index: u64, time: u64) {
        if wants_fit(diff) {
            match self.burst {
                Some(mut burst) => {
                    burst.last_index = index;
                    burst.last_time = time;
                    if time.saturating_sub(burst.first_time) > MAX_RUN_MS {
                        self.burst = None;
                        self.busy = true;
                    } else {
                        self.burst = Some(burst);
                    }
                }
                None if !self.busy => {
                    self.burst = Some(Burst {
                        anchor_index: anchor.0,
                        anchor_time: anchor.1,
                        first_time: time,
                        last_index: index,
                        last_time: time,
                    });
                }
                None => {}
            }
        } else {
            if let Some(burst) = self.burst.take() {
                self.close_burst(burst);
            }
            self.busy = false;
        }
    }

    /// The run ended: fit one scale between its first and last grid.
    fn close_burst(&mut self, burst: Burst) {
        let Some(fit) = self.fit_burst(&burst) else {
            return;
        };
        let (end_index, end_time) = self.settled(&burst);
        self.moves.push(PendingMove {
            anchor_index: burst.anchor_index,
            anchor_time: burst.anchor_time,
            end_index,
            end_time,
            ln: fit.ln,
            cx: fit.cx,
            cy: fit.cy,
            resolve_at: burst.last_time + EXT_MS,
        });
    }

    /// The first grid of the run that is within the floor of its last one:
    /// the frame where the move is complete.
    fn settled(&self, burst: &Burst) -> (u64, u64) {
        let last = (burst.last_index, burst.last_time);
        let (Some(from), Some(to)) = (
            pos_of_index(&self.ring, burst.anchor_index),
            pos_of_index(&self.ring, burst.last_index),
        ) else {
            return last;
        };
        let Some(end) = self.ring.get(to) else {
            return last;
        };
        for pos in from + 1..to {
            if let Some(frame) = self.ring.get(pos) {
                if !wants_fit(mean_abs_diff(&frame.luma, &end.luma)) {
                    return (frame.index, frame.time_ms);
                }
            }
        }
        last
    }

    fn fit_burst(&self, burst: &Burst) -> Option<Fit> {
        let a = self
            .ring
            .get(pos_of_index(&self.ring, burst.anchor_index)?)?;
        let b = self.ring.get(pos_of_index(&self.ring, burst.last_index)?)?;
        let geom = Geom {
            cols: self.cols,
            rows: self.rows,
        };
        fit_move(&a.luma, &b.luma, geom)
    }

    /// A move's margins are in: the picture must be still before and after.
    fn resolve_move(&mut self, pending: PendingMove) {
        let Some(after) = self.ring.back() else {
            return;
        };
        let Some(anchor_pos) = pos_of_index(&self.ring, pending.anchor_index) else {
            return;
        };
        let before_pos = pos_at_or_before(&self.ring, pending.anchor_time.saturating_sub(EXT_MS))
            .unwrap_or(0)
            .min(anchor_pos);
        let Some(before) = self.ring.get(before_pos) else {
            return;
        };
        let geom = Geom {
            cols: self.cols,
            rows: self.rows,
        };
        let stride = geom.fine_stride();
        let centre = (pending.cx, pending.cy);
        let (coarse, _) = scan_scale(&before.luma, &after.luma, geom, centre, stride, 0.01);
        let mut best = (
            coarse,
            cost(&before.luma, &after.luma, geom, coarse, centre, stride),
        );
        for k in -4i32..=4 {
            let ln = coarse + k as f32 * 0.0025;
            let c = cost(&before.luma, &after.luma, geom, ln, centre, stride);
            if c < best.1 {
                best = (ln, c);
            }
        }
        let base = cost(&before.luma, &after.luma, geom, 0.0, centre, stride);
        if best.1 > BEST_RATIO * base {
            return;
        }
        let ratio = best.0 / pending.ln;
        if !(EXT_RATIO_MIN..=EXT_RATIO_MAX).contains(&ratio) {
            return;
        }
        let scale = f64::from(best.0).exp();
        if (scale - 1.0).abs() < MIN_SCALE_DELTA {
            return;
        }
        self.events.push(DetectedEvent {
            time_ms: pending.end_time,
            frame_index: pending.end_index,
            kind: if scale > 1.0 {
                MotionKind::PunchIn
            } else {
                MotionKind::ZoomOut
            },
            scale: Some(scale),
            center_x: Some(f64::from(pending.cx)),
            center_y: Some(f64::from(pending.cy)),
            area: None,
        });
    }

    // ---- overlay ----

    fn look_for_overlay(&mut self, index: u64, time: u64) {
        let Some(cur) = self.ring.back() else {
            return;
        };
        let Some(pos) = pos_at_or_before(&self.ring, time.saturating_sub(LOOKBACK_MS)) else {
            return;
        };
        let Some(reference) = self.ring.get(pos) else {
            return;
        };
        if reference.index == cur.index {
            return;
        }
        let changed = count_changed(&cur.luma, &reference.luma) as f64;
        let cells = self.n as f64;
        if changed < MIN_AREA * cells || changed > MAX_AREA * cells {
            return;
        }
        self.overlay = Some(PendingOverlay {
            cand_index: index,
            cand_time: time,
            ref_index: reference.index,
            verify_at: time + PERSIST_MS,
        });
    }

    /// The persistence time has passed: does the change still hold, in one
    /// block, with the rest quiet, and was it not a flash coming back?
    fn verify_overlay(&mut self, pending: PendingOverlay) {
        let Some(now) = self.ring.back() else {
            return;
        };
        let Some(reference_pos) = pos_of_index(&self.ring, pending.ref_index) else {
            return;
        };
        let Some(reference) = self.ring.get(reference_pos) else {
            return;
        };
        let Some(block) = analyse(
            &mut self.scratch,
            &now.luma,
            &reference.luma,
            self.cols,
            self.rows,
        ) else {
            return;
        };
        let cells = self.n as f64;
        let size = block.size as f64;
        let area = size / cells;
        if !(MIN_AREA..=MAX_AREA).contains(&area) {
            return;
        }
        if block.others as f64 > OTHER_SHARE * size + OTHER_FLOOR * cells {
            return;
        }
        let bbox = ((block.max_x - block.min_x + 1) * (block.max_y - block.min_y + 1)) as f64;
        if size / bbox < MIN_FILL {
            return;
        }
        // Held: the block was already changed a little before the check.
        let held_pos = pos_at_or_before(&self.ring, now.time_ms.saturating_sub(STABLE_MS))
            .unwrap_or(self.ring.len() - 1);
        let Some(held) = self.ring.get(held_pos) else {
            return;
        };
        let held_cells = changed_in_block(&self.scratch.mask, &held.luma, &reference.luma);
        if (held_cells as f64) < HOLD_SHARE * size {
            return;
        }
        // Still before: the block was not already changing in the comparison
        // grid.
        let quiet_time = reference.time_ms.saturating_sub(QUIET_MS);
        if let Some(quiet) = pos_at_or_before(&self.ring, quiet_time)
            .filter(|p| *p != reference_pos)
            .and_then(|p| self.ring.get(p))
        {
            let moving = moved_in_block(&self.scratch.mask, &reference.luma, &quiet.luma);
            if moving as f64 > QUIET_SHARE * size {
                return;
            }
        }
        // A flash coming back: the block now looks like the grid from before
        // the change window.
        let before_time = pending.cand_time.saturating_sub(LOOKBACK_MS + PERSIST_MS);
        let pre_pos = pos_at_or_before(&self.ring, before_time).or_else(|| {
            self.ring
                .front()
                .filter(|f| f.time_ms < reference.time_ms)
                .map(|_| 0)
        });
        if let Some(pre) = pre_pos.and_then(|p| self.ring.get(p)) {
            let same = same_in_block(&self.scratch.mask, &now.luma, &pre.luma);
            if same as f64 >= REVERT_SHARE * size {
                return;
            }
        }
        // The event is the first grid that holds the change in full.
        let total = block_sum(&self.scratch.mask, &now.luma, &reference.luma);
        let from = pos_of_index(&self.ring, pending.cand_index).unwrap_or(reference_pos + 1);
        let mut at = self.ring.len() - 1;
        for pos in from..self.ring.len() {
            if let Some(frame) = self.ring.get(pos) {
                let sum = block_sum(&self.scratch.mask, &frame.luma, &reference.luma);
                if sum as f64 >= FULL_SHARE * total as f64 {
                    at = pos;
                    break;
                }
            }
        }
        let Some(frame) = self.ring.get(at) else {
            return;
        };
        self.events.push(DetectedEvent {
            time_ms: frame.time_ms,
            frame_index: frame.index,
            kind: MotionKind::OverlayChange,
            scale: None,
            center_x: None,
            center_y: None,
            area: Some(area),
        });
    }
}

/// Whether a frame moves enough that a scale may explain it.
fn wants_fit(mean_diff: f32) -> bool {
    mean_diff >= SCALE_FLOOR
}

fn pos_at_or_before(ring: &VecDeque<Frame>, time: u64) -> Option<usize> {
    ring.iter().rposition(|f| f.time_ms <= time)
}

fn pos_of_index(ring: &VecDeque<Frame>, index: u64) -> Option<usize> {
    ring.iter().rposition(|f| f.index == index)
}

fn mean_abs_diff(a: &[u8], b: &[u8]) -> f32 {
    let mut total = 0u64;
    for (ca, cb) in a.chunks(4096).zip(b.chunks(4096)) {
        let mut sum = 0u32;
        for (x, y) in ca.iter().zip(cb) {
            sum += u32::from(x.abs_diff(*y));
        }
        total += u64::from(sum);
    }
    total as f32 / a.len().max(1) as f32
}

fn count_changed(a: &[u8], b: &[u8]) -> usize {
    let mut total = 0usize;
    for (ca, cb) in a.chunks(4096).zip(b.chunks(4096)) {
        let mut count = 0u32;
        for (x, y) in ca.iter().zip(cb) {
            count += u32::from(x.abs_diff(*y) >= LEVEL);
        }
        total += count as usize;
    }
    total
}

// ---- connected blocks ----

struct Block {
    size: usize,
    others: usize,
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
}

/// Finds the largest connected block of cells that differ by `LEVEL` or more
/// and leaves its cells marked 3 in the scratch mask.
fn analyse(
    scratch: &mut Scratch,
    cur: &[u8],
    reference: &[u8],
    cols: usize,
    rows: usize,
) -> Option<Block> {
    scratch.mask.clear();
    scratch.mask.extend(
        cur.iter()
            .zip(reference)
            .map(|(a, b)| u8::from(a.abs_diff(*b) >= LEVEL)),
    );
    let total = scratch.mask.iter().filter(|m| **m == 1).count();
    let mut best_size = 0usize;
    let mut best_start = None;
    for start in 0..scratch.mask.len() {
        if scratch.mask[start] == 1 {
            let found = flood(scratch, start, 1, 2, cols, rows);
            if found.size > best_size {
                best_size = found.size;
                best_start = Some(start);
            }
        }
    }
    let start = best_start?;
    let mut block = flood(scratch, start, 2, 3, cols, rows);
    block.others = total - best_size;
    Some(block)
}

fn flood(scratch: &mut Scratch, start: usize, from: u8, to: u8, cols: usize, rows: usize) -> Block {
    let mut block = Block {
        size: 0,
        others: 0,
        min_x: usize::MAX,
        max_x: 0,
        min_y: usize::MAX,
        max_y: 0,
    };
    scratch.stack.clear();
    scratch.stack.push(start);
    scratch.mask[start] = to;
    while let Some(cell) = scratch.stack.pop() {
        let (x, y) = (cell % cols, cell / cols);
        block.size += 1;
        block.min_x = block.min_x.min(x);
        block.max_x = block.max_x.max(x);
        block.min_y = block.min_y.min(y);
        block.max_y = block.max_y.max(y);
        for ny in y.saturating_sub(1)..=(y + 1).min(rows - 1) {
            for nx in x.saturating_sub(1)..=(x + 1).min(cols - 1) {
                let at = ny * cols + nx;
                if scratch.mask[at] == from {
                    scratch.mask[at] = to;
                    scratch.stack.push(at);
                }
            }
        }
    }
    block
}

fn changed_in_block(mask: &[u8], a: &[u8], b: &[u8]) -> usize {
    mask.iter()
        .zip(a.iter().zip(b))
        .filter(|(m, (x, y))| **m == 3 && x.abs_diff(**y) >= LEVEL)
        .count()
}

fn moved_in_block(mask: &[u8], a: &[u8], b: &[u8]) -> usize {
    mask.iter()
        .zip(a.iter().zip(b))
        .filter(|(m, (x, y))| **m == 3 && x.abs_diff(**y) >= QUIET_LEVEL)
        .count()
}

fn same_in_block(mask: &[u8], a: &[u8], b: &[u8]) -> usize {
    mask.iter()
        .zip(a.iter().zip(b))
        .filter(|(m, (x, y))| **m == 3 && x.abs_diff(**y) < LEVEL)
        .count()
}

fn block_sum(mask: &[u8], a: &[u8], b: &[u8]) -> u64 {
    mask.iter()
        .zip(a.iter().zip(b))
        .filter(|(m, _)| **m == 3)
        .map(|(_, (x, y))| u64::from(x.abs_diff(*y)))
        .sum()
}

// ---- scale fit ----

#[derive(Clone, Copy)]
struct Geom {
    cols: usize,
    rows: usize,
}

impl Geom {
    /// About 1600 sample points per fit for the coarse pass.
    fn coarse_stride(&self) -> usize {
        let cells = self.cols * self.rows;
        let mut stride = 1usize;
        while cells / (stride * stride) > 1600 {
            stride += 1;
        }
        stride
    }

    /// About 3600 sample points per fit for the refinement.
    fn fine_stride(&self) -> usize {
        let cells = self.cols * self.rows;
        let mut stride = 1usize;
        while cells / (stride * stride) > 3600 {
            stride += 1;
        }
        stride
    }
}

#[derive(Clone, Copy)]
struct Fit {
    ln: f32,
    cx: f32,
    cy: f32,
    /// The mean difference at this scale and centre.
    cost: f32,
}

// Times the pattern search ran (tests check that the cheap gate keeps it off
// motion that is no scale).
#[cfg(test)]
thread_local! {
    static REFINES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Mean absolute difference between `b` and `a` magnified by `exp(ln)` about
/// `centre` (shares of the frame), sampled every `stride` cells. A negative
/// `ln` magnifies `b` into `a` instead, so every sample reads inside the
/// source grid.
fn cost(a: &[u8], b: &[u8], geom: Geom, ln: f32, centre: (f32, f32), stride: usize) -> f32 {
    let (source, target, ln) = if ln >= 0.0 { (a, b, ln) } else { (b, a, -ln) };
    let inv = (-ln).exp();
    let (cols, rows) = (geom.cols, geom.rows);
    let max_x = (cols - 1) as f32;
    let max_y = (rows - 1) as f32;
    let (ccx, ccy) = (centre.0 * max_x, centre.1 * max_y);
    let mut sum = 0f32;
    let mut count = 0u32;
    for y in (0..rows).step_by(stride) {
        let sy = (ccy + (y as f32 - ccy) * inv).clamp(0.0, max_y);
        let y0 = (sy as usize).min(rows - 2);
        let fy = sy - y0 as f32;
        let top = &source[y0 * cols..(y0 + 1) * cols];
        let bottom = &source[(y0 + 1) * cols..(y0 + 2) * cols];
        let row = &target[y * cols..(y + 1) * cols];
        for x in (0..cols).step_by(stride) {
            let sx = (ccx + (x as f32 - ccx) * inv).clamp(0.0, max_x);
            let x0 = (sx as usize).min(cols - 2);
            let fx = sx - x0 as f32;
            let upper = f32::from(top[x0]) * (1.0 - fx) + f32::from(top[x0 + 1]) * fx;
            let lower = f32::from(bottom[x0]) * (1.0 - fx) + f32::from(bottom[x0 + 1]) * fx;
            let value = upper * (1.0 - fy) + lower * fy;
            sum += (f32::from(row[x]) - value).abs();
            count += 1;
        }
    }
    sum / count.max(1) as f32
}

/// The best ln scale at a fixed centre between `-LN_MAX` and `LN_MAX`, and
/// its cost.
fn scan_scale(
    a: &[u8],
    b: &[u8],
    geom: Geom,
    centre: (f32, f32),
    stride: usize,
    step: f32,
) -> (f32, f32) {
    let mut best = (0.0f32, cost(a, b, geom, 0.0, centre, stride));
    let steps = (LN_MAX / step).floor() as i32;
    for k in -steps..=steps {
        let ln = k as f32 * step;
        let c = cost(a, b, geom, ln, centre, stride);
        if c < best.1 {
            best = (ln, c);
        }
    }
    best
}

/// Fits a scale and its fixed point between two grids: a coarse search over
/// scales and nine centres, a gate (`None` if no scale comes close to
/// explaining the difference), then a pattern search. The caller judges the
/// refined fit against the grids it cares about.
fn fit_move(a: &[u8], b: &[u8], geom: Geom) -> Option<Fit> {
    let coarse = geom.coarse_stride();
    let base_coarse = cost(a, b, geom, 0.0, (0.5, 0.5), coarse);
    let mut best = Fit {
        ln: 0.0,
        cx: 0.5,
        cy: 0.5,
        cost: base_coarse,
    };
    for ln in COARSE_LN {
        for cy in [0.0f32, 0.5, 1.0] {
            for cx in [0.0f32, 0.5, 1.0] {
                let c = cost(a, b, geom, ln, (cx, cy), coarse);
                if c < best.cost {
                    best = Fit {
                        ln,
                        cx,
                        cy,
                        cost: c,
                    };
                }
            }
        }
    }
    if best.cost > GATE_RATIO * base_coarse {
        return None;
    }
    #[cfg(test)]
    REFINES.with(|n| n.set(n.get() + 1));
    let fine = geom.fine_stride();
    best.cost = cost(a, b, geom, best.ln, (best.cx, best.cy), fine);
    for (d_ln, d_c) in REFINE {
        for _ in 0..2 {
            let mut next = best;
            for i in -1i32..=1 {
                for j in -1i32..=1 {
                    for k in -1i32..=1 {
                        if i == 0 && j == 0 && k == 0 {
                            continue;
                        }
                        let ln = (best.ln + i as f32 * d_ln).clamp(-LN_MAX - 0.02, LN_MAX + 0.02);
                        let cx = (best.cx + j as f32 * d_c).clamp(0.0, 1.0);
                        let cy = (best.cy + k as f32 * d_c).clamp(0.0, 1.0);
                        let c = cost(a, b, geom, ln, (cx, cy), fine);
                        if c < next.cost {
                            next = Fit {
                                ln,
                                cx,
                                cy,
                                cost: c,
                            };
                        }
                    }
                }
            }
            if next.cost < best.cost - 1e-4 {
                best = next;
            } else {
                break;
            }
        }
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHAPES: [(usize, usize); 2] = [(160, 90), (180, 91)];
    const FPS: u64 = 24;

    // The blocks below move cells by 100 levels, far above `LEVEL`.
    const _: () = assert!(LEVEL < 100);

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

    fn time_at(i: usize, fps: u64) -> u64 {
        (i as u64 * 1000 + fps / 2) / fps
    }

    fn time_of(i: usize) -> u64 {
        time_at(i, FPS)
    }

    fn run_at(cols: usize, rows: usize, clip: &Clip, fps: u64) -> Vec<DetectedEvent> {
        let mut det = MotionDetector::new(cols, rows);
        for (i, frame) in clip.frames.iter().enumerate() {
            det.push(i as u64, time_at(i, fps), frame, clip.cuts.contains(&i));
        }
        det.finish()
    }

    fn run(cols: usize, rows: usize, clip: &Clip) -> Vec<DetectedEvent> {
        run_at(cols, rows, clip, FPS)
    }

    /// Cells in the mask move `delta` levels away from the middle grey.
    fn with_delta(base: &[u8], mask: &[bool], delta: u8) -> Vec<u8> {
        base.iter()
            .zip(mask)
            .map(|(v, m)| match (*m, *v < 128) {
                (false, _) => *v,
                (true, true) => v.saturating_add(delta),
                (true, false) => v.saturating_sub(delta),
            })
            .collect()
    }

    /// A flat grey scene with a textured subject in the middle, seen at `scale`.
    fn render_subject(cols: usize, rows: usize, scale: f32) -> Vec<u8> {
        let mut out = Vec::with_capacity(cols * rows);
        for y in 0..rows {
            let v = y as f32 / (rows - 1) as f32;
            for x in 0..cols {
                let u = x as f32 / (cols - 1) as f32;
                let (su, sv) = (0.5 + (u - 0.5) / scale, 0.5 + (v - 0.5) / scale);
                let inside = (0.35..0.65).contains(&su) && (0.3..0.7).contains(&sv);
                // Fine detail, so that magnifying the subject changes every
                // cell of it and not just its edges.
                out.push(if inside {
                    texture(su * 2.5, sv * 2.5) as u8
                } else {
                    128
                });
            }
        }
        out
    }

    /// Cells of `mask` set to a new pseudo-random value on every call.
    fn with_animation(base: &[u8], mask: &[bool], seed: u32) -> Vec<u8> {
        base.iter()
            .zip(mask)
            .enumerate()
            .map(|(i, (v, m))| {
                if *m {
                    (hash(i as i32, seed as i32, 5) * 255.0) as u8
                } else {
                    *v
                }
            })
            .collect()
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
        assert!(
            (cx - centre.0).abs() <= centre_tol,
            "cx {cx} vs {}",
            centre.0
        );
        assert!(
            (cy - centre.1).abs() <= centre_tol,
            "cy {cy} vs {}",
            centre.1
        );
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
    fn a_block_on_busy_footage_is_still_one_overlay() {
        // Noise of +-3 keeps every frame above the scale floor, so no scale is
        // ever fitted; overlays must not depend on that.
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            let blk = with_block(&base, &block_mask(cols, rows));
            let mut clip = Clip::default();
            for i in 0..80 {
                let src = if i < 30 { &base } else { &blk };
                clip.one(noisy(src, i as u32, 3));
            }
            let events = run(cols, rows, &clip);
            assert_eq!(events.len(), 1, "{cols}x{rows}: {events:?}");
            assert_eq!(events[0].kind, MotionKind::OverlayChange);
            assert_eq!(events[0].frame_index, 30);
        }
    }

    #[test]
    fn the_level_separates_a_caption_from_a_faint_shadow() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let mask = block_mask(cols, rows);
        let mut faint = Clip::default();
        faint
            .hold(&base, 30)
            .hold(&with_delta(&base, &mask, 15), 50);
        assert!(run(cols, rows, &faint).is_empty());
        let mut medium = Clip::default();
        medium
            .hold(&base, 30)
            .hold(&with_delta(&base, &mask, 40), 50);
        let events = run(cols, rows, &medium);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].frame_index, 30);
    }

    #[test]
    fn the_area_range_runs_from_two_percent_to_a_third() {
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            for (x0, y0, x1, y1) in [(0.6, 0.6, 0.74, 0.74), (0.1, 0.1, 0.7, 0.68)] {
                let mask = rect(cols, rows, x0, y0, x1, y1);
                let mut clip = Clip::default();
                clip.hold(&base, 30).hold(&with_block(&base, &mask), 50);
                let events = run(cols, rows, &clip);
                assert_eq!(events.len(), 1, "{cols}x{rows} {x0}: {events:?}");
                let want = count(&mask) as f64 / (cols * rows) as f64;
                assert!((events[0].area.expect("area") - want).abs() < 0.005);
            }
        }
    }

    /// Single changed cells every `step` cells, away from any block.
    fn strays(cols: usize, rows: usize, step: usize) -> Vec<bool> {
        let mut mask = vec![false; cols * rows];
        for y in (0..rows).step_by(step) {
            for x in (0..cols).step_by(step) {
                mask[y * cols + x] = true;
            }
        }
        mask
    }

    fn union(a: &[bool], b: &[bool]) -> Vec<bool> {
        a.iter().zip(b).map(|(x, y)| *x || *y).collect()
    }

    #[test]
    fn a_few_stray_cells_do_not_spoil_a_block() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        // About 1.6% of the frame strays beside a 7.5% block...
        let big = union(&block_mask(cols, rows), &strays(cols, rows, 8));
        let mut clip = Clip::default();
        clip.hold(&base, 30).hold(&with_block(&base, &big), 50);
        let events = run(cols, rows, &clip);
        assert_eq!(events.len(), 1, "{events:?}");
        // ...and 0.8% beside a 2% block.
        let small_block = rect(cols, rows, 0.6, 0.6, 0.74, 0.74);
        let small = union(&small_block, &strays(cols, rows, 11));
        let mut clip = Clip::default();
        clip.hold(&base, 30).hold(&with_block(&base, &small), 50);
        let events = run(cols, rows, &clip);
        assert_eq!(events.len(), 1, "{events:?}");
    }

    #[test]
    fn a_block_under_one_percent_with_strays_is_no_overlay() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let tiny = rect(cols, rows, 0.6, 0.6, 0.7, 0.68);
        let mask = union(&tiny, &strays(cols, rows, 16));
        let mut clip = Clip::default();
        clip.hold(&base, 30).hold(&with_block(&base, &mask), 50);
        let events = run(cols, rows, &clip);
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn a_slow_fade_over_half_a_second_is_still_an_overlay() {
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            let blk = with_block(&base, &block_mask(cols, rows));
            let mut clip = Clip::default();
            clip.hold(&base, 30);
            for k in 1..=12 {
                clip.one(blend(&base, &blk, k as f32 / 12.0));
            }
            clip.hold(&blk, 50);
            let events = run(cols, rows, &clip);
            assert_eq!(events.len(), 1, "{cols}x{rows}: {events:?}");
            assert!(
                (38..=42).contains(&events[0].frame_index),
                "frame {}",
                events[0].frame_index
            );
        }
    }

    #[test]
    fn a_noisy_fade_still_lands_on_the_end_of_the_fade() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let blk = with_block(&base, &block_mask(cols, rows));
        let mut clip = Clip::default();
        for i in 0..30 {
            clip.one(noisy(&base, i, 1));
        }
        for k in 1..=6 {
            clip.one(noisy(&blend(&base, &blk, k as f32 / 6.0), 30 + k, 1));
        }
        for i in 0..50 {
            clip.one(noisy(&blk, 40 + i, 1));
        }
        let events = run(cols, rows, &clip);
        assert_eq!(events.len(), 1, "{events:?}");
        assert!(
            (34..=36).contains(&events[0].frame_index),
            "frame {}",
            events[0].frame_index
        );
    }

    #[test]
    fn a_block_that_jumps_elsewhere_inside_the_persistence_time_is_no_event() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let first = with_block(&base, &rect(cols, rows, 0.55, 0.55, 0.80, 0.85));
        let second = with_block(&base, &rect(cols, rows, 0.10, 0.10, 0.35, 0.40));
        let mut clip = Clip::default();
        clip.hold(&base, 30).hold(&first, 10).hold(&second, 60);
        let events = run(cols, rows, &clip);
        assert!(events.is_empty(), "{events:?}");
    }

    #[test]
    fn a_card_that_animates_inside_is_one_overlay() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let mask = block_mask(cols, rows);
        let mut clip = Clip::default();
        clip.hold(&base, 30);
        for i in 0..60 {
            clip.one(with_animation(&base, &mask, i));
        }
        let events = run(cols, rows, &clip);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].kind, MotionKind::OverlayChange);
        assert!(
            (30..=33).contains(&events[0].frame_index),
            "frame {}",
            events[0].frame_index
        );
    }

    #[test]
    fn a_punch_in_over_a_flat_scene_is_not_also_an_overlay() {
        for (cols, rows) in SHAPES {
            let mut clip = Clip::default();
            clip.hold(&render_subject(cols, rows, 1.0), 30);
            for s in ease(1.0, 1.18, 6, true) {
                clip.one(render_subject(cols, rows, s));
            }
            clip.hold(&render_subject(cols, rows, 1.18), 40);
            let events = run(cols, rows, &clip);
            assert_eq!(events.len(), 1, "{cols}x{rows}: {events:?}");
            assert_eq!(events[0].kind, MotionKind::PunchIn);
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
                assert_one_move(
                    &events,
                    MotionKind::PunchIn,
                    1.18,
                    (f64::from(cx), f64::from(cy)),
                    0.15,
                );
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
            let clip = zoom_clip(
                cols,
                rows,
                1.0,
                &ease(1.0, 1.18, 6, true),
                (0.5, 0.5),
                30,
                40,
            );
            let events = run(cols, rows, &clip);
            let e = assert_one_move(&events, MotionKind::PunchIn, 1.18, (0.5, 0.5), 0.1);
            assert!(
                (34..=36).contains(&e.frame_index),
                "frame {}",
                e.frame_index
            );
        }
    }

    #[test]
    fn a_linear_move_over_twelve_frames_is_still_one_event() {
        let (cols, rows) = (160, 90);
        let clip = zoom_clip(
            cols,
            rows,
            1.0,
            &ease(1.0, 1.18, 12, false),
            (0.5, 0.5),
            30,
            40,
        );
        let events = run(cols, rows, &clip);
        assert_one_move(&events, MotionKind::PunchIn, 1.18, (0.5, 0.5), 0.1);
    }

    #[test]
    fn a_slow_push_in_is_no_event() {
        for (cols, rows) in SHAPES {
            for (steps, smooth) in [
                (17, true),
                (18, true),
                (24, true),
                (36, true),
                (48, true),
                (14, false),
                (16, false),
                (20, false),
                (30, false),
                (60, false),
            ] {
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
            vec![
                MotionKind::PunchIn,
                MotionKind::ZoomOut,
                MotionKind::OverlayChange
            ],
            "{events:?}"
        );
        assert!(events.windows(2).all(|w| w[0].time_ms < w[1].time_ms));
    }

    #[test]
    fn the_motion_that_is_no_scale_never_reaches_the_pattern_search() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let blk = with_block(&base, &block_mask(cols, rows));
        let mut clip = Clip::default();
        clip.hold(&base, 30)
            .hold(&blk, 30)
            .hold(&shifted(&base, 50), 30);
        for k in 1..=6 {
            clip.one(shifted(&base, -8 * k));
        }
        clip.hold(&base, 30);
        let before = REFINES.with(std::cell::Cell::get);
        let _ = run(cols, rows, &clip);
        assert_eq!(REFINES.with(std::cell::Cell::get), before);
        // A real scale does reach it.
        let zoom = zoom_clip(cols, rows, 1.0, &[1.18], (0.5, 0.5), 30, 40);
        let _ = run(cols, rows, &zoom);
        assert!(REFINES.with(std::cell::Cell::get) > before);
    }

    #[test]
    fn a_move_in_the_frame_right_after_a_cut_is_found() {
        for (cols, rows) in SHAPES {
            let a = render(cols, rows, 1.0, 0.5, 0.5);
            let b = render(cols, rows, 1.0, 0.8, 0.2);
            let mut clip = Clip::default();
            clip.hold(&a, 20).cut(&b);
            clip.one(render(cols, rows, 1.18, 0.8, 0.2));
            clip.hold(&render(cols, rows, 1.18, 0.8, 0.2), 40);
            let events = run(cols, rows, &clip);
            assert_eq!(events.len(), 1, "{cols}x{rows}: {events:?}");
            assert_eq!(events[0].frame_index, 21);
        }
    }

    #[test]
    fn a_fade_over_three_quarters_of_a_second_is_one_overlay() {
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            let blk = with_block(&base, &block_mask(cols, rows));
            let mut clip = Clip::default();
            clip.hold(&base, 30);
            for k in 1..=18 {
                clip.one(blend(&base, &blk, k as f32 / 18.0));
            }
            clip.hold(&blk, 50);
            let events = run(cols, rows, &clip);
            assert_eq!(events.len(), 1, "{cols}x{rows}: {events:?}");
            assert!(
                (40..=47).contains(&events[0].frame_index),
                "frame {}",
                events[0].frame_index
            );
        }
    }

    #[test]
    fn a_card_that_stops_animating_and_then_changes_is_two_overlays() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let mask = block_mask(cols, rows);
        let mut clip = Clip::default();
        clip.hold(&base, 10);
        for i in 0..20 {
            clip.one(with_animation(&base, &mask, i));
        }
        clip.hold(&with_animation(&base, &mask, 19), 20);
        clip.hold(&with_animation(&base, &mask, 999), 60);
        let events = run(cols, rows, &clip);
        assert_eq!(events.len(), 2, "{events:?}");
        assert!((10..=13).contains(&events[0].frame_index), "{events:?}");
        assert_eq!(events[1].frame_index, 50, "{events:?}");
    }

    #[test]
    fn a_caption_over_a_part_that_moves_is_one_overlay() {
        let (cols, rows) = (160, 90);
        let base = render(cols, rows, 1.0, 0.5, 0.5);
        let mask = block_mask(cols, rows);
        let flicker = rect(cols, rows, 0.6, 0.6, 0.64, 0.7);
        // A seed for which the part differs between the grids the detector
        // compares (frames 24 and 20).
        let on = |i: usize, seed: i32| hash(i as i32, seed, 9) < 0.5;
        let seed = (0..50).find(|s| on(24, *s) != on(20, *s)).expect("a seed");
        let mut clip = Clip::default();
        for i in 0..80 {
            let frame = if on(i, seed) {
                with_delta(&base, &flicker, 60)
            } else {
                base.clone()
            };
            clip.one(if i < 30 {
                frame
            } else {
                with_block(&frame, &mask)
            });
        }
        let events = run(cols, rows, &clip);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].frame_index, 30);
    }

    fn low_contrast(frame: &[u8]) -> Vec<u8> {
        frame
            .iter()
            .map(|v| (128 + (i16::from(*v) - 128) / 4) as u8)
            .collect()
    }

    #[test]
    fn a_faint_punch_in_under_noise_is_found() {
        for (cols, rows) in SHAPES {
            let before = low_contrast(&render(cols, rows, 1.0, 0.5, 0.5));
            let after = low_contrast(&render(cols, rows, 1.18, 0.5, 0.5));
            let mut clip = Clip::default();
            for i in 0..80 {
                clip.one(noisy(if i < 30 { &before } else { &after }, i as u32, 1));
            }
            let events = run(cols, rows, &clip);
            assert_one_move(&events, MotionKind::PunchIn, 1.18, (0.5, 0.5), 0.15);
        }
    }

    #[test]
    fn the_largest_scale_tried_is_reached() {
        for (cols, rows) in SHAPES {
            let clip = zoom_clip(cols, rows, 1.0, &[1.24], (0.5, 0.5), 30, 40);
            let events = run(cols, rows, &clip);
            assert_one_move(&events, MotionKind::PunchIn, 1.24, (0.5, 0.5), 0.1);
            let clip = zoom_clip(cols, rows, 1.24, &[1.0], (0.5, 0.5), 30, 40);
            let events = run(cols, rows, &clip);
            assert_one_move(&events, MotionKind::ZoomOut, 1.0 / 1.24, (0.5, 0.5), 0.1);
        }
    }

    // ---- other frame rates ----

    #[test]
    fn moves_are_measured_in_time_not_in_frames() {
        let (cols, rows) = (160, 90);
        for fps in [30u64, 60] {
            // 0.25 s and 0.4 s moves are one event each.
            for seconds in [0.25f64, 0.4] {
                let steps = (seconds * fps as f64).round() as usize;
                let clip = zoom_clip(
                    cols,
                    rows,
                    1.0,
                    &ease(1.0, 1.18, steps, true),
                    (0.5, 0.5),
                    (fps * 2) as usize,
                    (fps * 2) as usize,
                );
                let events = run_at(cols, rows, &clip, fps);
                assert_eq!(events.len(), 1, "{fps} fps {seconds} s: {events:?}");
                assert_eq!(events[0].kind, MotionKind::PunchIn);
                let want = time_at(2 * fps as usize + steps - 1, fps);
                let got = events[0].time_ms;
                assert!(got + 150 >= want && got <= want + 40, "{got} vs {want}");
            }
            // A push-in over 1.25 s is none.
            let steps = (1.25 * fps as f64).round() as usize;
            for smooth in [true, false] {
                let clip = zoom_clip(
                    cols,
                    rows,
                    1.0,
                    &ease(1.0, 1.18, steps, smooth),
                    (0.5, 0.5),
                    (fps * 2) as usize,
                    (fps * 2) as usize,
                );
                let events = run_at(cols, rows, &clip, fps);
                assert!(events.is_empty(), "{fps} fps smooth {smooth}: {events:?}");
            }
        }
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
        clip.hold(&a, 30)
            .one(zoomed.clone())
            .hold(&zoomed, 2)
            .cut(&b)
            .hold(&b, 30);
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

    /// Per-frame cost of 1800 frames (75 s at 24 fps). Run with
    /// `cargo test -p decompile --release motion::tests::timing -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing report, run by hand in release mode"]
    fn timing() {
        use std::time::Instant;
        for (cols, rows) in SHAPES {
            let base = render(cols, rows, 1.0, 0.5, 0.5);
            let blk = with_block(&base, &block_mask(cols, rows));
            let noisy_set: Vec<Vec<u8>> = (0..16).map(|i| noisy(&base, i, 1)).collect();
            let busy_set: Vec<Vec<u8>> = (0..16).map(|i| noisy(&base, i, 3)).collect();
            let mut zoom_set = vec![base.clone()];
            for s in ease(1.0, 1.18, 6, true) {
                zoom_set.push(render(cols, rows, s, 0.5, 0.5));
            }
            let mut punch: Vec<&[u8]> = Vec::new();
            for i in 0..1800usize {
                let phase = i % 120;
                let zoomed = (i / 120) % 2 == 1;
                let k = (phase + 1).min(6);
                let idx = if phase >= 6 {
                    if zoomed {
                        0
                    } else {
                        6
                    }
                } else if zoomed {
                    6 - k
                } else {
                    k
                };
                punch.push(&zoom_set[idx]);
            }
            let cases: Vec<(&str, Vec<&[u8]>)> = vec![
                (
                    "still talking head (noise +-1)",
                    (0..1800).map(|i| noisy_set[i % 16].as_slice()).collect(),
                ),
                (
                    "busy footage (noise +-3)",
                    (0..1800).map(|i| busy_set[i % 16].as_slice()).collect(),
                ),
                (
                    "gestures: 4 noisy frames, 2 still, repeating",
                    (0..1800)
                        .map(|i| {
                            if i % 6 < 4 {
                                busy_set[i % 16].as_slice()
                            } else {
                                noisy_set[0].as_slice()
                            }
                        })
                        .collect(),
                ),
                ("punch-in out/in every 5 s", punch),
                (
                    "overlay on/off every 3 s",
                    (0..1800)
                        .map(|i| {
                            if (i / 72) % 2 == 1 {
                                blk.as_slice()
                            } else {
                                base.as_slice()
                            }
                        })
                        .collect(),
                ),
            ];
            for (name, pick) in &cases {
                let mut det = MotionDetector::new(cols, rows);
                let start = Instant::now();
                for i in 0..1800usize {
                    det.push(i as u64, time_of(i), pick[i], false);
                }
                let events = det.finish();
                let total = start.elapsed();
                println!(
                    "{cols}x{rows} {name}: {:.2} ms for 1800 frames = {:.4} ms/frame, {} events",
                    total.as_secs_f64() * 1000.0,
                    total.as_secs_f64() * 1000.0 / 1800.0,
                    events.len()
                );
            }
        }
    }
}
