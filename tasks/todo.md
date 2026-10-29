# Todo: studio-service (M1)

Plan: [plan.md](plan.md). Spec: [docs/specs/studio-service.md](../docs/specs/studio-service.md). Tick each box in the commit that completes it. Stage explicit paths; never `git add -A` (`.env` holds a key).

## Task 0: Spec
- [x] User approves the spec, ADR-028 and ADR-029 (2026-10-29; workspace ~/Movies/ReelStudio, stops when the app says)

## Task 1: Progress and cancel
- [x] `--progress json` events from the decode, whisper, render, matte and measure loops and the make-reels steps; the result event equals the stdout line (TransNet windows and manimgx stages: not yet, the worker and model calls report as their stage)
- [x] SIGTERM stops children and leaves no temporary file; without the flag, output is unchanged

## Task 2: Store and catalogue
- [ ] Objects, trees, read-only files; schema version 1 and migrations; assets by reference

## Task 3: Checkpoints
- [ ] Append, head, revert, branch, diff; cache key and `force`

## Task 4: Queue
- [ ] Classes, priority, `after`, cancel, interrupted on restart; consumers ingest outputs

## Task 5: Command catalogue
- [ ] Typed args to argv and env for every command; `make-reels` moved onto it, its tests unchanged

## Task 6: `reelctl serve`
- [ ] Socket, lock, peer uid; JSON-RPC and events; understand plan/run gate; gc; doctor

## Task 7: `pipelines.make_reels`
- [ ] The M0 chain as jobs and checkpoints

## Task 8: End to end
- [ ] `serve_fixture.rs` covers the success criteria
