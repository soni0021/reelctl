# ADR-010: Run whisper in a child process

## Status
Accepted

## Date
2026-10-25

## Context
whisper.cpp 1.8.3 aborts the whole process with `WHISPER_ASSERT(filter_width < a->ne[2])` when DTW timestamps are on and a 30 s window advances only a few frames, so the window has fewer audio tokens than the median filter's width of 7. A probe with `reelctl`'s settings hit it on real audio. Upstream fixed it on 2026-10-19 ([ggml-org/whisper.cpp#4096](https://github.com/ggml-org/whisper.cpp/pull/4096), commit `a46fadb36a`), after the latest release (v1.9.5), and whisper-rs 0.16.0, the newest crate, bundles 1.8.3. In-process, an abort would end `reelctl` with no message, because whisper.cpp's logs are discarded.

## Decision
- `reelctl` runs whisper in a child `reelctl whisper-worker`, hidden from `--help`. The parent writes the 16 kHz audio to a temporary file once, then sends one JSON request per line on the child's stdin; the child loads the model once and answers one JSON line per pass.
- If the child dies, the parent runs that pass again in a fresh child loaded without DTW, records it for `asr_note`, and goes back to a DTW child for the passes after it. If the retry dies too, `decompile` fails with exit code 1 and an error naming both.
- The temporary file is removed when the run ends, on success or failure.
- The `decompile` library keeps an in-process runner as its default, for unit tests and for callers that are not `reelctl`; `reelctl` falls back to it if it cannot find its own executable.
- Chosen by the user on 2026-10-25 over waiting for a whisper-rs release (option B) and building against an unreleased whisper.cpp revision (option C) ([spec](../specs/whisper-coverage.md#resolved-questions)).

## Alternatives Considered

### Wait for a whisper-rs release with the fix
- Leaves the abort in place for an unknown time.

### Patch whisper-rs-sys to a whisper.cpp revision with the fix
- Fixes the cause, but builds against unreleased code and leaves the published crates.

### Catch the abort in-process
- SIGABRT from `abort()` cannot be recovered from safely inside the process.

## Consequences
- An abort costs one extra model load and one pass without DTW, whose words start at segment timestamps.
- Worker start-up was not measurable above run-to-run noise: three alternating runs of `speech12` with Apex took 9.7–12.5 s through the worker and 13.0–18.2 s in-process on `main`.
- When whisper-rs ships whisper.cpp with #4096, the worker can stay as a guard or be removed in a new ADR.
