# Spec: Visual motion events (`visual-motion`)

Status: written 2026-10-28 under the user's blanket "Do all of them" (after a list of remaining work that named `visual-motion`); the user has not reviewed it line by line. The four answers at the end are the recommended ones, shown to the user in the same message. Built on `feature/visual-motion`; evidence in [status.md](../status.md#visual-motion-2026-10-28). Plan: `tasks/plan.md` (archived with this branch's merge). Branch: `feature/visual-motion`. Module `visual-motion` in the [P0 capability map](p0-capability-map.md), limited to edit events by its decision 1. Follows [planer.md](../../planer.md) Pass 1 ("optical-flow ... camera punch-in ratios") and Pass 2 ("static visual windows (>2.5s without B-roll, zoom, or graphic callout)").

## Objective

`reelctl decompile` finds hard cuts. An informational Reel is mostly edited without them: a punch-in that scales the same take by 1.15×, a caption box or chart that pops over the picture, a B-roll card. The 18-minute NEET video has no cut above the 0.30 threshold, yet a punch-in or an overlay appears every few seconds. `reelctl diff` therefore flags it as one static window after another, and a clip's `static_window` flag cannot tell a dead talking head from an edited one.

This module measures those events in pixels and writes them to `edit_dna.json`:

- **`punch_in`**: the picture scales up by a measured factor (for example 1.18) over a few frames, around a measured centre, with no cut.
- **`zoom_out`**: the reverse.
- **`overlay_change`**: a connected region of the frame changes, and stays changed, while the rest of the frame does not: a caption box, chart, lower third, or B-roll card appearing, changing, or going. It reports the share of the frame it covers.

`reelctl diff` then treats an event as a visual change: a stretch is a static window only if it holds no cut and no event. Nothing about cuts changes.

