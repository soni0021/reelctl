# Implementation Plan: whisper coverage

Spec: [docs/specs/whisper-coverage.md](../docs/specs/whisper-coverage.md), approved 2026-10-25. Branch: `feature/whisper-coverage`. Checklist: [tasks/todo.md](todo.md). Earlier plans are in [tasks/archive/](archive/relative-silence/plan.md).

## Overview

Make whisper's output cover the speech that is there, and keep a whisper.cpp abort from ending `reelctl`. Four tasks:

1. Pure functions that find holes and merge re-run words (no model).
2. The second pass, with padding, inside the current process. This is where the measurable gain is, and it is checked on the NEET video and the iPhone clip at Checkpoint A.
3. Move whisper into a child process, with the DTW-off retry.
4. Measurements and docs.

## Architecture Decisions

- **One word timeline, ends computed last.** Today `words_from_segments` sets each word's end from the next word or gap as it maps segments. With re-runs, a word next to a hole only knows its neighbour after merging. So passes return timed starts (text, start, segment end), `coverage::merge` combines them, and word ends are computed once at the end with the existing rule.
- **A hole starts at a word's start**, so its re-run transcribes that word again. A re-run's words therefore replace the first pass's words inside the hole, which avoids duplicates. A re-run that finds nothing leaves the first pass as it was. Settled in Task 1.
- **`coverage.rs` holds the pure logic:** `find_holes(starts, gaps, duration_ms)`, `merge(first, reruns)`, and dropping words that start at or after the end of the audio. Its constants (3 s hole, 1.5 s of sound, 1 s padding) carry their evidence in the module comment.
- **Passes reuse one loaded model.** A `Transcriber` holds the `WhisperContext` and the DTW preset, and runs `full` on a slice of the 16 kHz audio with 1 s of silence appended. Re-runs reuse it, so the model loads once per process.
- **The child process.** `reelctl` gets a hidden subcommand, `reelctl whisper-worker`, so `--help` does not show it.
  - The parent writes the 16 kHz audio to a temporary file, and sends the model path and the list of passes (offset, length, DTW on or off) on stdin as JSON.
  - The worker loads the model once and prints one JSON line per finished pass, so the passes completed before an abort are kept.
  - `decompile` gets a `WhisperRunner`: `InProcess` (library default, used by unit tests) or `Worker { program, args }`. `reelctl` passes its own executable from `std::env::current_exe()`.
  - If the worker dies from a signal, the parent re-runs the unfinished passes with DTW off. Those words start at their segment timestamps, and `asr_note` says which passes fell back. If that also fails, `decompile` exits with code 1 and an error.
- **Testing the abort without a model.** The runner's program is a parameter, so a unit test points it at `/bin/sh -c 'kill -ABRT $$'` and checks that the parent reports the abort and retries. No test-only switch is added to production code.
- **Timing.** Everything above counts toward `asr_ms`, including spawning the worker and writing the temporary file, and stays out of `signal_ms`.

## Task List

### Phase 1: Coverage
- [x] Task 1: Holes and merging (pure functions)
- [x] Task 2: Re-transcribe holes, with padding, in-process

### Checkpoint A
- [x] `cargo test` passes; spec criteria 1–5 hold on the NEET video, the iPhone clip, and the clean cuts (criterion 5 measured in Task 4)

### Phase 2: Abort safety and close-out
- [x] Task 3: Run whisper in a child process, with the DTW-off retry
- [x] Task 4: Measure and close out

### Checkpoint B
- [ ] All seven spec success criteria verified, reviewed with the user, merged into `main`

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Re-runs still come back empty on some holes, as the 30.4 s NEET cut did before padding | Medium | Padding fixed that case in the probe. Empty holes are reported with their times in `asr_note`. One round only, so this cannot loop |
| Splitting word-end logic out of `words_from_segments` changes output on runs without holes | Medium | Spec criterion 4: identical `words` for `speech12` with both models and the five clean cuts, compared against JSON saved before Task 2 |
| No synthetic clip reliably makes whisper drop a window | Medium | Task 2 tries a few (speech after a long tone, speech that starts at 29 s). If none works, the hole path is tested by unit tests plus the manual real-footage runs, as the spec allows |
| The worker adds about 1 s per process for loading the model, and temporary-file I/O | Low | One worker per decompile, plus one more only after an abort. Measured in Task 4 against the 10% criterion |
| The worker cannot find its own executable, for example when `decompile` is used as a library | Low | The library defaults to `InProcess`; only `reelctl` picks `Worker` |
| The NEET run takes about 7 minutes per try | Low | Iterate on the 60 s iPhone clip and on cuts. Run the full video once at Checkpoint A and once in Task 4 |

## Open Questions

None. The spec's three questions were answered on 2026-10-25.
