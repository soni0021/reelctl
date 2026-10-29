# Plan: studio-service (M1)

Spec: [docs/specs/studio-service.md](../docs/specs/studio-service.md) (approved 2026-10-29). ADRs: [ADR-028](../docs/decisions/ADR-028-a-local-daemon-over-a-unix-socket-runs-jobs-as-child-reelctl-processes.md) and [ADR-029](../docs/decisions/ADR-029-a-sqlite-catalogue-a-content-addressed-store-and-append-only-checkpoints.md). Branch: `claude/keen-clarke-1a8w5h`. Built in a cloud session, where every part of this milestone can be built and tested.

The previous plan is archived in `tasks/archive/make-reels/`. Its one open item is a real-lecture run on the Mac.

## Order

Each task is one commit with its docs: `cargo test` before the commit, the `audit` subagent after it.

1. **`--progress json` and clean cancel** in `reelctl`. This is the event schema, the hooks in the long loops, a SIGTERM handler that stops children and removes temporary files, and a test that output without the flag is unchanged.
2. **`crates/studio`: store and catalogue.** BLAKE3 objects, tree objects, the SQLite schema version 1 and its migrations, and assets.
3. **Checkpoints and the cache.** Append, head, revert, branch, diff by role, the cache key, and `force`.
4. **The queue.** Classes and limits, priority, `after` chains, cancel, interrupted jobs on restart, and consumers that run a child and ingest its outputs. Tests use a fake runner.
5. **The command catalogue.** Typed arguments to argv and environment for every command. `make-reels` moves onto it, with no change to its behaviour (its tests must pass unchanged).
6. **`reelctl serve` and the socket API.** The lock, the peer uid check, JSON-RPC and notifications, the `understand.plan`/`understand.run` gate, `store.gc`, and `doctor.check`.
7. **`pipelines.make_reels`.** The M0 chain as jobs with `after` links on reel subjects.
8. **End to end over the socket.** `serve_fixture.rs`, covering the spec's success criteria.

## Docs touched

`docs/cli.md` (`serve`, `--progress`), a new `docs/studio.md` (the API reference), `README.md`, `docs/status.md`, `CHANGELOG.md`, `AGENTS.md` (add `crates/studio`; `serve` opens a Unix socket and no network port), the spec, and `.gitignore` (`.reelstudio/`).
