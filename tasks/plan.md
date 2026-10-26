# Implementation Plan: decode inputs

Spec: [docs/specs/decode-inputs.md](../docs/specs/decode-inputs.md), approved 2026-10-26. Plan approved 2026-10-26. Branch: `feature/decode-inputs`. Checklist: [tasks/todo.md](todo.md). Earlier plans are in [tasks/archive/](archive/whisper-coverage/plan.md).

## Overview

Read 10-bit 4:2:0 video, and resample non-48 kHz audio as one stream. Three tasks: 10-bit reading first, because it touches every pixel read and so carries the risk to identical 8-bit output; then the resampler; then real footage and close-out.

## Architecture Decisions

- **Every plane read already goes through `YuvView`.** `sample_luma` reads `view.y`; `pixel_rgb` reads luma and chroma for color and thumbnails. So `YuvView`'s byte slices become a `Plane` accessor with `at(row, col) -> u8`: 8-bit planes return the byte, and 10-bit planes return the little-endian 16-bit sample shifted right by 2. Strides stay in bytes, as FFmpeg reports them.
- **`chroma_planes` gains a `yuv420p10le` branch** next to `yuv420p`/`yuvj420p` and `nv12`. Other formats keep an error, which now names the format.
- **`decode_note` is built per file:** the existing text, plus a sentence when the video was 10-bit and when its transfer is HLG or PQ (`AVCOL_TRC_ARIB_STD_B67`, `AVCOL_TRC_SMPTE2084`), read from the first decoded frame.
- **The resampler becomes a struct** that `decode_file` keeps across audio frames: input rate, the fractional read position, and the last input sample, so interpolation can span a frame boundary. 48 kHz frames bypass it, as now. Its output length over a stream is `round(n × 48,000 / rate)` within one sample.
- **Fixtures:** `draft12_10bit` re-encodes `draft12`'s sources with `libx265` Main 10 (`yuv420p10le`), like an iPhone. `draft12_44k` is 60 s of `draft12`-style video and tone with 44.1 kHz AAC audio and a mute at 50.0–50.5 s, the point where per-frame drift shows.
- **Baseline first:** save every existing fixture's `edit_dna.json` from the current build before Task 1. After each task, all of them must match apart from timings.

## Task List

### Phase 1: Inputs
- [x] Task 1: Read 10-bit 4:2:0 video
- [x] Task 2: Resample audio as one stream

### Checkpoint A
- [x] `cargo test` passes; all existing fixtures unchanged apart from timings

### Phase 2: Real footage and close-out
- [x] Task 3: Real footage, measurements, and docs

### Checkpoint B
- [ ] All six spec success criteria verified, reviewed with the user, merged into `main`

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| The `Plane` accessor changes an 8-bit value or slows the hot path | Medium | The baseline comparison covers every fixture. The benchmark's `signal_ms` must stay in 242–262 ms, measured alternately against the current build |
| x265 10-bit encoding differs enough from x264 8-bit that `draft12_10bit`'s cut moves | Low | The criterion allows ± 1 frame; flat colors and a hard cut leave no ambiguity |
| HEVC 10-bit decodes slowly (about 6 s per 60 s at 1080p), which makes the default test suite slow | Low | `draft12_10bit` is 640×360 and 12 s |
| The original file's first decoded frame lacks transfer metadata | Low | Then the note says only 10-bit; checked on `IMG_1435.MOV` in Task 3 |
| The 4-channel spatial-audio track gets picked instead of the AAC stereo track | Low | Checked in Task 3; `decode_file` uses FFmpeg's best audio stream, as the 8-bit conversion's copied stream shows |

## Open Questions

None. The spec's three questions were answered on 2026-10-26.
