# Capability Map: Finish P0

Status: approved 2026-10-25, with the recommended answer to each decision below. Each module below gets its own spec in `docs/specs/<module id>.md`, approved before coding, in the build order at the end.

P0's success metric ([planer.md](../../planer.md), section 7): decompile any 60 s Instagram Reel into full millisecond `edit_dna.json` telemetry in under 500 ms, and output a side-by-side pacing diff against a draft video. What works today and what is measured is in [status.md](../status.md).

| Module id | Responsibility | Depends on | State |
|---|---|---|---|
| `relative-silence` | Silence gaps under room noise | — | Done, merged 2026-10-25 |
| `whisper-coverage` | (Done, merged 2026-10-26.) Stop whisper from silently dropping speech. In a full 18-minute run it returned no words for the first 30.4 s, though only 4.7 s of that was silent; 30 s and 60 s cuts of the same start transcribe it. On the iPhone clip it dropped 15.96–30.86 s. The cause is not known yet: appending the loudest passage to a 60 s cut did not reproduce it. Candidate fixes: transcribe in pieces split at silence gaps, and re-run any stretch with sound but no words | — | To spec |
| `decode-inputs` | Read 10-bit and HDR video (iPhone HEVC HLG, `yuv420p10le` / `p010`) instead of stopping with `unsupported pixel format`. Also resample audio that is not 48 kHz without drift: per-frame linear resampling drops fractional samples, so at 44.1 kHz a pause planted at 50.0 s is found 10–40 ms early, about 0.5 s by 18 minutes | — | [Spec](decode-inputs.md) approved 2026-10-26. 10-bit 4:2:0 read and audio resampled as one stream on `feature/decode-inputs`; real recording next |
| `fast-decode` | Meet 500 ms on 60 s of real 1080p footage. Today it takes 2.1 s, over 99% of it decoding, already frame-threaded. Measured with FFmpeg on that clip: skipping non-reference frames 1.61 s (826 of 1,441 frames), skipping all B-frames 2.91 s, keyframes only 0.43 s (36 frames). The original HEVC 10-bit file takes 6.23 s. Frame skipping alone cannot reach 500 ms, so the spec evaluates finding candidate cuts from keyframes and packet sizes, then decoding only around them, plus VideoToolbox for HEVC. If nothing keeps cuts frame-accurate under 500 ms, it proposes a revised target with numbers | `decode-inputs` (same decode loop) | To spec |
| `comparative-diff` | `diff` against one to three outliers: second-by-second cut rhythm, dead-air density, and speaking rate side by side with the draft, plus the existing flags. This is the main gap against the metric | — | To spec |
| `visual-motion` | Punch-in zoom curves and overlay or B-roll changes, so `static_window` stops flagging edited talking heads that have no camera cuts (the NEET video: 0 cuts in 18 min, but overlays and zooms throughout) | — | To spec, limited to edit events (decision 1) |
| `transnet-cuts` | TransNetV2 cut detection through `ort` | — | Deferred (decision 2) |

Build order: `whisper-coverage` → `decode-inputs` → `fast-decode` → `comparative-diff` → `visual-motion`. `transnet-cuts` is deferred. `whisper-coverage` goes first because it fixes wrong output from a feature already on `main`, and a Reel's hook is in its first seconds. The two decode modules share one code path, and `fast-decode` carries the most risk. `comparative-diff` only reads `edit_dna.json` files and could be built in parallel.

Decisions, answered by the user on 2026-10-25 with the recommended options:

1. **`visual-motion` in P0?** Yes, as a spec limited to detecting overlay changes and punch-in zooms as edit events that end a static window. Full Bézier zoom curves and `.cube` LUT extraction would stay in Pass 1b. This changes the AGENTS.md P0 gap list.
2. **`transnet-cuts`:** download the TransNetV2 ONNX weights? Not now. The scene-score detector finds the hard cuts in every clip tested so far; TransNetV2 matters for soft transitions, which `visual-motion` partly covers.
3. **If `fast-decode` cannot reach 500 ms without losing frame-accurate cuts,** which gives? Keep frame accuracy and revise the target to the measured time, recorded in an ADR.

Not in this map: P1–P3 and the `--whisper-language` flag. They are tracked in [status.md](../status.md#next-steps).