Not in this module: classifying what an overlay is (that is the model's job in `reelctl understand`), Bézier easing curves and LUTs (each gets its own spec later), and pans, rotations, and speed ramps.

## Evidence

- `docs/status.md`, the 18-minute NEET video: 0 cuts; scene score above 0.08 on 9 frames, "all overlay changes or possible jump cuts"; what look like punch-in zooms; `static_window` flags throughout.
- The scene score is `min(mafd, |mafd - prev_mafd|) / 100`, a mean absolute luma difference on a 160×90 grid. A scale of a talking head changes few pixels per frame, and a box covers a small part of the frame, so both stay below 0.30. The NEET explainer's caption box scored 0.303 and became a cut by luck.
- The decode loop already builds a luma grid for every frame (`SadState`, point-sampled, about 160×90) and keeps only the previous and current ones. Events can be measured from those grids without a second decode and without skipping frames ([fast-decode](fast-decode.md) forbids skipping).
- No fixture has a planted zoom or overlay: the solid-colour clips look the same at every scale.

## Tech Stack

Rust workspace as today. No new dependencies. Detection is plain integer and float code over the luma grids in `crates/decompile` (a new module `motion`), the type in `crates/edit-dna`, and the flag change in `crates/diff`.

## Contract

### Output

`edit_dna.json` gains `visual_events` (schema version 3; a version 2 file parses with an empty list through `#[serde(default)]`):

```json
{"visual_events": [
  {"time_ms": 4000, "frame_index": 96, "kind": "punch_in", "scale": 1.18, "center_x": 0.5, "center_y": 0.5, "area": null},
  {"time_ms": 8000, "frame_index": 192, "kind": "overlay_change", "scale": null, "center_x": null, "center_y": null, "area": 0.12}
]}
```

- `time_ms` and `frame_index` are the first frame that shows the change in full (a cut's convention), measured, never padded.
- `scale` is the factor between the picture before and after the move (greater than 1 for `punch_in`, less than 1 for `zoom_out`), `center_x` and `center_y` are the fixed point of the move as a share of the frame, `area` is the changed share of the frame for an overlay. A field that does not apply is `null`.
- Events are sorted by `time_ms`. An event never lies on a frame that is also a cut.

### Detection

All thresholds are constants in `motion.rs`, chosen on the fixtures and the two real reels, with their basis recorded in `status.md`.

- **Overlay change.** Compare the current luma grid with the one from a few frames earlier (so a fade over several frames counts) and with the first frames after. A cell counts as changed if it differs by at least a set number of levels. An event needs a connected block of changed cells covering a set range of the frame, a quiet remainder (so a speaker moving or a camera shake is not an overlay), and a change that persists for a set time after (so a passing hand or a flash is not one).
- **Scale.** Each frame is compared with the grid at least 125 ms earlier (not with the previous frame: a consecutive-frame difference never fired on a flat scene with an eased move and depended on the frame rate). A run of frames that differ from their baseline by more than a floor is fitted for one scale about a centre, by a coarse list of scales and centres and then a pattern-search refinement; it is an event only if that scale explains the picture clearly better than no scale, from 250 ms before to 250 ms after the run, and the total change is at least 0.05. A run that lasts more than 600 ms is a drift, not an event. Measured at 24 fps with an 18% total change: a linear move of 12 or 13 steps is an event, 14 or more is not; a smoothstep ease up to 16 steps is an event; an accelerating move that ends fast counts, because its fast end is a real move. Slower or gentler pushes give none; measuring those is the zoom-curves spec's neighbour, not this one. Gating: the fit runs only on a run above the floor, and a cheap coarse check rejects non-scale motion before the refinement.
- **Cost control.** The scale fit runs only on frames whose plain difference is above the floor. A still talking head costs one comparison per frame.

### `diff`

`push_static_windows` splits a window at every `visual_events` time as it does at a cut. Files with no events behave exactly as today. `flags` stay byte-identical for every existing fixture pair.

## Boundaries

- **Always:** keep `cuts`, `shot_histograms`, `payoff_window` and every other field of `edit_dna.json` identical for files without events; measure, never invent a time; add the cost to `signal_ms` (the events are structural).
- **Ask first:** changing the 0.30 cut threshold; raising the cost of `signal_ms` by more than 10% on the benchmark.
- **Never:** skip frames to save time; use the model for these events; commit a real reel.

## Testing Strategy

- **Detector, unit tests on synthetic grids** (no video): a block that appears, grows, fades in, goes; a block that appears and vanishes within the persistence window (no event); global brightness shift (no event); a grid scaled by 1.18 about the centre and about a corner (event, right scale and centre); an eased scale over 6 frames (one event); scale 1.03 (below the floor, no event); noise only (no event).
- **Fixtures** (`fixtures/generate.sh`, a textured static background so a scale changes pixels, **540×960 portrait** like a real reel, whose luma grid has uneven steps and a different shape from a landscape one): `motion12` with a planted punch-in at 4.0 s (1.0 to 1.18 over 6 frames), a zoom-out at 6.0 s back to 1.0, and an overlay box at 8.0 s that stays, with a sidecar of the planted values; `still12` with the same texture and no events; `motion12_wide`, a 640×360 landscape copy of `motion12` so both grid shapes are covered; and `bench60_textured`, 60 s of textured portrait video with a punch-in every 5 s, for the cost check (the solid-colour `bench60` never runs the scale fit). Existing fixtures: those with a solid colour give no event; in `textured_cuts12` the shot that zooms by itself and the shot with moving elements are probed first and the result is recorded here before the criterion is final: an event there is allowed only if it is a move completing within 12 frames. Their `cuts` are unchanged.
- **End to end** in `p0_fixture.rs`: `decompile` of `motion12` gives exactly the planted events within two frames and 0.03 in scale, `area` within 30% of planted, no cut; `still12` and all other fixtures give none; `diff` of `motion12` with `static_window` gone where an event splits it and present without events.
- **Mutation:** thresholds and the persistence rule are each changed in turn and a test must fail.

## Success Criteria

1. `motion12` and `motion12_wide` give the planted `punch_in`, `zoom_out`, and `overlay_change` and nothing else; `still12` and every solid-colour fixture give no event; `textured_cuts12` gives no event except one the probe showed to be a real move within 12 frames; the other fields of every file are identical to the build before the change.
2. `diff` flags a 12 s stretch with no cut as one `static_window`, and no flag where events split it into windows of 2.5 s or less; the existing p0 `diff` tests pass unchanged.
3. The 18-minute NEET video yields a list of events whose first twenty were checked by eye against the frames, recorded in `status.md` with the number that were right, and the 9 frames the scene score marked above 0.08.
4. `signal_ms` on the 60 s solid-colour benchmark rises by at most 10%, and on `bench60_textured`, where the scale fit runs, the cost of the module is measured and recorded (with and without it); the cost on the two real reels and the NEET video is measured too. An ADR records a rise of more than 10% anywhere, and the 500 ms target already misses on real footage.
5. `docs/edit-dna.md`, the schema, `cli.md`, `status.md`, the capability map, README and CHANGELOG match the code.

## Resolved Questions

Answers taken (the recommended ones):

1. **One overlay kind, not on/off.** A region that changes cannot be called "on" or "off" from pixels alone. `overlay_change` carries the area; `reelctl understand` names the rest.
2. **Schema version 3 with a defaulted list** rather than a new file, so existing readers keep working.
3. **Events split `static_window` in `diff`** (the one `diff` change); no new flag in this module.
4. **Thresholds are tuned on the fixtures and two real reels** and are untested on fast-cut footage. That limit goes in `status.md` with the numbers.
