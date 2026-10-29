# ADR-030: Cancel with a flag, a child registry and a forced exit

Status: Accepted (2026-10-29)

## Context

`reelctl serve` cancels a job by sending SIGTERM to its child `reelctl` (ADR-028). That child may be decoding in-process, waiting on ffmpeg, the whisper worker, manimgx or Smelter, or about to write its output. A cancelled job must stop soon, leave no temporary file, and never write a result file that the service would then ingest as if the job had succeeded. `#![forbid(unsafe_code)]` holds everywhere but `hwdecode.rs`, so the signal handling and the kill must use safe APIs.

## Decision

- **Flag.** `ctrlc` (with its `termination` feature: SIGINT, SIGTERM and SIGHUP) sets an atomic cancel flag in `crates/progress`. Long loops (decode, sampling, TransNet, matting, the render's progress reader) poll it and return an error; every command calls `progress::check()` before it writes a result file, and `understand` before the model call. `main` turns any error after a cancel into exit 130 with an `error` event.
- **Children.** Every child process is registered with `progress::track` (or run through `progress::tracked_output`) while it runs. On a cancel the handler sends each one SIGTERM through `nix::sys::signal::kill`, a safe wrapper over `kill(2)`, rather than running a `kill` process.
- **Temporary files.** Each temporary file and folder guard (`TempFile`, `TempDir`, the compositor's `TempFile`) registers its path with `progress::temporary`. Normally the guard's `Drop` removes it.
- **Forced exit.** A second signal, or a run still going 10 s after the first, calls `force_exit`: SIGTERM to the registered children, removal of the registered temporary paths (their guards will not run), an `error` event, exit 130.

## Consequences

- New dependency `nix` (default features off, `signal` only), in `crates/progress`.
- SIGHUP also cancels, which the `termination` feature brings; closing the terminal stops a run, as before (the default action ended it too, without clean-up).
- A pid is signalled only while its `Tracked` guard is alive. The guard is dropped just after `wait` returns, so for that instant the registry holds a pid that is already reaped and could in principle be reused by an unrelated process; a forced exit landing in that window could signal it. The window is microseconds; dropping the guard before reaping would need the child's exit seen without reaping it.
- The forced exit releases the ONNX Runtime environments, as `main` does, before `std::process::exit`. A model session still running on another thread keeps its environment alive, and then ONNX Runtime's static destructors can abort the process at exit: the exit code is then 134 (SIGABRT) instead of 130, after the children were signalled and the temporary paths removed. `nix` 0.31 has no safe `_exit`, so this stays a known gap; a loop that holds a model and ignores the flag for 10 s is a bug in that loop.
- Short probes (`ffprobe -show_streams`, the `ps` in a test) are not tracked; they end by themselves within a second.
