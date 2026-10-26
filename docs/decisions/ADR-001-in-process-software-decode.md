# ADR-001: Decode in-process with ffmpeg-next software decoding

## Status
Accepted. Partly superseded by [ADR-012](ADR-012-decode-hevc-in-hardware-and-every-frame-exactly.md) on 2026-10-26: HEVC now decodes on the media engine, and the loop filter is no longer skipped on non-reference frames. Decoding in-process through `ffmpeg-next`, and sampling coarse grids from the decoded planes, stand.

## Date
2026-10-18. Recorded on 2026-10-20 from [decode.rs](../../crates/decompile/src/decode.rs) and [AGENTS.md](../../AGENTS.md).

## Context
P0's success metric is decompiling a 60-second reel in under 500 ms ([planer.md](../../planer.md), section 7). planer.md suggests decoding on the GPU. AGENTS.md requires `ffmpeg-next`, with no `ffmpeg` subprocess on the structural path.

## Decision
- Decode with libavcodec through `ffmpeg-next`, in-process and in software: frame threading with one thread per available core, and the in-loop deblocking filter skipped on non-reference frames (`skip_loop_filter = nonref`).
- Take every measurement from the decoded planes inside the decode loop, on coarse grids: about 160×90 luma samples for the scene score, about 32×18 samples for color, and 64×36 thumbnails for the filmstrip. Frames are never fully converted to RGB or kept in memory.

## Alternatives Considered

### VideoToolbox hardware decoding
- Measured on the 60 s 1080p fixture: `ffmpeg -hwaccel videotoolbox` took 3.8 s, and multithreaded software decoding took 0.38 s.
- `ffmpeg-next` has no wrapper for creating a hardware device.
- Rejected: ten times slower on this workload.

### Running the `ffmpeg` command
- Ruled out by AGENTS.md. No further reason was recorded.

### GPU decoding through gpu-video or Smelter
- Named in planer.md. Not evaluated for P0.

## Consequences
- The synthetic 60 s benchmark takes about 0.3 s of structural time on an Apple M3 ([status](../status.md)). Real footage is unmeasured.
- Skipping the loop filter makes non-reference frames slightly blockier. Its effect on scene scores and histograms has not been measured.
- Only 8-bit 4:2:0 frames (`yuv420p`, `yuvj420p`, `nv12`) are read; 10-bit input fails.
- On flat color, the subsampled scene score matches a full-frame score. On textured footage the difference has not been measured.
- Building needs the FFmpeg development libraries, found through `pkg-config`, and clang for the generated bindings.
