# Spec: Zoom easing curves (`zoom-curves`)

Status: written 2026-10-28 under the user's blanket "Do all of them"; the user has not reviewed it line by line. The answers at the end are the recommended ones. Branch: `feature/zoom-curves`, cut from `feature/visual-motion` (it reads the detector's scale fits). Module `zoom-curves` in the [P0 capability map](p0-capability-map.md). Follows [planer.md](../../planer.md) Pass 1 ("exact camera punch-in ratios and cubic Bézier easing curves"). Depends on [visual-motion](visual-motion.md).

## Objective

`visual-motion` says that a punch-in happened and by how much. An editor who wants to copy it needs the feel of the move too: whether it snaps, eases in, overshoots, or settles. This module fits each measured `punch_in` and `zoom_out` with a cubic Bézier easing curve and writes it into `edit_dna.json`'s `zoom_curves`, which has been a `StubbedPass` ("unimplemented") since the first release.

A curve is `cubic-bezier(1/3, y1, 2/3, y2)` in CSS terms: the progress of the scale (0 at the start, 1 at the end of the move) against normalised time. The time control points are fixed at one third and two thirds, so the curve is a cubic polynomial in time and `y1`, `y2` are the only unknowns. A move lasts at most 12 frames, which is too few samples to fit four free numbers, and fixing the time side makes the two that remain identifiable and the result testable.

Not in this module: pans and rotations, slow drifts longer than 12 frames (a Ken Burns push-in), speed ramps, and the LUT ([cube-lut](cube-lut.md)).

## Evidence

- The detector already fits a scale between consecutive frames of a move (`fit_burst`, `fit_move` in `crates/decompile/src/motion.rs`) and joins them into one event at the frame where the move completes. The per-frame scales are computed and dropped.
- The fixtures plant a smoothstep ease over 6 frames (`motion12`); the detector reads the total scale to within 0.001 (1.1794 against 1.18). The per-frame track was checked by the fixture's author against the planted smoothstep (1.011, 1.045, 1.086, 1.133, 1.167, 1.178).
- `static_window` and the clipper's frame plan do not need curves; this is for the editor and for planer.md's "Edit DNA".

## Tech Stack

Rust workspace as today. No new dependencies. The track comes from `crates/decompile/src/motion.rs`, the fit is a small pure module beside it, the type is in `crates/edit-dna`.

## Contract

### Output

`zoom_curves` becomes an object with `status` (`measured` or `unimplemented`), `reason`, and `curves`; schema version 4. A file of version 3 or earlier reads with `status: unimplemented` and an empty list. A new file has `status: measured` and one entry in `curves` for each `punch_in` and `zoom_out` in `visual_events`, in the same order (`curves: []` when there are none; overlay changes have no curve).

```json
{"zoom_curves": {"status": "measured", "reason": "", "curves": [
  {"event_index": 0, "start_ms": 4000, "end_ms": 4250, "from_scale": 1.0, "to_scale": 1.1794,
   "bezier": [0.3333, 0.02, 0.6667, 1.08], "samples": 7, "fit_rms": 0.012}
]}}
```

- `event_index` points into `visual_events`. `start_ms` is the time of the last frame before the move (the anchor, as measured), `end_ms` the event's time.
- `from_scale` and `to_scale` are the scale at the anchor (1 by definition, so the field is the total factor's start, kept for readers that compose curves) and the measured total.
- `bezier` is `[x1, y1, x2, y2]` with `x1 = 1/3` and `x2 = 2/3`; `y1` and `y2` between −0.5 and 1.5 (overshoot is allowed and real).
- `samples` is the number of measured frames used and `fit_rms` the root mean square of the difference between the curve and the measured progress, in units of progress.
- A move with fewer than 3 samples (a one-frame snap) has no curve to fit: its entry keeps the times and `samples` as measured and has `bezier: null` and `fit_rms: null`, so the list still has one entry per move.

### Measuring

The detector hands each event its measured scale track: the cumulative scale at every frame of the move, taken from the fits it already makes between consecutive frames, not from a second search. The fit is least squares on `y1`, `y2` over the samples (progress = measured scale change over total scale change, time = share of the move's duration), by a direct solve (the model is linear in `y1`, `y2`), clamped to the allowed range.

### Cost

No second decode and no new per-frame work for still frames. The fit runs once per event on at most 13 points. Part of `signal_ms`; measured against the build before the change.

## Boundaries

- **Always:** one entry in `curves` per punch-in or zoom-out, in order; `from_scale`, `to_scale`, times and samples are measured, never filled in; every other field of `edit_dna.json` stays identical for files without events.
- **Ask first:** changing the 12-frame limit; fitting free time control points.
- **Never:** invent a curve for a move with too few samples; skip frames.

## Testing Strategy

- **Fit, unit tests** on synthetic tracks: a linear track gives `y1 ≈ 1/3`, `y2 ≈ 2/3`; an ease-in (`t³`), an ease-out and a smoothstep give the exact cubic's control points within 0.02; an overshoot track (progress above 1 and back) gives `y2` above 1; a noisy track still fits; fewer than 3 samples gives `null`; a flat track (no total change) does not divide by zero; the clamp holds.
- **Fixtures:** `ease12` (portrait, still texture) with three punch-ins that plant known cubics over 12 frames: an ease-in, an ease-out, and one with overshoot, with their control points in the sidecar. `motion12`'s planted smoothstep over 6 frames is the short case.
- **End to end:** `decompile` of `ease12` recovers each planted `y1`, `y2` within 0.1 and `fit_rms` under 0.05; `motion12` gives curves that reproduce the planted smoothstep within 0.05 at five points; a clip with no event gives `curves: []`; the version 3 file reads.
- **Mutation:** the clamp, the sample minimum, and the normalisation are changed in turn.

## Success Criteria

1. The planted ease-in, ease-out and overshoot are recovered within 0.1 on both control values.
2. `motion12` and `motion12_wide` give one curve per move; every other field is identical to the build before the change.
3. On the NEET reel and the lecture, the curves for the events the detector finds are listed in `status.md` with their `fit_rms`.
4. `signal_ms` on `bench60_textured` rises by at most 3% over the visual-motion build.
5. `docs/edit-dna.md`, the schema, `status.md`, the capability map, README and CHANGELOG match; AGENTS.md's P0 gap list no longer lists zoom curves.

## Resolved Questions

1. **Fixed thirds for the time control points.** Four free numbers cannot be told apart from seven noisy samples.
2. **One entry per move, `null` fit for a snap** so indexes line up with `visual_events`.
3. **Schema version 4, with a defaulted object**, so version 3 and earlier files read.
4. **Slow drifts are out of scope** until a longer-horizon scale fit is specified with its own cost.
