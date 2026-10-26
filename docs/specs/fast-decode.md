# Spec: fast decode

Status: **draft, awaiting approval** (2026-10-26); question 1 answered (A), questions 2 and 3 open. Module `fast-decode` in the [P0 capability map](p0-capability-map.md). Branch: `feature/fast-decode`. The user's direction for this module: quality is the goal, time is not.

## Objective

P0's success metric ([planer.md](../../planer.md), section 7) is a full `edit_dna.json` for a 60 s Reel in under 500 ms. `signal_ms` is over 99% decode, and decode is far from 500 ms on real footage:

| Input, 60 s equivalent | `signal_ms` today |
|---|---|
| 1080p H.264 8-bit (the real iPhone clip converted) | about 2.1 s |
| 1080×1920 HEVC 10-bit (the original iPhone recording) | about 6.1 s |

The earlier capability-map plan was to find candidate cuts from keyframes and packet sizes, and decode only around them. **This spec rejects that plan** because it loses cuts (see Evidence), and a cut that goes missing silently is worse than a slow run. The contract of this module is instead:

> **Every output the full decode produces (cuts and their scores, per-shot colors, thumbnails) stays exactly what it is. Speed comes only from decoding the same frames more cheaply, never from skipping frames.**

That is a hard limit on how fast `decompile` can get. This spec says how far exact decoding goes on this machine, builds it, and states plainly what stays out of reach, instead of claiming 500 ms.

Behavior:

- **HEVC decodes on the Apple media engine (VideoToolbox) when it works,** because it is the one exact option that is clearly faster. On the real iPhone file: 4.0 s against 8.9 s in software, and 5.7 CPU-seconds against 62. H.264 stays on the software decoder, where hardware is slower (11.2 s against 3.0 s).
- **Anything that goes wrong in hardware falls back to software,** with the same output: no device, an unsupported profile, an error on the first frame.
- Frames from the hardware decoder come back as `nv12` (8-bit) or `p010` (10-bit), and are sampled at the same points as today. `p010` holds a 10-bit value in the top bits of 16, so its top 8 bits are the high byte, and the values equal the software decoder's `yuv420p10le >> 2`.
- `decode_note` says which decoder ran, and why software ran when hardware was tried.
- Software decoding skips all loop filtering instead of only non-reference frames, **only if** the known-cut corpus gives the same cuts and scores within 0.005. It saved 10–15% in the measurements below.

Out of scope: sampling or skipping frames, an approximate "preview" mode (see Resolved Questions), a different cut detector, other platforms' hardware decoders, the audio path (0.08 s), and the 10-bit tone-mapping question.

## Evidence

All on the Apple M3 MacBook Air (8 cores, fanless), FFmpeg 8.1, macOS 26.5, 2026-10-26, load average 2–6 so single numbers vary by about 10%.

**Decode cost, and how much is left to gain.**

| Measurement | Result |
|---|---|
| `IMG_1435.MOV`, 87.3 s, HEVC Main 10, 1080×1920, 2,103 frames, software, default threads | 8.8–8.9 s real, 60.5–62.6 s CPU (7 cores busy) |
| Same, one thread | 42.7 s |
| Same, hardware (VideoToolbox) | 4.0 s real, 5.7–5.9 s CPU |
| Same file as 8-bit H.264 High (1080p reference), software | 3.0 s real, 21.7 s CPU |
| Same, hardware | 11.2–11.5 s |
| Demuxing the 175 MB file without decoding | 0.28 s (mostly start-up and disk) |
| Reading all packet sizes and keyframe flags of the file | 0.05 s |
| Audio decode | 0.08 s |
| Corpus, HEVC 10-bit, software / hardware (full-size read-back) / hardware with GPU downscale to 160×90 | 3.8 s / 2.3 s / 2.8 s. The media engine decodes about 630 frames/s whatever the output size, so the GPU downscale adds nothing |
| Software knobs, corpus H.264 / HEVC: default, loop filter off for non-reference frames (today), for all frames | 1.78 / 3.75 s, 1.67 / 3.70 s, 1.53 / 3.43 s |

