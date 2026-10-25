# Capability Map: Finish P0

Status: draft, 2026-10-25, awaiting the user's approval. Each module below gets its own spec in `docs/specs/<module id>.md`, approved before coding, in the build order at the end.

P0's success metric ([planer.md](../../planer.md), section 7): decompile any 60 s Instagram Reel into full millisecond `edit_dna.json` telemetry in under 500 ms, and output a side-by-side pacing diff against a draft video. What works today and what is measured is in [status.md](../status.md).

| Module id | Responsibility | Depends on | State |
|---|---|---|---|
| `relative-silence` | Silence gaps under room noise | — | Done, merged 2026-10-25 |
| `ten-bit-decode` | Read 10-bit and HDR video (iPhone HEVC HLG, `yuv420p10le` / `p010`) instead of stopping with `unsupported pixel format` | — | To spec |
| `fast-decode` | Meet 500 ms on 60 s of real 1080p footage. Today it takes 2.1 s, over 99% of it decoding, already frame-threaded. The main lever is skipping non-reference frames, since 72% of the real test clip's frames are B-frames, while keeping cut times frame-accurate. If no option gets there, the spec proposes revising the target with numbers | `ten-bit-decode` (same decode loop) | To spec |
| `comparative-diff` | `diff` against one to three outliers: second-by-second cut rhythm, dead-air density, and speaking rate side by side with the draft, plus the existing flags. This is the main gap against the metric | — | To spec |
| `visual-motion` | Punch-in zoom curves and overlay or B-roll changes, so `static_window` stops flagging edited talking heads that have no camera cuts (the NEET video: 0 cuts in 18 min, but overlays and zooms throughout) | — | **Needs a decision:** AGENTS.md assigns zoom curves to Pass 1b, "not this tree" |
| `transnet-cuts` | TransNetV2 cut detection through `ort` | — | **Needs a decision:** a weights download, which needs an explicit go-ahead |

Build order: `ten-bit-decode` → `fast-decode` → `comparative-diff`, then `visual-motion` and `transnet-cuts` if approved. The two decode modules go first because they share one code path and `fast-decode` carries the most risk, so it should fail fast. `comparative-diff` only reads `edit_dna.json` files and could be built in parallel.

Decisions for the user:

1. **`visual-motion` in P0?** Recommended: yes, as a spec limited to detecting overlay changes and punch-in zooms as edit events that end a static window. Full Bézier zoom curves and `.cube` LUT extraction would stay in Pass 1b. This changes the AGENTS.md P0 gap list.
2. **`transnet-cuts`:** download the TransNetV2 ONNX weights (kept outside the repo, like the whisper models)? Recommended: not now. The scene-score detector finds the hard cuts in every clip tested so far; TransNetV2 matters for soft transitions, which `visual-motion` partly covers.
3. **If `fast-decode` cannot reach 500 ms without losing frame-accurate cuts,** which gives? Recommended: keep frame accuracy and revise the target to the measured time, recorded in an ADR.

Not in this map: P1–P3, the `--whisper-language` flag, and the known issue where whisper dropped 15 s of speech on one clip (see [status.md](../status.md#known-issues)). They are tracked in status.md.
