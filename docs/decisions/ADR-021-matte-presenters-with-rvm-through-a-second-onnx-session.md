# ADR-021: Matte presenters with RobustVideoMatting through a second ONNX session

## Status
Accepted

## Date
2026-10-29

## Context
planer.md's P2 wants presenters cut out cleanly over diagrams and screen recordings (`reelctl compositor isolate-presenter`, RobustVideoMatting in Rust through `ort`). The project already runs one ONNX model (TransNetV2, [ADR-018](ADR-018-run-transnetv2-through-a-dynamically-loaded-onnx-runtime.md)) with a library the user installs and weights the user supplies. RVM is different in kind: a recurrent network that must be run in order, one frame at a time, with four state tensors fed from each frame to the next, on video that is far larger than TransNetV2's 48x27 frames. The planer names the `fp16` file and a 172 FPS figure that is a GPU number; this machine is an Apple M3 with no GPU execution provider in use. Written under the user's "p1-p3", not reviewed line by line.

## Decision
- A new crate `crates/compositor` holds a pure core (downsample ratio, planar tensor conversion, recurrent state, alpha clamping, compositing, a frame pipeline that reads and writes through `Read` and `Write`) and two edges (the `ort` session, and the `ffprobe`/`ffmpeg` command line tools). The model sits behind a `MattingModel` trait so every test runs with a fake. The crate forbids `unsafe` code.
- ONNX Runtime is loaded as in ADR-018: `ort` with `load-dynamic`, an absolute `ORT_DYLIB_PATH`, `ort::init_from` (never `ort::init`), and a `release_runtime()` that `main` calls before exiting. `ort` allows one environment per process, so the compositor owns its own, separate from `decompile`'s; `isolate-presenter` never runs the TransNet path, so the two never meet.
- Both RVM ONNX files work. The `fp32` file feeds `float32` tensors. For the `fp16` file the adapter converts to and from `half::f16` at the session boundary (`ort`'s `half` feature, enabled in the compositor crate only) and the rest of the code sees `f32`. The model's own precision decides, read from its `src` input; the ratio is `float32` in both.
- Frames stream through the `ffmpeg` command line: one process decodes the input to raw `rgb24` at a constant frame rate (upright, SDR), another encodes raw frames, with the original audio mapped across, to a temporary file that is renamed on success. A video or image background is a third decoder, looped, scaled to cover and cropped. Colour is converted with the file's own matrix, or BT.709 from 720 lines when untagged, and back with BT.709 limited range, tagged. HDR input is refused rather than tone-mapped wrongly.
- The downsample ratio is RVM's own rule, `min(512 / max(w, h), 1)`, fixed for the clip. The output is `fgr * alpha + background * (1 - alpha)` using the model's foreground estimate, as the RVM demo does. Non-finite alpha is an error naming the frame; finite values outside 0..1 are clamped.
- Three outputs: `composite` (mp4), `matte` (grey mp4) and `rgba` (ProRes 4444 mov, straight alpha).
- Exit codes follow `timeline`: 2 for an argument or setup that is unusable (including missing weights or library, which are checked before the first frame is decoded), 1 for a failure while running. The input and background are probed before the weights are opened, so a bad command fails cheaply.
- OpenTimelineIO integration is left out: a matted clip is a new media file that `timeline init` already accepts, and recording it in a project would touch `crates/timeline`, which other slices are changing.

## Consequences
- A command that needs both TransNetV2 and RVM in one process cannot have it: `ort` builds one environment per process for good. A future pipeline command would need a shared environment (a small crate both depend on); until then they are separate commands.
- The pipeline is serial because the state depends on the previous frame, so it is as fast as one model call per frame. On the Apple M3 CPU, 720p at the automatic ratio took about 80 to 180 milliseconds per 720p frame under load (see [status](../status.md#rvm-presenter-2026-10-29)), well under the planer's GPU figure, and `fp16` was slower than `fp32` because the CPU provider converts rather than computes in half precision. Neither is a real-time rate here.
- ProRes 4444 with alpha is large (about 22 MB per second at 720p).
- The ONNX adapter's tensor code (layout, state order, `float16` conversion) cannot run in the test suite, which has no model. It is checked by a manual comparison against the official Python path, and a mistake there would pass the offline tests.
- The matte is the model's: a cluttered scene, a second person, or a moving camera gives what RVM gives; there is no refinement.
- Not covered: HDR input, a GPU execution provider, hair or edge refinement, several presenters, OpenTimelineIO.