Software decoding already uses 7 of 8 cores, so 500 ms for a full decode would need about 5–10 times more compute than this machine has. Hardware HEVC is the only large exact gain, and at 630 frames/s it bottoms out near 2.3 s per 60 s of 24 fps video.

**Why not decode fewer frames.** A known-cut corpus was built outside the repo: 60 s, 1080×1920, 24 fps, 11 segments alternating between `IMG_1435.MOV` clips from different minutes, `testsrc2`, and `mandelbrot`, with 10 hard cuts at known frames. It was encoded four ways: x264 High and x265 Main 10, each with a keyframe every 24 frames and 3 B-frames, with scene-cut detection on (x264 `-sc_threshold` default; x265 `scenecut=40`) and off (x264 `-sc_threshold 0`, x265 `scenecut=0`).

| Check | Result |
|---|---|
| Today's full decode finds the planted cuts | 10 of 10 in all four encodes, at the planted frame ± 1 |
| A keyframe sits at the cut | 7 of 10 with scene-cut detection on, in both codecs; fewer where it is off |
| The cut frame's packet size, ranked among non-keyframes by size relative to its neighbours (1 is the biggest), scene-cut detection off | x264: 278, 13, 537, 70, 210, 7, 458, 72, 162, 6. x265: 46, 306, 322, 51, 53, 354, 264, 52, 38, 296 |
| The one cut in `IMG_1435.MOV` (frame 1726) | Not a keyframe, and its packet is 84 KB against a median of 83 KB, so size does not mark it |

Packet size and keyframes miss many cuts. Comparing only keyframes would also miss a cut and a cut back inside one second, or two cuts within a second, and fast cutting is what Reels do. No frame-skipping scheme tried is exact, so none is in this spec.

Not measured: the corpus uses synthetic and one real source, and no scraped Reels (and none may be added to the repo). Whether hardware and software frames are bit-identical is unmeasured and is the first thing the implementation checks.

## Tech Stack

`ffmpeg-next` 9 with FFmpeg 8.1, as now. Hardware decoding needs raw FFmpeg calls that `ffmpeg-next` does not wrap (`av_hwdevice_ctx_create`, a `get_format` callback, `av_hwframe_transfer_data`), through `ffmpeg-sys-next`, which `ffmpeg-next` already depends on. This is the first `unsafe` code in the repo. It lives in one module, `hwdecode.rs`, each block with a safety comment. No other new dependency.

## Commands

```bash
cargo check
cargo test
cargo build --release
target/release/reelctl decompile --input ~/Downloads/IMG_1435.MOV --out /tmp/img1435.edit_dna.json
REELCTL_PROFILE=1 target/release/reelctl decompile --input <reel.mp4> --out <edit_dna.json>
bash fixtures/generate.sh textured_cuts12 /tmp/textured_cuts12.mp4
```

## Project Structure

| Path | Change |
|---|---|
| `crates/decompile/src/hwdecode.rs` | New: VideoToolbox device, `get_format` callback, frame transfer, the only `unsafe` |
| `crates/decompile/src/decode.rs` | Choose hardware or software per codec; `p010` plane support; `decode_note` says which ran |
| `fixtures/generate.sh` | `textured_cuts12`: 12 s, 640×360, hard cuts between textured sources, B-frames, scene-cut detection off. A small stand-in for the corpus |
| `crates/reelctl/tests/p0_fixture.rs` | End-to-end tests |
| Docs | `docs/status.md` (measurements, the revised statement of the target), `docs/cli.md`, `docs/edit-dna.md` (`decode_note`), ADR-012, ADR-001 superseded in part, README, CHANGELOG, the capability map |

## Code Style

```rust
/// Why the hardware decoder was not used, for `decode_note`.
enum HwSkip {
    Codec,
    NoDevice(String),
    FirstFrame(String),
}
```

