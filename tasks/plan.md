# Implementation Plan: fast decode

Spec: [docs/specs/fast-decode.md](../docs/specs/fast-decode.md), approved 2026-10-26. Branch: `feature/fast-decode`. Checklist: [tasks/todo.md](todo.md). Earlier plans are in [tasks/archive/](archive/decode-inputs/plan.md).

## Overview

Make `decompile` faster on HEVC without changing any output: decode HEVC on the Apple media engine, fall back to software on any error, and record honestly what stays out of reach (500 ms is unmet on real footage). Frame skipping is out of scope; the spec rejects it with measurements. Five tasks, ordered so the exactness test exists before the hardware path does.

## Architecture Decisions

- **Exactness is checked, not assumed.** Task 1 adds the check before any hardware code: a fixture with planted cuts (`textured_cuts12`), baselines of every fixture, and software outputs for the known-cut corpus (outside the repo). The hardware path must reproduce them.
- **Hardware frames become ordinary frames before sampling.** `av_hwframe_transfer_data` copies each VideoToolbox frame into a software `nv12` (8-bit) or `p010` (10-bit) frame, and the existing sampling code reads it. `p010` stores the 10-bit value in the top bits of 16, so its top 8 bits are the high byte: `Plane::Ten` gets a shift (2 for `yuv420p10le`, 8 for `p010`). No tone mapping, no new sampling.
- **Task 2 holds only what is live before the hardware path:** the decoder choice and the new `decode_note` sentences would have no caller until Task 3, and the project allows no `#[allow(dead_code)]`, so they move there.
- **Choice per codec, by measurement:** HEVC tries hardware (8.9 s → 4.0 s); H.264 and everything else use software (hardware is 11.2 s against 3.0 s). The choice is a small pure function with unit tests.
- **Fallback covers three points:** no device at creation, an error opening the decoder, and an error on the first frame. After the first frame decodes, an error is an error. Each reason becomes a sentence in `decode_note`.
- **`--decoder <auto|software>`** on `decompile` (default `auto`) forces the software path. It is needed for the comparison tests and as an escape hatch, and it is documented in `docs/cli.md`.
- **All `unsafe` in `hwdecode.rs`,** compiled only on macOS (`cfg(target_os = "macos")`), each block with a `// SAFETY:` line. FFmpeg's raw bindings come from `ffmpeg_next::ffi`; no new dependency.
- **The loop-filter change is its own task,** because it can alter scene scores. It lands only if the corpus gives the same cuts and scores within 0.005.

## Task List

### Phase 1: The exactness check
- [x] Task 1: Baselines, the `textured_cuts12` fixture, and the corpus tooling

### Phase 2: The hardware path
- [x] Task 2: `p010` planes (the decoder choice and `decode_note` moved to Task 3, where they are used)
- [ ] Task 3: VideoToolbox decode with fallback and `--decoder`

### Checkpoint A
- [ ] `cargo test` passes; all fixtures unchanged apart from timings; corpus and the iPhone file give the same results with hardware as with software

### Phase 3: Software tuning and close-out
- [ ] Task 4: Loop filter off for all frames, if cuts stay identical
- [ ] Task 5: Measurements and docs

### Checkpoint B
- [ ] All six spec success criteria verified, reviewed with the user, merged into `main`

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Hardware frames differ from software frames (colour range, plane layout, rounding) | High | Task 3's equality tests on `draft12_10bit`, `textured_cuts12`, the corpus, and the iPhone file. A difference is a bug to find before the speed matters |
| VideoToolbox frame order or timestamps differ with `has_b_frames` | Medium | The tests compare cut frame indices and times to software; a mismatch fails them |
| Hardware decode errors partway through a file | Medium | Only the first frame falls back; a later error stops with a message naming the decoder. Measured on the iPhone file and the corpus. If errors appear mid-file in practice, widen the fallback to restart in software |
| The `unsafe` code leaks a buffer or frees one twice | Medium | One module, `// SAFETY:` lines, a test that decodes the 12 s fixture 50 times in one process. FFmpeg's reference counts are handled by the `ffmpeg-next` frame wrapper |
| CI or a teammate builds on Linux | Low | `hwdecode.rs` and its tests are `cfg(target_os = "macos")`; other platforms always use software |
| The hardware test cannot run (no media engine) | Low | It skips itself with a printed reason, and the software tests still run |
| Loop-filter-off changes a score enough to move a cut | Low | Task 4 is conditional, and the corpus decides |

## Open Questions

None. The spec's three questions were answered on 2026-10-26.
