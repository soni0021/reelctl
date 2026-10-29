# Spec: Presenter cut-out with RobustVideoMatting (`rvm-presenter`)

Status: built (see [status](../status.md#rvm-presenter-2026-10-29)). Written 2026-10-29 under the user's blanket "p1-p3"; the user has not reviewed it line by line. The answers at the end are the recommended ones. Branch: `feature/rvm-presenter`, cut from `main` (b312ffd). Plan: [rvm-presenter-plan](rvm-presenter-plan.md). Decision record: [ADR-021](../decisions/ADR-021-matte-presenters-with-rvm-through-a-second-onnx-session.md). Follows [planer.md](../../planer.md) (PeterL1n/RobustVideoMatting: "run `rvm_mobilenetv3_fp16.onnx` inside Rust (pykeio/ort) ... to isolate presenters cleanly over informational diagrams and screen recordings"; `reelctl compositor isolate-presenter`). Uses the same dynamically loaded ONNX Runtime pattern as [transnet-cuts](transnet-cuts.md) and [ADR-018](../decisions/ADR-018-run-transnetv2-through-a-dynamically-loaded-onnx-runtime.md).

## Objective

A talking-head Reel is more useful as an explainer when the presenter stands in front of the thing being explained: a chart, a diagram, a screen recording. Today the project can cut and zoom a clip ([timeline-editor](timeline-editor.md)) but cannot separate the person from the room. This module cuts the presenter out of a video with RobustVideoMatting (RVM, a recurrent matting network that predicts an alpha matte and a foreground estimate for each frame) and writes one of three things:

- **composite** (default): the presenter over a background you give it, a solid colour, an image (for example a chart frame) or a video;
- **matte**: the alpha matte alone, as a grey video, for another compositor to use;
- **rgba**: the presenter with its alpha, as a ProRes 4444 `.mov`, for Smelter or an editor to lay over anything.

Command: `reelctl compositor isolate-presenter --input <video> --background <#rrggbb|image|video> --weights <rvm.onnx> --out <file>`. The model is a file the user supplies; ONNX Runtime is a library the user installs. Neither is vendored or downloaded.

Not in this module: OpenTimelineIO integration (see Resolved Questions), rendering a background from manimgx or Smelter (they write a file, which `--background` accepts), temporal refinement of the alpha, hair or edge refinement beyond what the model gives, GPU execution providers, HDR input, more than one presenter selected from many, and live preview.

## Evidence

- RVM's published contract (`documentation/inference.md`, `inference.py` in PeterL1n/RobustVideoMatting, read 2026-10-29): input RGB scaled to 0..1; four recurrent states fed back frame to frame, first frame zeros; `downsample_ratio` chosen so the downsampled longest side is at most 512 (`auto_downsample_ratio(h, w) = min(512 / max(h, w), 1)`); output `fgr` (foreground, 3 channels) and `pha` (alpha, 1 channel); the demo composites `fgr * pha + bgr * (1 - pha)`.
- The ONNX files from the project's v1.0.0 release, read with the conda ONNX Runtime 1.21 on 2026-10-29. Inputs `src` `[B,3,H,W]`, `r1i`..`r4i` `[B,C,H,W]`, `downsample_ratio` `[1]` (always `float32`); outputs `fgr`, `pha`, `r1o`..`r4o`, with 16, 20, 40 and 64 channels. The `fp32` file has `float32` for everything but the ratio and the `fp16` file `float16` for `src`, the states, `fgr` and `pha`. `planer.md` names the `fp16` file; the 172 FPS in it is a GPU figure. This project runs on an Apple M3 CPU, so both files are supported and both are measured ([status](../status.md)).
- The pattern for a second ONNX session exists in `crates/decompile/src/transnet.rs`: a trait for the model so tests run offline with a fake, `load-dynamic` so the build downloads nothing, an absolute `ORT_DYLIB_PATH`, a shared environment released before exit (ONNX Runtime 1.21 aborts at exit otherwise).
- `crates/reelctl/src/timeline_cmd.rs` renders by running the `ffmpeg` and `ffprobe` command line tools, reads phone rotation from `ffprobe`, writes to a temporary file and renames.

## Tech Stack

Rust workspace as today. New crate `crates/compositor`. New dependencies: none beyond the workspace's: `ort` (already pinned, with its `half` feature added in this crate for the `fp16` file), `half` (already in the registry cache, pulled in by `ort`'s feature), `serde_json` for `ffprobe`, `thiserror`, `image` is not needed. No `unsafe`: the crate has `#![forbid(unsafe_code)]`. `ffmpeg` and `ffprobe` command line tools are required at run time, as for `render`.

## Design

### Layers

| Module | Pure? | What |
|---|---|---|
| `ratio` | yes | `auto_downsample_ratio`, ratio validation |
| `tensor` | yes | `Tensor4`, `RecurrentState`, `FrameRgb` (u8 interleaved to f32 planar), alpha cleaning |
| `composite` | yes | per-pixel composite, `matte` frame, `rgba` frame |
| `background` | yes | `Background` source of frames: `SolidColor`, `RawFrames<R: Read>`; `#rrggbb` parsing |
| `pipeline` | yes | `isolate(reader, background, writer, model, options)`: reads exact frames, feeds the state forward, writes composite bytes |
| `model` | yes | the `MattingModel` trait and its output type |
| `ort_model` | no | the ONNX Runtime session (`RvmModel`), weights and library checks, `release_runtime()` |
| `media` | no | `ffprobe`, the decoder and encoder child processes, the temporary file, `isolate_presenter(request, model)` |

`reelctl` has one thin file, `crates/reelctl/src/compositor_cmd.rs`: arguments, opening the model, calling `isolate_presenter`, exit codes. Its one-line change in `main.rs` is the subcommand and a call to `compositor::release_runtime()` beside `decompile::release_onnx_runtime()`.

### The model seam

```rust
pub trait MattingModel {
    fn matte(&mut self, frame: &FrameRgb, ratio: f32, state: &RecurrentState)
        -> Result<ModelOutput, CompositorError>;
}
pub struct ModelOutput { pub fgr: Vec<f32>, pub pha: Vec<f32>, pub state: RecurrentState }
```

`fgr` is planar `[3, H, W]` and `pha` is `[H, W]`, both 0..1. `RecurrentState` is four `Tensor4` (`shape: [usize; 4]`, `data: Vec<f32>`), always `f32`; the ONNX adapter converts to `float16` and back when the model is the `fp16` file. A state is `zeros` (`[1,1,1,1]` each) for the first frame, and afterwards exactly the previous call's output. The real session cannot run in a test; a fake does, and it is made hostile on purpose: its state output has a different shape from its input and carries the frame number, so a test can show that frame N received frame N−1's state, and that frame 0 received zeros.

### Decisions

- **Downsample ratio:** `min(512 / max(width, height), 1)`, RVM's own rule, fixed once per clip because the state shapes depend on it. `--downsample-ratio` overrides it (0 < r <= 1). The output of a 1080p clip is 0.2667 (the documentation's 0.25 is an example inside 256..512).
- **Alpha:** non-finite alpha is an error naming the frame (the model is broken or the input is); a finite value outside 0..1 is clamped (the network overshoots a little, more so in `fp16`). `fgr` is clamped the same way. A non-finite `fgr` is an error.
- **Composite:** per channel, `round(fgr * alpha * 255 + background * (1 - alpha))`, using the model's foreground estimate `fgr`, not the source frame: it is what the RVM demo does, and it removes the old background's colour from the soft edge. The source frame is not used after it has been fed to the model.
- **Rgba:** straight (not premultiplied) alpha: `fgr` and `alpha`, each rounded to 8 bits. ProRes 4444 `yuva444p10le` in a `.mov`.
- **Matte:** `round(alpha * 255)` as a grey frame, encoded as H.264 `yuv420p`, so it is limited range (0 is 16, 1 is 235) after decoding; the command's output line says so.
- **Background fit:** an image or video is scaled to cover the output (`force_original_aspect_ratio=increase`) and cropped at the centre; it is never stretched. A video background loops until the presenter ends; an image repeats. It has no audio used. The scaling is done by `ffmpeg`, so a background of any format `ffmpeg` reads works, and `RawFrames` only reads exact frames.
- **Colour:** SDR only. The decoder converts to RGB with the video's own matrix when it is tagged, and with BT.709 for untagged video of 720 lines or more and BT.601 below that (the convention players use). The encoder converts back with BT.709 limited range and writes `bt709` tags (matrix, primaries, transfer and range: the encoder's own tag options write only two of the four with ffmpeg 8.1, so the frames are tagged with `setparams` and a test reads all four), so a pass-through (alpha 1 everywhere) returns the input colours within codec noise. A source whose transfer is HLG or PQ (an iPhone HDR recording) is refused with a message; the user converts it to SDR first. Silently tone-mapping wrongly would be worse.
- **Frame rate:** the output has the input's average frame rate, constant (`fps` filter on the decode side); a variable-rate phone video is made constant, and the audio is mapped from the original, so frame count and audio stay within a frame of each other. A video with a rotation tag is rotated upright by `ffmpeg` and the probed size is swapped to match; the output has no rotation tag.
- **Audio:** the original audio is encoded as AAC 192 kbit/s when there is one (`-map 1:a?`); `matte` output has none; `rgba` carries it as `pcm_s16le`.
- **One session, serial frames:** frame N+1 needs frame N's state, so frames are run one at a time. Decoding and encoding are separate `ffmpeg` processes, so they overlap with the model in time.
- **Memory:** one frame in flight: the RGB bytes, the planar `f32` tensor, the outputs and the state. About 6 MB + 25 MB + 31 MB + the state at 1080p; measured in [status](../status.md). The whole video is never held.

