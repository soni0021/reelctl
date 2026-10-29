# Todo: studio-service (M1)

Plan: [plan.md](plan.md). Spec: [docs/specs/studio-service.md](../docs/specs/studio-service.md). Tick each box in the commit that completes it. Stage explicit paths; never `git add -A` (`.env` holds a key).

## Task 0: Spec
- [x] User approves the spec, ADR-028 and ADR-029 (2026-10-29; workspace ~/Movies/ReelStudio, stops when the app says)

## Task 1: Progress and cancel
- [x] `--progress json` events from the decode, sample, TransNet, whisper, render, matte and measure loops and the make-reels steps; the result event equals the stdout line (manimgx stages: not yet, the worker reports as its stage)
- [x] SIGTERM stops children, writes no further output and leaves no temporary file; without the flag, stdout and outputs are unchanged (audit fixes: ADR-030)

## Task 2: Store and catalogue
- [x] Objects, trees, read-only files; schema version 1 and migrations; assets by reference

## Task 3: Checkpoints
- [x] Append, head, revert, branch, diff; the cache key and the cached-outputs lookup (`force` is the queue's, task 4)

## Task 4: Queue
- [x] Classes, priority, `after`, cancel, interrupted on restart; consumers ingest outputs (with a fake runner; the real one is task 5/6)

## Task 5: Command catalogue
- [x] Typed args to argv and env for every command a job runs (not `dataset build`, whose inputs are folders: later); `make-reels` moved onto it, its tests unchanged

## Task 6: `reelctl serve`
- [x] Socket, lock, peer uid; JSON-RPC and events; understand plan/run gate; gc; doctor (checkpoint diff by hash only; the DNA metrics and `.otio` differences are later)

## Task 7: `pipelines.make_reels`
- [ ] The M0 chain as jobs and checkpoints
  - [x] The step list and clip choice as pure code in `crates/studio/src/pipeline.rs` (2 tests); `make-reels` and `pipelines.make_reels` not yet moved onto it

## Task 8: End to end
- [ ] `serve_fixture.rs` covers the success criteria
