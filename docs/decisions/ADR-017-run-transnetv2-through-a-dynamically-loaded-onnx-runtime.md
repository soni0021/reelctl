# ADR-017: Run TransNetV2 through a dynamically loaded ONNX Runtime

## Status
Accepted. Supersedes [ADR-002](ADR-002-scene-score-cut-detector.md) in part: `TransNetV2Detector` no longer always errors. The scene-score detector, the `CutDetector` trait and `--cut-detector scene` are unchanged.

## Date
2026-10-28. Written under the user's blanket "Do all of them" and not reviewed line by line. Spec: [transnet-cuts](../specs/transnet-cuts.md).

## Context
ADR-002 left TransNetV2 unlinked because its weights were not available and P0 did not download models. The user has since allowed the download (`~/models/transnet/transnetv2.onnx`, MIT). The scene score cannot see dissolves and fades. Three choices needed an answer: how `ort` gets ONNX Runtime, how the model gets its 48×27 frames, and what `auto` does.

## Decision
- **`ort 2.0.0-rc.15` with `load-dynamic`** (features `std`, `load-dynamic`, `api-17`), pinned exactly because no stable 2.0 exists. The ONNX Runtime library is found through `ORT_DYLIB_PATH` and nowhere else. The code calls `ort::init_from(path)`, which returns an error, never `ort::init()`, whose lazy loader panics. One environment is built per process and cached, because `ort` allows only one.
- **48×27 frames come from the decode loop,** a 4×4 box average of the decoder's planes through `pixel_rgb`, collected only when the model will run. Not from the 64×36 thumbnails (point samples) and not from libswscale (it decides the colour range from the pixel format, which the two decoders report differently).
- **A run of frames above 0.5 is one cut, at the frame after the run;** a run touching either end of the clip gives none. `scene_score` holds the highest probability in the run.
- **`transnet` is checked before decoding and never falls back;** `auto` falls back to the scene score with the reason in `cut_detector_note`. `auto` uses TransNetV2 only when a weights path was given.
- **Filmstrips for TransNetV2 cuts are rebuilt from the 48×27 frames,** so the PNGs match `cuts`.

## Alternatives Considered

### `ort` with `download-binaries`
- Fetches ONNX Runtime during the build. A fresh checkout could not run `cargo test` offline, and the build would depend on a third-party host. Rejected.

### Scale with libswscale
- Closest to the official `ffmpeg -vf scale=48:27`, but the range handling differs between the software and media-engine decoders, so one file could give two model inputs.

### Keep a 64×36 thumbnail for every frame and resample
- 6.9 KB per frame (224 MB for 18 minutes), and resampling point samples cannot undo their aliasing.

### Make TransNetV2 the default when weights exist
- It costs about 7 ms per video frame (see the spec), so a default would add about 13 s to a 60 s reel's decompile. Opt-in by weights path keeps the default fast.

## Consequences
- A process that ran the model must call `decompile::release_onnx_runtime()` before it exits (`reelctl` does): ONNX Runtime 1.21 aborts at exit if its environment is still alive (exit status 134 after the output was written). `ort` allows one environment per process for good, so nothing can open the model again after the release.
- The default build links no ONNX Runtime and needs no download; running TransNetV2 needs a library of version 1.17 or newer on the machine (conda's `onnxruntime` wheel and Homebrew's formula both ship one).
- `signal_ms` includes TransNetV2 (cut detection is structural), so a run with weights is far over 500 ms.
- Portrait video is squashed to 48×27 as the official input is; how well the model copes with that is only measured on two reels.
- The motion detector still treats scene-score cuts as the cuts it must not report an event on; with TransNetV2 cuts a dissolve may still produce an overlay event.