### Failure order

Everything that can be checked is checked before a decoder starts, and nothing is written to `--out` unless the run completes: the file is made beside the output under a temporary name and renamed.

1. Arguments: `--background` is `#rrggbb` or an existing file (required for composite, refused for matte and rgba); `--out` is not a directory and has the right extension (`.mov` for rgba); `--downsample-ratio` is in (0, 1].
2. The input: a file, `ffprobe` reads it, it has a video stream, SDR, a size of at least 16 by 16.
3. The background: `ffprobe` reads it, SDR.
4. The weights: given, exist, are a file (not a directory).
5. ONNX Runtime: `ORT_DYLIB_PATH` set, not empty, absolute, a file; the library loads.
6. The model: loads, has the RVM inputs and outputs by name and the element type `float32` or `float16`, `float32` for `downsample_ratio`.

The input and background come before the weights (changed from the first draft of this spec, which had them after) so that the cheap checks fail first and so that the failure tests for them need no model.

### CLI

```
reelctl compositor isolate-presenter --input <video> --weights <rvm.onnx> --out <file>
    [--background <#rrggbb|image|video>] [--mode composite|matte|rgba]
    [--downsample-ratio <r>]
```

`--weights` also reads `REELCTL_RVM_WEIGHTS`; the library comes only from `ORT_DYLIB_PATH` (ONNX Runtime's own name). Exit codes, as for `timeline`: **2** for an argument or setup that is unusable (a missing, mistyped or unloadable weights file or library, a bad background, an input that cannot be read or is HDR, a bad output path), **1** for a failure while running (`ffmpeg` died or is missing, the model failed or returned a non-finite alpha, the output cannot be written). On success it prints one line:

```
isolate-presenter: out=out.mp4 mode=composite frames=120 size=1080x1920 fps=30 downsample_ratio=0.2667 model=float32 model_ms=3150 ms_per_frame=26.3 total_ms=4020
```

## Boundaries

- **Always:** validate before decoding; a failed run leaves no `--out` file; the matte is the model's, with no invented alpha; every number in `status.md` is measured with a date and a machine; the tests use a fake model.
- **Ask first:** a GPU execution provider; changing the colour handling; vendoring weights.
- **Never:** commit weights or media; use the network; `unsafe` in this crate; fall back to a different model when the weights are wrong; run on an HDR source as though it were SDR.

## Testing Strategy

- **Unit (pure core):** the ratio table (1920x1080, 1080x1920, 3840x2160, 640x360, 512x512, 300x200, a non-multiple like 1000x563) and the validation bounds; u8 to planar `f32` (exact values and layout); alpha clamp and non-finite refusal; composite arithmetic at alpha 0, 1, 0.5, with `fgr` different from the source; matte and rgba frames; `#rrggbb` parsing and rejects; the pipeline reads exact frames and fails on a short final frame, an empty stream and a background that ends early; the state fed to frame N is frame N−1's output (the hostile fake) and frame 0 gets zeros; the ratio and frame size reach the model; the model's error stops the run with the frame number; a probe parser for rotation, rational rates, HDR and a missing video stream; the ORT setup checks (library unset, empty, relative, a directory, missing weights), as for TransNet.
- **Integration (`crates/compositor/tests`, real `ffmpeg`, fake model):** a two-colour clip with a fake that gives alpha 1 on the left half and 0 on the right: the output's left half is the input's colours within codec noise and the right half is the background, for a solid colour, an image and a looping video; alpha 1 everywhere returns the input colours (the matrix check); `matte` gives 16/235; `rgba` has an alpha channel; audio is present in composite and absent in matte; the frame count and the duration equal the input's; a variable-rate input and a rotated input come out constant-rate and upright; a failing fake leaves no file.
- **CLI (`crates/reelctl/tests/compositor_fixture.rs`, no model):** every setup failure: no weights, weights a directory, weights missing, `ORT_DYLIB_PATH` unset, empty, relative, missing, a file that is not a library, a bad or missing background, a missing input, `--out` a directory, rgba to `.mp4`, an HDR-tagged input; each checks the exit code, the message and that `--out` does not exist; `--help` lists the command. The child environment drops `REELCTL_WHISPER_MODEL`, `REELCTL_TRANSNET_WEIGHTS`, `REELCTL_RVM_WEIGHTS`, `ORT_DYLIB_PATH` and `OPENROUTER_API_KEY`.
- **Manual, real model (documented, not in the suite):** `docs/status.md` records the commands and results on the M3: ms per frame at the automatic ratio for `fp32` and `fp16`, peak memory from `/usr/bin/time -l`, an exit status of 0 (the environment is released), the maximum difference in `pha` against the official Python path on the same frames (`crates/compositor/manual/rvm_reference.py`), and one composite frame looked at.
- **Mutation:** by hand on the pure core: the 512 constant, the `min(.., 1)`, the clamp bounds, `fgr` for the source, the state fed back replaced by zeros each frame, alpha for 1 − alpha, the `round`. Survivors are recorded in `status.md`.

## Success Criteria

1. `cargo test --workspace` is green with no network, no weights and no ONNX Runtime; no test is ignored or skipped.
2. With a fake model, the composite shows the presenter region over the chosen background, for the three background kinds, with correct colours.
3. Every setup failure listed exits 2 with a message that names the file or variable and says what to do, and leaves no output.
4. On the real model, `fp32` and `fp16` both run on a clip with a person, the process exits 0, the alpha agrees with the official Python path to a stated tolerance, and the cut-out looks right. If it does not, `status.md` says what was seen.
5. README, `docs/cli.md`, `docs/status.md`, CHANGELOG, AGENTS.md, the capability notes and ADR-021 match the code.

## Resolved Questions

1. **OTIO integration: left out.** A matted video is a new media file, so `reelctl timeline init --input matted.mp4` already makes a project of it. Recording "this clip was matted" in the project's `metadata.reelctl` would need a change in `crates/timeline`, which the Smelter and manimgx slices also touch, for no editing it enables yet. A later slice that places the presenter on a track will bring its own spec.
2. **Both `fp32` and `fp16`.** `planer.md` names `fp16`; on a CPU `fp16` is converted rather than computed and may be slower, so the choice is the user's, made by which file they pass, and the numbers are in `status.md`.
3. **Own environment, not decompile's.** `ort` allows one environment per process. `isolate-presenter` never decodes with the TransNet path, so the compositor builds and releases its own. A command that needs both models in one process would need a shared environment crate; ADR-021 records that as a consequence.
4. **`--background` required only for composite**, and `#rrggbb` needs its `#` so a file named `abcdef` is never read as a colour.
5. **Straight alpha in `rgba`.** Compositors expect it; premultiplying is their job.
6. **`ffmpeg` scales the background.** It is already required, handles every format and avoids a second decoder in Rust.
7. **Exit codes 1 and 2 only**, as `timeline`; 3 and 4 are `understand`'s.
