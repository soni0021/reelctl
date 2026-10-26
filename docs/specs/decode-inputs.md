# Spec: decode inputs

Status: approved 2026-10-26, with the recommended answer to each open question (see Resolved Questions). Task 1, reading 10-bit video, done 2026-10-26 ([ADR-011](../decisions/ADR-011-read-10-bit-video-from-its-top-8-bits.md)); the resampler and the real recording are next. Plan: [tasks/plan.md](../../tasks/plan.md). Module `decode-inputs` in the [P0 capability map](p0-capability-map.md). Branch: `feature/decode-inputs`.

## Objective

Two input problems make `reelctl decompile` fail or drift on ordinary footage:

1. **10-bit video is rejected.** iPhone recordings are HEVC Main 10. The test recording, `IMG_1435.MOV`, decodes to `yuv420p10le` with BT.2020 primaries, HLG transfer (`arib-std-b67`), and limited range. `decompile` stops with `decode video frame: unsupported pixel format; the fast path reads yuv420p or nv12`, so a creator has to convert every phone clip first. Android HDR and 10-bit H.264 exports decode to the same format.
2. **Audio that is not 48 kHz drifts early.** `push_audio` resamples each decoded frame on its own with `resample_linear`, and each frame drops its fractional output sample. For 44.1 kHz AAC (1,024 samples per frame) that is 0.56 samples per frame, about 24 samples or 0.5 ms per second. A pause planted at 50.0 s was found 10–40 ms early ([status.md](../status.md#known-issues)), about 0.5 s by the end of an 18-minute video. YouTube audio is often 44.1 kHz.

This change makes `decompile` read 10-bit 4:2:0 video directly, and resamples audio as one continuous stream. 8-bit input and 48 kHz audio give exactly the output they give today.

Behavior:

- **10-bit 4:2:0** (`yuv420p10le`, what FFmpeg's software HEVC, H.264, VP9, and AV1 decoders produce for 10-bit streams) is read where it is sampled today: the 160×90 luma grid for cut scores, the 32×18 color grid, and the 64×36 thumbnails. Each sampled value is the top 8 bits of its 10-bit sample, so limited-range 64–940 becomes 16–235, the same range 8-bit uses. A 10-bit frame is read as full range when its metadata says so, since FFmpeg has no full-range 10-bit pixel format; 8-bit full range is still `yuvj420p` only. No frame is converted as a whole, so sampling costs what it costs for 8-bit.
- **HDR is not tone-mapped.** Colors are computed from the coded values with the existing BT.601 conversion, as for SDR input (a known inaccuracy listed in status.md). For HLG and PQ footage, the per-shot colors and thumbnails are therefore flatter and less saturated than the picture a viewer sees. Cuts, silence, loudness, and words do not depend on color.
- `decode_note` says when the input was 10-bit and when it was HDR (HLG or PQ transfer), and that colors were not tone-mapped. No schema change.
- **Audio resampling keeps its position across frames,** so the output has `round(input samples × 48,000 / input rate)` samples within one, and a moment at time t in the input lands at t in the output. Audio already at 48 kHz is untouched.
- Other pixel formats (4:2:2, 4:4:4, 12-bit, P010) still stop with a clear error that names the format (Resolved Question 1).

Out of scope: tone mapping and BT.2020 or BT.709 color matrices, decode speed (module `fast-decode`), hardware decoding, and audio channel layouts beyond today's channel average.

## Evidence

- `IMG_1435.MOV`: `ffprobe` reports `hevc`, `Main 10`, `yuv420p10le`, `color_range=tv`, `color_space=bt2020nc`, `color_transfer=arib-std-b67`, `color_primaries=bt2020`.
- Homebrew FFmpeg 8.1's `libx264` and `libx265` both encode `yuv420p10le` (High 10 and Main 10), so a 10-bit test clip can be generated offline.
- FFmpeg decoding of the first 60 s of the original file took 6.23 s; the 8-bit conversion took 1.90 s ([capability map](p0-capability-map.md)). 10-bit HEVC will be slow until `fast-decode`.
- `ffmpeg-next` 9's default features include `software-resampling` (libswresample), which `decompile` already builds with.
- 44.1 kHz drift, measured on a tone with a mute planted at 50.0–50.5 s: 48 kHz AAC found 50,010–50,510 ms; 44.1 kHz AAC and 44.1 kHz PCM both found 50,000–50,470 ms.

## Tech Stack

No new dependencies. `ffmpeg-next` 9 with FFmpeg 8.1, as now.

## Commands

```bash
cargo check
cargo test
bash fixtures/generate.sh draft12_10bit /tmp/draft12_10bit.mp4
bash fixtures/generate.sh draft12_44k /tmp/draft12_44k.mp4
cargo build --release
target/release/reelctl decompile --input ~/Downloads/IMG_1435.MOV --out /tmp/img1435.edit_dna.json
```

## Project Structure

| Path | Change |
|---|---|
| `crates/decompile/src/decode.rs` | A sample accessor over 8-bit or 10-bit planes, used by luma sampling, color, and thumbnails; a stateful resampler; the `decode_note` text |
| `fixtures/generate.sh` | `draft12_10bit` (`draft12` encoded as 10-bit HEVC, with BT.2020 and HLG tags written into the bitstream by FFmpeg's `hevc_metadata` filter; tagging the encoder's output instead makes FFmpeg convert the pixels to the BT.2020 matrix) and `draft12_44k` (`draft12` with 44.1 kHz AAC audio and its mute moved to 50.0 s, 60 s long) |
| `crates/reelctl/tests/p0_fixture.rs` | End-to-end tests for both clips |
| Docs | `docs/cli.md` (input requirements), `docs/edit-dna.md` (`decode_note`), `docs/status.md`, CHANGELOG, and ADR-011 for reading 10-bit video from its top 8 bits without tone mapping |

## Code Style

```rust
/// One plane of a decoded frame, read one sample at a time as 8 bits.
enum Plane<'a> {
    Eight { data: &'a [u8], stride: usize },
    /// Little-endian 16-bit samples holding 10-bit values.
    Ten { data: &'a [u8], stride: usize },
}
```

Small free functions, errors as `DecompileError`, and the measurement behind each constant in the module comment, as in `silence.rs`.

## Testing Strategy

Test-first.

- **Unit tests, no FFmpeg files:**
  - The accessor returns `v >> 2` for 10-bit samples and the byte itself for 8-bit, at row and column offsets with strides wider than the width.
  - The resampler: 60 s of 44.1 kHz audio fed in 1,024-sample frames gives 2,880,000 samples ± 1; an impulse at 50.0 s comes out at 50.0 s ± 1 sample; 48 kHz input passes through unchanged.
- **End-to-end, default suite:**
  - `draft12_10bit` gives the same cut (± 1 frame), the same silence gaps, and the same payoff result as `draft12`, and per-shot mean RGB within 3 of it. `decode_note` mentions 10-bit and HLG.
  - `draft12_44k` finds the planted pause within 10 ms of 50,000–50,500 ms.
  - Every existing fixture's `edit_dna.json` is unchanged apart from timings.
- **Manual, real footage outside the repo:** the original `IMG_1435.MOV` decompiles, and its cuts and silence gaps equal those of an 8-bit H.264 conversion of the whole file with the audio stream copied. The whole file, because the silence threshold is computed over the whole recording.

## Boundaries

- **Always:** keep 8-bit and 48 kHz output identical; keep sampling sparse, with no whole-frame conversion; update the docs in the same commit.
- **Ask first:** tone mapping or color matrix changes; hardware decoding; any schema change.
- **Never:** commit real recordings; use the network in tests.

## Success Criteria

1. `reelctl decompile --input IMG_1435.MOV` exits 0, and its cuts and silence gaps equal those of an 8-bit conversion of the whole file with the audio copied.
2. `draft12_10bit` matches `draft12` as described under Testing Strategy, in the default test suite.
3. `draft12_44k` finds its planted pause within 10 ms of the true times; today it is 10–40 ms early.
4. All existing fixtures give identical `edit_dna.json` apart from `signal_ms`, `asr_ms`, and other timings, and the 60 s benchmark's `signal_ms` stays in its recent range (242–262 ms).
5. `decode_note` says 10-bit and HDR when they apply.
6. `cargo test` passes, and the docs listed under Project Structure are updated.

## Resolved Questions

Answered 2026-10-26 with the recommended options:

1. **Other pixel formats.** 8-bit 4:2:2 and 4:4:4 (some screen recorders and exports) and 10-bit 4:2:2 (ProRes) would need chroma sampling by format rather than a fixed half-size grid. No recording measured so far uses them; they keep a clear error naming the format. Not now.
2. **How to resample.**
   - **A (chosen):** keep linear interpolation, but carry the position and last sample across frames. It fixes the drift, leaves the interpolation unchanged, and is a small, unit-testable change.
   - **B:** libswresample through `ffmpeg-next`. Higher quality filtering and well tested, but it changes the samples of every resampled input, and so their silence gaps and levels, by small amounts.
3. **HDR colors.** Say in `decode_note` that they were not tone-mapped, and leave color correction to a later module together with the BT.601/BT.709 known issue.
