# ADR-029: A SQLite catalogue, a content-addressed store, and append-only checkpoints

## Status
Accepted (with [studio-service](../specs/studio-service.md), 2026-10-29)

## Date
2026-10-29

## Context
The user wants a local file store and a database "which checkpoints and things can be reverted", and chose a checkpoint for every job's outputs. Outputs range from small JSON files (edit DNA, `.otio`) to large renders. Most commands are deterministic: the same inputs give the same bytes.

## Decision
- **Files.** Outputs live in `objects/<aa>/<blake3>`, immutable and read-only. A directory output is a tree object: a JSON manifest of names, hashes and sizes. Inputs such as source videos are recorded by path, size, mtime and hash, not copied, unless asked.
- **The catalogue** is SQLite (`rusqlite` with the `bundled` feature, in WAL mode), with numbered migrations. Its tables are assets, objects, subjects, jobs, job_events and checkpoints. Secrets are never stored; only the names of the environment variables a job got.
- **History.** Every succeeded job appends a checkpoint to its subject and moves the subject's head. Revert appends a copy of an earlier checkpoint. Branch starts a new subject from one. Nothing is deleted except by explicit garbage collection, which removes only unreferenced objects and defaults to a dry run.
- **Cache.** A job's cache key is the hash of its command, normalised arguments, input hashes and the `reelctl` version. A match reuses the earlier outputs, marked `cached`. Live `understand` calls are never cached.

## Consequences
- **Two new dependencies:** `rusqlite`, which compiles SQLite in, so the build gets slower but no system library has to match, and `blake3`.
- **Undo is cheap.** It is a new row pointing at existing objects, so reverting a render costs no disk.
- **Disk grows.** Renders accumulate until the user collects garbage. The app's Storage view shows the size per subject.
- **Determinism matters.** It holds for the edit operations. `signal_ms` and model calls are not deterministic, so cached results are labelled and `force` re-runs them.
