# Status

Last updated 2026-10-20. Update this page in the same commit as any change that affects it.

## Summary

P0, the Rust decompiler and reel-diff CLI, is built and its tests pass. It meets the 500 ms target on the synthetic 60-second benchmark clip but misses it on real footage: 60 s of an iPhone recording, converted to 8-bit H.264, took 2.1–2.8 s ([First real footage](#first-real-footage-2026-10-20)). `diff` does not yet compare pacing against the outlier. P1–P3 in [planer.md](../planer.md) have not started.

## Verified 2026-10-20

Apple M3 (8 cores, 16 GB), macOS 26.5, rustc 1.98.1, FFmpeg 8.1.

| Check | Result |
|---|---|
| `cargo check --workspace --all-targets` | Passes with 23 warnings (see Known issues) |
| `cargo test --workspace` | 17 of 17 pass: 16 unit tests and the end-to-end fixture test |
| 60 s benchmark, release build | `signal_ms` 310, 316, 284 over three runs; the target is under 500 |
| 12 s fixtures (640×360), release build | `signal_ms` 11–21 |

The benchmark clip (`bash fixtures/generate.sh bench60`) is 1920×1080 at 30 fps: 30 s of solid red, then 30 s of solid white, over a sine tone, encoded as H.264 Constrained Baseline without B-frames. Flat color with simple encoding is about as cheap to decode as video gets. Real footage has texture and motion and is usually encoded with B-frames, so its `signal_ms` is not known yet.

`signal_ms` is not wall-clock time. It leaves out process start, word timing, and PNG writing ([ADR-005](decisions/ADR-005-signal-ms-measures-the-structural-path.md)). The whole benchmark command took 0.30–0.80 s.

Reproduce the benchmark:

```bash
cargo build --release
bash fixtures/generate.sh bench60 /tmp/bench60.mp4
REELCTL_PROFILE=1 target/release/reelctl decompile \
  --input /tmp/bench60.mp4 --out /tmp/bench60.edit_dna.json
```

## First real footage, 2026-10-20

The input was an 87 s iPhone recording: HEVC Main 10 with HDR (HLG), 1080×1920 at 24 fps with B-frames, AAC stereo, and a 4-channel spatial-audio track. It ran on the same machine as above. The file stays outside the repo.

| Check | Result |
|---|---|
| `decompile` on the original file | Stops with `decode video frame: unsupported pixel format`, because the video is 10-bit |
| First 60 s converted to 8-bit H.264 High (`-vf format=yuv420p -c:v libx264 -preset fast -crf 20 -profile:v high`), release build | `signal_ms` 2828, 2205, 2087 over three runs, against a target of 500. Decoding was over 99% of it |
| FFmpeg decoding every frame of that file in software | 2.12 s (680 fps) |
| FFmpeg decoding with VideoToolbox | 5.71 s |
| FFmpeg decoding keyframes only | 0.44 s, but only 62 of 1,440 frames, about one per second |
| Silence gaps at −40 dBFS | 26 gaps, the longest 190 ms, so no pause of 0.3 s or more. The room noise sits between −30 and −25 dB; at −25 dB there are 19 pauses of 0.3 s or more |
| whisper `base.en`, on the `feature/whisper-asr` branch | One invented English sentence, repeated. The speech is probably not English, or is too noisy for this model |

What this means:

- Decoding every frame at 1080p cannot meet 500 ms on this machine, because FFmpeg alone takes about 2.1 s. Meeting the target needs a different decode strategy, or a different target.
- A fixed −40 dBFS threshold misses pauses when there is room noise, so `diff` would report no dead air in a recording like this.

## What works

- `reelctl decompile` writes `edit_dna.json` with cuts, silence gaps, integrated loudness, per-shot RGB histograms, and the 8–10 s payoff cue, plus one `timeline_view` PNG per cut.
- `reelctl diff` flags dead air over 180 ms, more than 2.5 s without a cut, and a missing payoff cue when the outlier has one.
- The JSON Schema is generated from the Rust types and matches them.

## Gaps

Already listed in [AGENTS.md](../AGENTS.md) as P0 gaps:

| Gap | Effect now | To close it |
|---|---|---|
| whisper.cpp not linked | `words` are voiced bursts named `burst-N`; `words_per_minute` counts bursts, not words | In progress. `cmake` 4.4.4 and `~/models/whisper/ggml-base.en.bin` are installed; the [spec](specs/whisper-asr.md) awaits approval |
| TransNetV2 not linked | Every cut comes from the scene-score detector; `--cut-detector transnet` exits with an error | TransNetV2 ONNX weights and an `ort` dependency |
| Zoom curves and `.cube` LUT | Reported as `"status": "unimplemented"` | Pass 1b work |

Not yet listed in AGENTS.md:

- **`diff` barely uses the outlier.** Dead air and static windows are properties of the draft alone; the outlier only feeds the payoff check. `diff` takes one outlier, where planer.md asks for one to three and a second-by-second pacing comparison. This is the main gap against the P0 success metric.
- **Static windows count only cuts.** planer.md defines a static window as more than 2.5 s without B-roll, zoom, or a graphic callout. Nothing detects zooms or graphics yet, so a punch-in without a cut is still flagged.
- **The 500 ms target fails on real footage.** Only one real recording has been run so far ([First real footage](#first-real-footage-2026-10-20)). The tests still use only synthetic clips ([ADR-006](decisions/ADR-006-synthetic-offline-fixtures.md)).
- **The silence threshold is fixed at −40 dBFS.** On the real recording it found no pause of 0.3 s or more, because room noise sat above it.
- **10-bit video is rejected.** Decoding stops with `unsupported pixel format` on anything other than 8-bit 4:2:0. That rules out iPhone HDR recordings until they are converted, which the real-footage run confirmed.
- **`--draft` takes an MP4.** planer.md specifies a `.otio` timeline; OpenTimelineIO is P2 work.

## Known issues

- `cargo check` reports 22 warnings in library and binary code (`unreachable_patterns`, `unreachable_code`), plus 1 in a `diff` unit test. The 22 come from 11 `other => { ... }` arms placed after `match` arms that already cover every case. They do not change behavior.
- In a `timeline_view` PNG, the thumbnails and the waveform use different time scales. The 8 thumbnails cover about 0.27 s at 30 fps; the waveform and word ticks cover 1.5 s either side of the cut, across the same width.
- The JSON value for TransNetV2 is `"trans_net_v2"`, but `CutDetectorKind::as_str()` returns `"transnet_v2"`. Nothing emits either until TransNetV2 is linked.
- Colors are converted from YUV with BT.601 coefficients, while most HD video uses BT.709, so absolute RGB values are slightly off.
- The quality interview in [CONSTRAINTS.md](../CONSTRAINTS.md) has not been run, so only the floor rules plus `cargo check` and `cargo test` are enforced.

## Next steps

In progress: real word timing with whisper.cpp ([spec](specs/whisper-asr.md), branch `feature/whisper-asr`).

Then, to finish P0 against its own success metric before starting P1:

1. Find a decode strategy that meets 500 ms on real footage, or revise the target.
2. Read 10-bit video, such as iPhone HDR recordings.
3. Detect silence relative to each recording's noise floor instead of at a fixed −40 dBFS.
4. Make `diff` comparative: accept one to three outliers and compare cut rhythm second by second.
5. Link TransNetV2. It needs ONNX weights, a download that needs an explicit go-ahead.
