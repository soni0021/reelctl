# Spec: TransNetV2 cut detection (`transnet-cuts`)

Status: written 2026-10-28 under the user's blanket "Do all of them"; the user has not reviewed it line by line. The answers at the end are the recommended ones. Module `transnet-cuts` in the [P0 capability map](p0-capability-map.md), whose decision 2 ("download the weights? Not now") the user's "Do all of them" lifted: the weights are at `~/models/transnet/transnetv2.onnx` (not in the repo). Follows [planer.md](../../planer.md) (TransNetV2 run through `ort`). Supersedes the "always errors" part of [ADR-002](../decisions/ADR-002-scene-score-cut-detector.md) through [ADR-017](../decisions/ADR-017-run-transnetv2-through-a-dynamically-loaded-onnx-runtime.md).

## Objective

`--cut-detector transnet` has always exited with an error, and `auto` has always fallen back. This module makes TransNetV2 run: the `ort` crate loads an ONNX Runtime library that the user supplies, runs the model over 48×27 RGB frames taken from the decode loop, and turns its per-frame transition probabilities into the same `cuts` list the scene-score detector writes.

Why: the scene score reads the change between two adjacent frames, so a dissolve or a fade (small steps over 12 to 24 frames) never crosses 0.30. On a synthetic 12 s clip with a one-second dissolve and a one-second fade through black, the scene-score detector finds 0 cuts; TransNetV2 marks both transitions (Evidence).

Not in this module: downloading weights or the ONNX Runtime library, GPU or Core ML execution providers, TransNetV2's many-hot output (the official post-processing uses the single-frame output), training or fine-tuning, and any change to what the scene-score path outputs apart from the `cut_detector_note` text (Contract, "Scene-score path").

## Evidence

All on the Apple M3 MacBook Air (8 cores), macOS 26.5, 2026-10-28, with the load average above 100 for most of the session (other builds were running), so every timing here is an upper bound. Model: `transnetv2.onnx` (sha256 `c4d54a682bace32f25136ef83ca2c9d403e8f8193775efeb995172a0d95a8e0c`, MIT, ONNX opset 17), input `input` float32 `[1,100,27,48,3]` (channels last, RGB, raw 0 to 255; the graph divides by 255), outputs `534` (single-frame probability) and `535` (many-hot probability), both `[1,100,1]` and already through a sigmoid. Runtime: ONNX Runtime 1.21.0 (the library inside a conda `onnxruntime` wheel at `/opt/anaconda3/lib/python3.12/site-packages/onnxruntime/capi/libonnxruntime.1.21.0.dylib`); `brew list onnxruntime` says it is not installed, and Homebrew's formula is 1.31.1.

- **`ort` links with no download.** The crate is `ort 2.0.0-rc.15` (crates.io has no stable 2.0; the newest release is `2.0.0-rc.15` and `max_stable_version` is empty). With `default-features = false, features = ["std", "load-dynamic", "api-17"]` it builds from the crate sources alone (it compiles three new crates, `ort`, `ort-sys` and `libloading`; `smallvec` was already in the tree; no `ndarray`, no `download-binaries`, no TLS stack. `Cargo.lock` also lists the optional `ndarray`, `tracing` and their helpers, which are not built) and loads the library at run time from a path. A scratch program built this way ran the model on ONNX Runtime 1.21.0 and returned `[1,100,1]` outputs. `api-17` makes `ort` require a library of version 1.17 or newer; it refuses an older one with an error that names both versions, and a missing file with `failed to load from <path>: dlopen failed`. `ort`'s default loader panics on a missing library, so the code never uses it: it always calls `ort::init_from(path)`, which returns an error.
- **Official post-processing, checked on fixtures.** With ffmpeg's `scale=48:27` as the input (the official one), windows of 100 frames at stride 50, the first 25 frames padded with copies of frame 0, the end padded with `25 + 50 - (n % 50 or 50)` copies of the last frame, the middle `[25:75)` of each window kept and the result truncated to `n` frames:

| Clip | Frames above 0.5 (single-frame output) | Scene score finds |
|---|---|---|
| `textured_cuts12` (24 fps; hard cuts at 3, 6, 9 s, so new shots start at frames 72, 144, 216) | 71 (0.9649), 143 (0.9843), 215 (0.9937) | frames 72, 144, 216 |
| `draft12` (30 fps; one hard cut red to white at 6 s, new shot at frame 180) | 179 (0.9982) | frame 180 |
| `dissolve12` (24 fps; dissolve over frames 96 to 119, fade through black over frames 192 to 215) | 108 (0.8372), 109 (0.8093); 206 (0.5653), 207 (0.6233) | nothing |

  For a hard cut the model's probability is on the last frame of the old shot (71), not on the first frame of the new one (72). So the cut is "the first frame after the run of frames above 0.5", which is the first frame of the new shot and equals the scene-score detector's `frame_index` on every hard cut (72, 144, 216, 180). This is also what the official `predictions_to_scenes` does: a scene ends on the first frame above the threshold and the next begins after the run.
- **Cost.** With stride 50 every frame is in two windows, so the model runs on about twice as many frames as the clip has. The first probe, on a machine at load average 370, took 0.8 s per window; the numbers under "Measured" (load 9) are 7 ms per video frame, or about 13 s for a 60 s reel at 30 fps. TransNetV2 is therefore far outside the 500 ms structural budget and is not the default path: `auto` uses it only when the user supplied weights, and `signal_ms` includes it (cut detection is structural).
- **Frames per video.** The decode loop already builds a 64×36 RGB thumbnail for every frame, by point sampling (`fill_thumb`: one pixel per 16×30 block on a 1080p frame; it also keeps only the last four). Point sampling aliases; ffmpeg's `scale` filters. The model was trained on ffmpeg-scaled input.

## Tech Stack

