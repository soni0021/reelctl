# ADR-017: Measure edit events in pixels and split static windows at them

## Status
Accepted. Refines [ADR-002](ADR-002-scene-score-cut-detector.md); it does not supersede it.

## Date
2026-10-28

## Context
`decompile` found hard cuts only. An edited talking head has none, but punch-ins and overlays every few seconds, so `diff` flagged it as one long `static_window` after another. The scene score reads a mean luma difference on a 160×90 grid; a 1.18× scale of a still face or a caption box covering a tenth of the frame stays far under the 0.30 cut threshold (the 38.5 s NEET reel's caption box scored 0.303 and became a cut only by luck). The model in `reelctl understand` can name such events but only at sampled frames and not to the frame.

## Decision
- `decompile` measures `punch_in`, `zoom_out` and `overlay_change` events from the luma grid the decoder already builds for the scene score, in the same single pass, with no second decode and no skipped frame (the fast-decode contract, [ADR-012](ADR-012-decode-hevc-in-hardware-and-every-frame-exactly.md)). The detector is `crates/decompile/src/motion.rs`, a pure module over grids.
- A frame is compared with the grid at least 125 ms earlier, not with the previous frame: a consecutive-frame difference never fired on a flat scene with an eased move and depended on the frame rate. A run of changed frames is fitted for one scale about a centre; it is an event only if that scale explains the picture clearly better than none (from 250 ms before to 250 ms after) and the total change is at least 0.05. A run longer than 600 ms is a drift, not an event. The rule is therefore time-based: 600 ms is about 14 frames at 24 fps and 36 at 60 fps. (The spec first said "12 frames"; the audit found the code is in time, which is the better rule, and the docs now say so.)
- An overlay is a connected block of changed cells covering 1–40% of the frame that fills its bounding box, with a quiet remainder, that persists for 500 ms. It is one kind, `overlay_change`: pixels cannot say whether a block appeared or went, or whether it is an overlay, a slide or board writing.
- `edit_dna.json` gains `visual_events` (schema version 3, defaulted so version 1 and 2 files read). An event never lies on a frame the cut detector calls a cut.
- `diff` splits a static window at every event as at a cut. Files without events behave exactly as before.

## Consequences
- Measured: planted events are found at the planted frames on portrait and landscape clips; no event on a still clip or on the solid-colour fixtures; the cost is under 10% of `signal_ms` (worst case +7% on a clip with a punch-in every 5 s). See [status](../status.md#visual-motion-2026-10-28).
- On a real 28.6-minute lecture, about five of eight events checked by eye were what the name says; a speaker walking across a static background is read as an overlay, and board writing reads as `overlay_change`. The thresholds were tuned on planted clips and four recordings and are untested on fast-cut footage.
- Slow pushes (a Ken Burns drift over more than 600 ms) are not events; measuring them belongs to a later module. Easing curves for the measured moves are specified in [zoom-curves](../specs/zoom-curves.md).
- The planted-event sidecars (`fixtures/generated/*.events.json`, written by `fixtures/generate.sh`) list each planted event with `kind`, `start_ms`, `end_ms`, `time_ms`, `frame_index`, `start_frame`, `scale`, `center_x`, `center_y`, `area`, and for boxes `change`; the end-to-end tests read them.
