# ADR-002: Detect cuts with FFmpeg's scene score while TransNetV2 is unlinked

## Status
Accepted; superseded in part by [ADR-018](ADR-018-run-transnetv2-through-a-dynamically-loaded-onnx-runtime.md) (2026-10-28): `TransNetV2Detector` no longer always errors, the `CutDetector` trait takes a `DetectorInput`, and the `auto`/`transnet` rules are those in ADR-018 and [cli.md](../cli.md). The scene-score detector and the rest of this record stand.

## Date
2026-10-18. Recorded on 2026-10-20 from [scene.rs](../../crates/decompile/src/scene.rs), [transnet.rs](../../crates/decompile/src/transnet.rs), and [AGENTS.md](../../AGENTS.md).

## Context
planer.md specifies TransNetV2, run through `pykeio/ort`, to find hard cuts, whip pans, and speed ramps. Its ONNX weights are not in the repo, and P0 does not download or vendor reference models (AGENTS.md). With no weights file to run, `ort` was not linked.

## Decision
- A `CutDetector` trait (`kind()` and `detect(frames)`) with two implementations:
  - `SceneScoreDetector`: the formula behind FFmpeg's `scene` value, `clip(min(mafd, |mafd − previous mafd|) / 100, 0, 1)`, where `mafd` is the mean absolute luma difference from the previous frame. A frame scoring above 0.30 starts a new shot. The threshold matches `select='gt(scene,0.3)'`.
  - `TransNetV2Detector`: always returns an error that says what is missing.
- `--cut-detector auto` tries TransNetV2 only when `--transnet-weights` names an existing file, and falls back to the scene score if it fails, recording why in `cut_detector_note`. `--cut-detector transnet` fails instead of falling back. `--cut-detector scene` never tries TransNetV2.

## Alternatives Considered

### Link TransNetV2 through ort now
- Blocked: the weights are not in the repo, and P0 does not download them.

### FFmpeg's `select` filter through the `ffmpeg` command
- Ruled out by [ADR-001](ADR-001-in-process-software-decode.md).

## Consequences
- In the fixture clips, hard cuts between solid colors are found at the exact frame.
- The score reacts to abrupt change from one frame to the next, so gradual transitions such as dissolves are likely to be missed, and flashes or fast motion may register as cuts. None of this has been tested.
- `detect` receives only per-frame times and scores (`ScoredFrame`), not pixels. TransNetV2 works on downscaled frames, so wiring it in means either passing frames through the trait or preparing TransNetV2's input inside the decode loop.
- Asking for TransNetV2 explicitly never silently returns scene-score cuts.