Rust workspace as today. One new dependency in `crates/decompile`: `ort =2.0.0-rc.15` with `default-features = false, features = ["std", "load-dynamic", "api-17"]` (no `ndarray`: `Tensor::from_array((shape, Vec<f32>))` takes a plain vector). The ONNX Runtime library itself is not a dependency and is not built or downloaded by cargo: it is found at run time through `ORT_DYLIB_PATH` (`ort`'s own variable name). The default build and `cargo test` need no network after the crates are fetched, and no library. The pure windowing and post-processing code needs no `ort` types and is tested without a model.

## Commands

```bash
cargo check
cargo test --workspace
bash fixtures/generate.sh dissolve12 /tmp/dissolve12.mp4
# Manual end-to-end run on the real model (not part of cargo test; needs the weights and a library):
export ORT_DYLIB_PATH=/opt/homebrew/lib/libonnxruntime.dylib      # or any libonnxruntime >= 1.17
export REELCTL_TRANSNET_WEIGHTS=~/models/transnet/transnetv2.onnx
target/release/reelctl decompile --cut-detector transnet --input /tmp/dissolve12.mp4 --out /tmp/dissolve12.edit_dna.json
```

## Project Structure

| Path | Change |
|---|---|
| `crates/decompile/src/transnet_window.rs` | New, pure: `ModelFrames` (the 48×27 frames), window layout and padding, `predict` (runs any window function over a clip), runs of probabilities above a threshold, runs to cut points |
| `crates/decompile/src/transnet.rs` | The `ort` code: library lookup, the process-wide environment, the session, the output lookup; `TransNetV2Detector`; the choice of detector and its failure messages |
| `crates/decompile/src/scene.rs` | `CutDetector::detect` takes a `DetectorInput` (scores and, for TransNetV2, the model frames); `finalize_cuts` gains a form that takes a score per cut |
| `crates/decompile/src/decode.rs` | Fills `ModelFrames` in the decode loop when asked (`fill_model_frame`); `RawBoundary` carries its thumbnail size |
| `crates/decompile/src/pipeline.rs` | Chooses the detector before decoding, rebuilds the filmstrip boundaries from the cuts it got |
| `crates/reelctl/tests/*.rs` | The spawned `reelctl` loses `ORT_DYLIB_PATH` from its environment as it loses the weights variable |
| `fixtures/generate.sh` | `dissolve12`: a dissolve and a fade through black made with ffmpeg's `xfade`, no hard cuts |
| Docs | This spec, ADR-017, `docs/cli.md`, `docs/edit-dna.md` (`cut_detector`, `scene_score`), the capability map. Left to the main session: README, CHANGELOG, `docs/status.md`, AGENTS.md, `tasks/` |

## Contract

### The model input

Each decoded frame becomes one 48×27 RGB frame (3,888 bytes), taken from the decoder's planes, never skipping a frame (the [fast-decode](fast-decode.md) contract). The frame is squashed to 48×27 whatever its shape, as ffmpeg's `scale=48:27` does in the official code; portrait input is distorted the same way (a limit, below).

**Decision (a): scale in the decode loop, from the planes, with a box average, not from the 64×36 thumbnails and not with libswscale.**

- Each output pixel is the mean of a 4×4 grid of points inside its cell (so 20,736 reads per frame, about as many as the scene-score grid), converted to RGB by the same `pixel_rgb` the thumbnails and histograms use. A mean over a cell approximates the area filter ffmpeg applies; point sampling does not. The thumbnails cannot serve: they are point samples already, they are 64×36 (resampling to 48×27 cannot recover what the point sampling dropped), and only four are kept.
- `libswscale` (the safe `ffmpeg_next::software::scaling::Context`) is rejected because it decides the colour range from the pixel format, while the decoders disagree about it: the media engine returns `nv12`/`p010` with a range tag and software returns `yuvj420p`. `pixel_rgb` already follows the frame's range for every format and both decoders ([ADR-012](../decisions/ADR-012-decode-hevc-in-hardware-and-every-frame-exactly.md)), so the same file gives the same model input from either decoder. swscale would also need a second pass over each frame and a per-frame format check.
- The frames are collected only when TransNetV2 will run (Contract, "Choosing"), as `u8` in one buffer (3,888 bytes per frame: 7 MB for 60 s at 30 fps, 126 MB for the 18-minute NEET video). They become `f32` one window at a time (100 frames, 1.5 MB).
- The accuracy against the official ffmpeg-scaled input is measured on the fixtures and the two real reels and reported under "Measured": the largest difference between the probabilities, and whether the cut lists agree.

### Windows and post-processing (pure functions)

- `n` frames are padded to `25 + n + (75 - (n % 50 or 50))` frames: frame 0 copied 25 times in front, the last frame copied after. Window `k` is the padded frames `[50k, 50k + 100)`, `ceil(n / 50)` windows. The model's outputs `[25, 75)` of window `k` are the probabilities of the original frames `[50k, 50k + 50)`, so every frame is covered exactly once; the result is cut to `n` (the last window keeps fewer). A window function is any `FnMut(&[f32]) -> Result<Vec<f32>, E>` that takes `100 × 27 × 48 × 3` values and returns 100 probabilities; `predict` is tested with fakes.
- A **run** is a maximal stretch of frames with a probability above 0.5 (strictly greater, as in the official code). A run that starts at frame 0 or ends at the last frame has no shot on one side and gives no cut. Every other run gives one cut at the frame after the run's last frame, with `scene_score` set to the highest probability in the run (4 decimals). Two runs one frame apart give two cuts.
- A hard cut gives a run of one frame, so its cut equals the scene-score detector's. A dissolve gives a short run in its middle, so its cut is the model's best guess at the end of the transition, not the first frame that began to change (frames 110 and 208 on `dissolve12`, for transitions that begin at 96 and 192). `time_ms` and `interval_ms` follow from the frame the same way as for scene-score cuts.

### Choosing and failing (decision (c))

`--cut-detector` and `--transnet-weights` mean what [docs/cli.md](../cli.md) said, with the real conditions:

- `scene`: the scene-score detector, never TransNetV2.
- `transnet`: TransNetV2 or an error, checked **before the file is decoded**: the weights file must exist, `ORT_DYLIB_PATH` must name a loadable ONNX Runtime library of version 1.17 or newer, and the model must load and have an input `[1,100,27,48,3]` and an output named `534`. Any failure exits 1 with the reason, for example `TransNetV2 needs the ONNX Runtime library: set ORT_DYLIB_PATH to the full path of libonnxruntime.dylib (macOS: brew install onnxruntime, then /opt/homebrew/lib/libonnxruntime.dylib)`. An error while the model runs on the frames also exits 1. It never returns scene-score cuts.
- `auto`: TransNetV2 when `--transnet-weights` names an existing file and the library and model load; otherwise the scene-score detector. If a weights path was given and any of these checks fails, or the model fails while running, the scene-score cuts are used and `cut_detector_note` says why. No weights path at all is not a failure: the note is the scene-score note alone.
- `cut_detector` is `trans_net_v2` when the model produced the list and `ffmpeg_scene_score` otherwise; `cut_detector_note` says which and why.
- The 48×27 frames are only collected when the model will run, so a fallen-back `auto` costs nothing extra.

### Scene-score path

Output is unchanged for every existing fixture except `cut_detector_note`: `SCENE_NOTE` said "the ort runtime is not linked", which stops being true, so it now says that TransNetV2 runs instead when weights and a library are supplied. `cuts`, `silence_gaps`, `shot_histograms`, every PNG and every other field are byte-identical to the build before this module (checked by comparing the outputs of the old and new binaries on the fixtures, excluding `signal_ms`, `asr_ms` and `cut_detector_note`).

### Filmstrip PNGs

The `timeline_view` PNGs are made at the frames in `cuts`. In the decode loop they are captured at the scene-score cuts, so when TransNetV2 produced the cuts the boundaries are rebuilt from the stored 48×27 frames: four frames before the cut and four from the cut (fewer if the next cut is closer), at 48×27 rather than 64×36 (`thumb_width` and `thumb_height` in the view say so; the PNG is 768 px wide instead of 1024). The PNG count always equals the number of cuts.

### `Cut.scene_score`

For TransNetV2 cuts the field holds the single-frame probability (highest in the run), for scene-score cuts the FFmpeg score as before. Both are in 0 to 1; the two scales are not comparable, and `cut_detector` says which one a file holds. (The doc comment in `crates/edit-dna/src/lib.rs`, "TransNetV2 would store its cut probability here", becomes true; the comment was left alone.)

## Boundaries

- **Always:** call `decompile::release_onnx_runtime()` before the process exits (`reelctl`'s `main` does): with the shared environment still alive at exit, ONNX Runtime 1.21's static destructors abort the process (`mutex lock failed: Invalid argument`, exit status 134) after the output was written; `ort` builds one environment per process for good, so the model cannot be opened again after the release. Keep the scene-score output as it was (apart from the note); fail `--cut-detector transnet` with the reason instead of returning other cuts; check library, weights and model before decoding; call `ort::init_from` with a path we resolved, never `ort::init`; keep one environment per process; no `unsafe` outside `hwdecode.rs`; no network in tests; update the docs in the same commit.
- **Ask first:** making TransNetV2 the default when weights exist but no one asked for it; vendoring or downloading the library or the weights; a GPU or Core ML provider; changing the threshold.
- **Never:** commit the weights or a library; skip frames to save time; swallow an error into scene-score cuts under `transnet`.

## Testing Strategy

Test-first. No test in `cargo test` loads the model or the library.

- **Unit, no model:** padded length and window count for 1, 49, 50, 51, 100, 101 and 1000 frames; with a fake window function that returns the frame value it was given, `predict` returns every original frame once, in order, with the padding exactly as the official code (first 25 copies of frame 0, the tail copies of the last frame); runs: none, a hard-cut single frame, a longer run, two runs one frame apart, a run starting at frame 0, a run reaching the last frame, a threshold tie (0.5 is not above); cuts' frame indices, times, intervals and scores; the 4×4 sampling positions; the error messages: weights path missing, library path missing, `ORT_DYLIB_PATH` not set, a file that is not a library (a text file); the choice among `scene`, `transnet` and `auto` for each combination of weights present and library present (using a library path that does not exist); a fake-model detector returning a known probability track gives the expected `Cut` list.
- **End to end in `cargo test` (offline):** `--cut-detector transnet` with a missing library exits 1 with the message (before decoding: the output file does not exist); `auto` with a weights file that exists but no library gives the scene-score cuts and a `cut_detector_note` naming the library; `scene` gives the same cuts as before on the existing fixtures. Spawned `reelctl` processes lose `ORT_DYLIB_PATH` unless a test sets it.
- **Manual, on the real model (recorded in the spec's "Measured" and in the report; not skipped silently, not run by `cargo test`):** the `Commands` block above on `textured_cuts12`, `draft12`, `dissolve12` and both real reels, with the cuts of each detector side by side, and once on each decoder (`--decoder software`) to confirm they agree.

## Success Criteria

1. `textured_cuts12` gives cuts at frames 72, 144, 216 and `draft12` at frame 180 with `--cut-detector transnet`, the same as the scene score; `dissolve12` gives one cut inside each transition (frames 96 to 119 and 192 to 215) where the scene score gives none.
2. The scene-score path writes the same `edit_dna.json` and PNGs as before apart from `cut_detector_note` and timings, on every fixture in `p0_fixture.rs`.
3. The pure-code tests above pass with no model and no library, and `cargo test --workspace` is green offline.
4. Every failure under `--cut-detector transnet` names what is missing and how to fix it; `auto` falls back with the same reason in `cut_detector_note`.
5. The cost per frame, the added crates, the binary size and the build time are measured and written down.

## Measured

2026-10-28/29, Apple M3 MacBook Air, macOS 26.5, release build, ONNX Runtime 1.21.0 (conda wheel), load average 9 to 30 while measuring (it was 370 earlier in the session), so times are upper bounds. Command: `ORT_DYLIB_PATH=<lib> reelctl decompile --cut-detector transnet --transnet-weights ~/models/transnet/transnetv2.onnx --input <clip> --out <json>`; every run exited 0.

| Clip | Scene score finds (frame) | TransNetV2 finds (frame, `scene_score`) | Official ffmpeg-scaled input finds (frame, probability) |
|---|---|---|---|
| `textured_cuts12` | 72, 144, 216 | 72 (0.9628), 144 (0.9849), 216 (0.9910) | 72 (0.9649), 144 (0.9843), 216 (0.9937) |
| `draft12` | 180 | 180 (0.9982) | 180 (0.9982) |
| `dissolve12` | none | 110 (0.8750), 208 (0.6481) | 110 (0.8372), 208 (0.6233) |
| `textured_cuts12_hevc_full` (media engine and `--decoder software`) | not run | 72 (0.9635), 144 (0.9849), 216 (0.9910) from both decoders | |
| Reel "NEET question" (1080×1920 H.264, 38.5 s, 1,153 frames) | 254 (score 0.3032) | 254 (0.7893), 434 (0.5010), 653 (0.5255), 863 (0.7119) | 254 (0.7945), 653 (0.5465), 863 (0.6835); frame 433 is 0.4910, just under 0.5, so no cut at 434 |
| Reel "He shamelessly steals his kids milk" (720×1280 H.264, 15.1 s, 451 frames) | none | none | none (highest frame probability 0.0455) |

- **The soft transitions are found.** The one-second dissolve (frames 96 to 119) gives a cut at 110 and the fade through black (192 to 215) a cut at 208; the scene score gives none. On the NEET reel TransNetV2 confirms the one scene-score cut (254) and adds two the scene score missed (653, 863); nobody has looked at those frames, so whether they are real edits is not established. It also reports 434, which the official input puts just under the threshold.
- **Accuracy of the 4×4 box input against ffmpeg's `scale=48:27`.** Cut lists agree on every clip except one borderline frame on the NEET reel (0.5010 here against 0.4910 there). On the cuts both produce, the probabilities differ by at most 0.028 (863: 0.7119 against 0.6835); on the hard-cut fixtures by at most 0.0027. Only the probabilities at cut frames were compared (the in-loop path does not dump the whole track), so this is not a bound on every frame. Both decoders give identical results.
- **Cost.** NEET reel, alternating runs, `REELCTL_PROFILE=1`: `decode_ms` 493 and 508 with `--cut-detector scene`, 498 and 529 with `transnet`, so collecting the 48×27 frames adds 0 to 20 ms for 1,153 frames (within the noise; under 0.02 ms per frame). The model adds `analyze_ms` of 8,095 and 8,413 for 1,153 frames: about 7 ms per video frame (windows overlap by half, so about 3.5 ms per frame run), including opening the session. At that rate 60 s at 30 fps costs about 13 s, and `signal_ms` for these runs was 17,052 (loaded, first run), about 8.6 s, 2,001 and 1,832 ms for the 38 s reel and the 12 s clips.
- **Binary and build.** `target/release/reelctl` grows from 6,061,664 to 6,274,368 bytes (+212,704, +3.5%); `otool -L` shows no ONNX Runtime dependency. A full clean release build took 1 min 57 s before the change (machine busy); the added crates compile in a few seconds, so no clean-build comparison under the same load was taken.
- **Scene-score path.** Old and new binaries on `textured_cuts12`, `draft12`, `textured_cuts12_hevc`, `outlier12`, `motion12` and `dissolve12`: every field of `edit_dna.json` equal except `signal_ms`, `asr_ms` and `cut_detector_note`, and every PNG byte-identical.

## Limits

- Portrait input is squashed to 48×27 as the official input is, which distorts it; two portrait reels is all that was tried.
- The cut of a soft transition is the model's peak, inside the transition. It is not the first changed frame.
- The motion detector still reports events as before; a dissolve that TransNetV2 calls a cut may also produce an `overlay_change` on a frame that is not exactly the cut.
- The NEET reel's extra cuts (653, 863) and the borderline 434 are unreviewed.
- Memory: the 48×27 frames of an 18-minute clip are about 126 MB.
- ONNX Runtime 1.21.0 is the only library tried. Homebrew's 1.31.1 formula is untested (not installed); `api-17` should accept it, `ort` logs a compatibility notice for a newer minor version.
- `release_onnx_runtime()` makes a second use of the model in one process impossible; nothing in `reelctl` needs one.

## Resolved Questions

1. **How to get 48×27 frames (answered 2026-10-28, recommended): scale in the decode loop with a 4×4 box average from the planes**, not from the thumbnails and not with swscale; see Contract.
2. **How to obtain ONNX Runtime: `load-dynamic`**, library path from `ORT_DYLIB_PATH`, no search of default directories (a search would make "library missing" tests depend on the machine). `download-binaries` was rejected: it needs the network at build time, which breaks offline `cargo test` for everyone who has not built before.
3. **`auto` stays opt-in by weights**: TransNetV2 is about 7 ms per video frame (13 s for 60 s at 30 fps), so `auto` without a weights path never runs it.
4. **Filmstrips for TransNetV2 cuts** come from the 48×27 frames (small, exact to the cuts) rather than keeping a 64×36 thumbnail for every frame (6.9 KB per frame, 224 MB for 18 minutes).
5. **The note text of the scene-score path changes** because the old text is no longer true; nothing else in its output does.
