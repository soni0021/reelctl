# ADR-012: Decode HEVC on the media engine, and decode every frame exactly

## Status
Accepted. Supersedes the VideoToolbox rejection and the loop-filter skip in [ADR-001](ADR-001-in-process-software-decode.md).

## Date
2026-10-26

## Context
Decoding is over 99% of `signal_ms`, and on real footage it is far from the 500 ms target: the 87 s iPhone HEVC recording took 8.4 s, and 1080p H.264 about 2.1 s per 60 s. Software decoding already keeps 7 of 8 cores busy, so more threads cannot help. The first plan was to find cuts from keyframes and packet sizes and decode only around them. The [spec](../specs/fast-decode.md) rejects it with measurements: on a 60 s corpus with 10 planted cuts, a keyframe sat at only 7 of them, and with scene-cut keyframes off the cut frame's packet size ranked anywhere from 6th to 537th among non-keyframes. The project's direction for this module is that quality matters and time does not.

## Decision
- **No frame is skipped.** Outputs (cuts, scene scores, colors, thumbnails) must equal those of a decode of every frame.
- **HEVC is decoded on the Apple media engine (VideoToolbox)** when running on macOS and the setting is `auto`. H.264 and everything else are decoded in software. On the iPhone recording the media engine took 4.0 s against 8.9 s, using 5.7 CPU-seconds against 62. For 1080p H.264 it took 11.2 s against 3.0 s.
- **Frames are copied to system memory** (`nv12` or `p010`) and sampled like any other frame. All `unsafe` code, the first in the workspace, is in `hwdecode.rs`, with a `// SAFETY:` comment on each block.
- **Any failure restarts the whole decode in software.** At any frame, for any reason, `decode_file` decodes the file again from the start with the software decoder, and `decode_note` gives the reason. If software fails too, its error is the one returned. A decode depends on nothing but the file, so this is simple and complete. The decoder offered to libavcodec is only VideoToolbox, and every received frame is checked, so "hardware" in the note means every frame came from it.
- **Software decoding filters every frame.** It no longer skips the loop filter on non-reference frames. With that skip, HEVC scene scores differed from the hardware decoder's by up to 0.022 on frames just after a cut; without it, the software and hardware scores are equal to four decimals on every fixture, the 4-encode corpus, and the iPhone recording. The skip saved 6% of decode time. Skipping it on all frames would have saved 14% and moved scores more.
- **`--decoder <auto|software>`** on `decompile` and `diff` forces software. `auto` is the default.

## Alternatives Considered

### Decode only keyframes and the frames around likely cuts
- 0.43 s for the keyframes of a 60 s clip, but it misses a cut and a cut back inside one GOP, two cuts within a second, and any cut the packet data does not mark. Measured above. Rejected: a missed cut is silent.

### Hardware decoding for H.264 too
- Slower on the measured file (11.2 s against 3.0 s). Rejected.

### Scale on the GPU before copying (`scale_vt`)
- 2.8 s against 2.3 s for the full-size copy on the corpus. The media engine decodes about 630 frames/s whatever the output size, so there is nothing to gain.

### Keep skipping the loop filter on non-reference frames
- Saves 6% and makes the two decoders disagree. Rejected given the direction on quality.

### Fall back only when the first frame fails
- Leaves a mid-file hardware error as a hard failure on a file software can decode. Restarting the decode is as simple.

## Consequences
- The 500 ms target is still unmet on real footage. Exact decoding gives about 2.1 s per 60 s of 1080p H.264 and, on this machine, roughly 3 s per 60 s of iPhone HEVC. [status.md](../status.md) records the numbers. `planer.md` is unchanged for the user to revise.
- Hardware has a fixed cost of about 160 ms, so small HEVC files are slower with it (a 12 s 640×360 clip: 183 ms against 23 ms in software). Real Reels are 1080p, where it wins from about 3–4 s of video. `--decoder software` is the override.
- Every `decode_note` changes: its software sentence no longer says the loop filter is skipped. Cuts, scores, colors, thumbnails, and PNGs of the existing fixtures are identical.
- The media engine returns every 8-bit frame as `nv12`, which has no full-range variant, so range follows the frame's metadata in every format (`yuvj420p` is also full range). This supersedes ADR-011's rule that 8-bit range comes from the pixel format alone. The frame copy reuses its buffers, so it starts again from an empty frame when the decoded size changes and reports an error if the copied size differs. It also frees the side data the reused frame still holds before copying properties, which otherwise grew without bound on HDR10 video. The audit found both gaps after the first merge-ready commit; each now has a failing test (`textured_cuts12_hevc_full`, `resize12_hevc`).
- A decode that falls back runs twice on the failing file. The cost is only on failure.
- Other platforms always use software. The hardware module compiles there as a stub.
- `visual-motion` keeps every frame available, which sampling would not have.