Small functions, errors as `DecompileError`, each `unsafe` block with a `// SAFETY:` line naming what makes it sound, and the measurement behind each choice in the module comment, as in `silence.rs`.

## Testing Strategy

Test-first.

- **Unit tests, no media:** `p010` plane reads return the high byte of each 16-bit sample, with strides wider than the row; the codec-to-decoder choice (HEVC tries hardware, H.264 and the rest use software); `decode_note` text for each case.
- **End-to-end, default suite:**
  - `textured_cuts12` gives the planted cuts, from both decoders where hardware is available. The hardware test skips itself, with a printed reason, on a machine with no VideoToolbox.
  - `draft12_10bit` is HEVC, so it takes the hardware path: its `edit_dna.json` equals the software result.
  - The existing fixtures are unchanged apart from timings.
- **Manual, outside the repo:** the known-cut corpus in all four encodings. The new path's `edit_dna.json` equals the software path's: same cuts, scores within 0.005, shot colors and thumbnails equal. Then the real iPhone file, three alternating runs against software.

## Boundaries

- **Always:** produce the same cuts, scores, colors, and thumbnails as a full software decode; fall back to software on any hardware error; keep every `unsafe` block in `hwdecode.rs` with a safety comment; update the docs in the same commit.
- **Ask first:** any mode that skips or samples frames; changing `planer.md`'s 500 ms metric; hardware decoding for H.264; tone mapping.
- **Never:** commit real recordings or the corpus; use the network in tests; claim a speed-up that changes an output.

## Success Criteria

1. **Same results.** On the known-cut corpus in all four encodings, and on the original iPhone file, the hardware path gives the same `cuts` (same frame indices), scene scores within 0.005, and equal per-shot colors and thumbnails as the software path.
2. **Faster on HEVC.** On the original iPhone file, `signal_ms` is at least 40% lower than software (measured 8.9 s → 4.0 s, 55%), in three alternating runs.
3. **Safe fallback.** With hardware forced off, and with a file the hardware decoder rejects, the output equals the software output, and `decode_note` says why software ran.
4. **Nothing else moves.** All existing fixtures give identical `edit_dna.json` apart from timings, and the 60 s H.264 benchmark's median `signal_ms` stays within 242–262 ms.
5. **`decode_note`** names the decoder used.
6. **Tests and docs.** `cargo test` passes and the docs under Project Structure are updated, including the measured per-class cost in [status.md](../status.md), where the 500 ms target is recorded as unmet for real footage, with numbers.

## Open Questions

1. **The 500 ms metric.** *Resolved: A, 2026-10-26.* With exact decoding the best case here is about 2.8 s per 60 s of iPhone HEVC and about 2.1 s of 1080p H.264. Choose one:
   - **A (recommended):** keep exactness as the contract. Record the target as unmet on real footage, with these numbers, in status.md, and leave `planer.md` unchanged for you to revise.
   - **B:** you amend the metric in `planer.md` now (for example to a decode-time budget per 60 s of footage) and I mirror it in the docs.
   - **C:** add an opt-in `--preview` mode that samples frames and may miss cuts, labelled as approximate in the output. Not recommended: the evidence shows it misses cuts on exactly the footage the tool exists for, and it would ship as "fast" while hiding that.
2. **Hardware decoding, with the first `unsafe` code in the repo.** The alternative is software only, which gives no speed-up. Recommended: yes, behind the fallback and the same-results check.
3. **All loop filtering off in software.** Saves 10–15%. Recommended: adopt it only if criterion 1 holds with it; otherwise leave it as is.
4. **`visual-motion`, the next module,** needs dense frames to see zooms and overlays. This spec keeps every frame available, which sampling would not. No action needed; recorded so the two modules stay compatible.

## Resolved Questions

1. **The 500 ms metric (answered 2026-10-26): A.** Exactness is the contract. 500 ms is recorded in [status.md](../status.md) as unmet on real footage, with the measured numbers; `planer.md` stays unchanged for the user to revise. Questions 2 and 3 are still open.
