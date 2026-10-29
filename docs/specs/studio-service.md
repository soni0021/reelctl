# Spec: The studio service (`reelctl serve`)

Status: **approved 2026-10-29** by the user ("Approve, start coding"), with the answers in Resolved Questions. Asked for on 2026-10-29. The user wanted to "shift to request/response model API and producer consumer model and add local filesystem and database which checkpoints and things can be reverted". They chose a Rust daemon, a Unix socket, and a checkpoint for every job's outputs. It is milestone M1 of the ReelStudio plan: the native SwiftUI Mac app (M2 and on) is a thin client of this service. Branch: `claude/keen-clarke-1a8w5h`.

## Objective

Today every `reelctl` command is a one-shot process. Nothing remembers what was run, a result cannot be taken back, and a GUI would have to build command lines and parse result lines itself. The service gives one local process that:

- takes **requests and returns responses** (JSON-RPC 2.0 over a Unix socket), so the Mac app and scripts drive `reelctl` the same way;
- **queues jobs** (producers) and runs them in **worker pools** (consumers), several at once where the machine allows, in order where a job needs another's output;
- keeps a **local store of every output** (content-addressed files) and a **SQLite catalogue** of assets, jobs, events and checkpoints;
- makes **every finished job a checkpoint** that can be **reverted to**, **branched from** and **compared**, without ever deleting history;
- keeps today's rules: no network port, the API key never stored, each live `understand` call confirmed with its cost, and one ONNX model per process.

The CLI stays exactly as it is. The service runs the same `reelctl` binary as child processes, the way `make-reels` already does ([ADR-027](../decisions/ADR-027-make-reels-runs-each-step-as-a-child-reelctl-with-a-minimal-environment.md)).

## Tech Stack

- A new crate, **`crates/studio`**. It holds the store, the catalogue, checkpoints, the cache, the queue, the command catalogue (typed arguments to argv and environment) and the protocol types. It does not decode video or call a model; it runs `reelctl`.
- **`crates/reelctl/src/serve_cmd.rs`**: the socket edge and the consumer threads.
- New dependencies:
  - **`rusqlite`** with the `bundled` feature, so SQLite is compiled in and there is no system library to match ([ADR-029](../decisions/ADR-029-a-sqlite-catalogue-a-content-addressed-store-and-append-only-checkpoints.md));
  - **`blake3`** for content hashes.
- No async runtime: std threads, `std::os::unix::net::UnixListener`, and one condition variable per pool.

## Design

### 1. The workspace on disk

A workspace is a folder the user picks (`reelctl serve --workspace <dir>`). The service owns `<dir>/.reelstudio/`, created with mode 0700:

| Path | What it holds |
|---|---|
| `studio.db` | The SQLite catalogue, in WAL mode |
| `objects/<aa>/<blake3>` | Immutable output files, read-only (0400), named by content hash. A directory output, such as a filmstrip folder or a reel folder, is a tree object: a JSON manifest of name, hash and size |
| `jobs/<job-id>/` | A job's scratch directory. The child writes here; on success the outputs are hashed, moved into `objects/`, and the directory is removed |
| `reelctl.sock` | The API socket, 0600 |
| `serve.lock` | One `serve` per workspace: a second exits 2 and names the running one's pid |

Source videos and other inputs are **imported by reference**: their absolute path, size, mtime and hash are recorded. They are not copied, because videos are large. `assets.import` with `copy: true` copies one in for a portable workspace. The repo's `Reels/` is never written to.

### 2. The catalogue (SQLite, schema version 1)

| Table | Columns |
|---|---|
| `schema_meta` | `version` |
| `assets` | `id`, `kind` (`video`, `edit_dna`, `understanding`, `otio`, `chart_spec`, `cube`, `jsonl`, `other`), `name`, `source_path`, `size`, `mtime`, `hash`, `created_at` |
| `objects` | `hash` (primary), `size`, `media_type`, `created_at` |
| `subjects` | `id`, `kind` (`reel`, `project`, `look`, `presenter`, `understanding`, `dataset`, `eval`, `make_reels`), `name`, `parent_subject`, `head_checkpoint`, `created_at` |
| `jobs` | `id`, `subject`, `command`, `args` (JSON, typed), `inputs` (JSON: role to hash), `class`, `priority`, `status`, `after` (JSON: job ids), `cache_key`, `from_checkpoint`, `exit_code`, `result` (JSON: the result line's fields), `error`, `env_names` (JSON: names only), `created_at`, `started_at`, `finished_at` |
| `job_events` | `job`, `seq`, `ts`, `event` (JSON), with primary key `(job, seq)` |
| `checkpoints` | `id`, `subject`, `parent`, `job`, `label`, `outputs` (JSON: role to hash), `cached` (bool), `note`, `created_at` |

`status` is one of `queued`, `blocked` (waiting on `after`), `running`, `succeeded`, `failed`, `cancelled` or `interrupted`.

Migrations are explicit and numbered. An unknown `version`, newer than this binary, exits 2 and names it. **No secret is ever stored**: `jobs.env_names` records the names of the variables a child got, never their values.

### 3. Checkpoints and revert

- **Every job makes one.** A `succeeded` job writes one checkpoint on its subject, with `parent` set to the subject's head, and moves the head to it. An edit of a project takes the head's `.otio` as its input, so a chain of edits is a chain of checkpoints.
- **Revert** (`checkpoints.revert {subject, checkpoint}`) appends a **new** checkpoint whose outputs equal the target's, labelled `revert to #N`, and moves the head to it. Nothing is deleted or rewritten, so a revert can itself be reverted.
- **Branch** (`subjects.branch {checkpoint, name}`) makes a new subject whose first checkpoint copies the given one. It is for trying a second edit path from one cut.
- **Diff** (`checkpoints.diff {a, b}`) lists the output roles that are the same, changed, or only in one, by hash. (Planned, not built: `reelctl diff`-style metrics for two edit DNAs, and clip and metadata differences for two `.otio`.)
- **Cache.** `cache_key = blake3(command, normalised args, input hashes, reelctl version)`. A new job whose key matches a succeeded job makes its checkpoint at once, marked `cached`, with no child process. `force: true` runs it anyway, for example to measure `signal_ms` again. `understand` live calls are **never** cached.
- **Garbage collection** (`store.gc {dry_run}`) removes only objects no checkpoint, job input or asset refers to. A dry run, which is the default, returns the files and bytes it would free. Nothing is collected without an explicit `dry_run: false`.

### 4. The queue (producers and consumers)

- **Producers.** The API handlers are the producers. `jobs.submit` validates the typed arguments against the command catalogue (section 6), resolves inputs to hashes, and inserts the job in one transaction: `queued`, or `blocked` when it names unfinished `after` jobs. Then it signals its pool's condition variable.
- **Consumers.** Each resource class has a pool of threads with a limit that can be changed at run time (`workspace.set_limits`):

  | Class | Commands | Default |
  |---|---|---|
  | `heavy` | decompile, diff, look, render, qa, compositor, reframe (presenter), overlay add-chart | 1 |
  | `light` | timeline, transform (punch-in, auto-punch-in, reframe centre), funnel, overlay captions and remove, dataset, eval | 4 |
  | `network` | understand | 1 |

  A consumer takes the oldest highest-priority runnable job and builds its argv and environment from scratch, as `make-reels` does: PATH, TMPDIR, HOME and the library paths, plus only what the command needs. It runs `reelctl <command> --progress json` in the job's scratch directory with stdin null. It reads the NDJSON on stderr into `job_events` and to subscribers, and on exit it ingests the outputs and writes the checkpoint, all in one transaction.
- **`after` chains.** A job's `after` jobs must all succeed first. An input may name an output of an `after` job (`{"from": "<job-id>", "role": "out"}`), which is resolved when the job becomes runnable, and makes that job an `after` job. An input may also be a stored hash, or `{"asset": <id>}`: an imported file, read where it is once its size and hash are checked again (a changed file fails the job). If an `after` job fails or is cancelled, every job waiting on it fails with `"after job <id> failed"`.
- **Cancel** (`jobs.cancel`): a queued or blocked job is cancelled at once. A running one gets SIGTERM, then SIGKILL after 5 s, and its scratch directory is removed.
- **Restart.** When `serve` starts, `running` jobs become `interrupted`, so they can be retried with `jobs.retry`, and `queued` and `blocked` jobs carry on. If the app quits, jobs keep running; a client that reconnects asks for the events it missed by sequence number.

### 5. The API (JSON-RPC 2.0, one JSON object per line, over the socket)

| Method | Params | Result |
|---|---|---|
| `workspace.info` | none | `{path, version, schema_version, limits, pid}` |
| `workspace.set_limits` | `{heavy?, light?, network?}` | the new limits |
| `doctor.check` | none | each tool and file with `found`, `version`, `path`: ffmpeg, ffprobe, the FFmpeg library version `reelctl` was built with, manimgx, Smelter, ONNX Runtime, the whisper model, and the TransNet and RVM weights |
| `assets.import` | `{path, kind, name?, copy?}` | `{asset}` |
| `assets.list` | `{kind?}` | `[asset]` |
| `subjects.create` | `{kind, name}` | `{subject}` |
| `subjects.list` | `{kind?}` | `[subject with head]` |
| `subjects.branch` | `{checkpoint, name}` | `{subject}` |
| `jobs.submit` | `{subject, command, args, inputs, after?, priority?, force?}` | `{job, status, cached}` |
| `jobs.get` | `{job}` | the job, with its last events |
| `jobs.list` | `{subject?, status?, limit?}` | `[job]` |
| `jobs.cancel` | `{job}` | `{status}` |
| `jobs.retry` | `{job}` | `{job}` (a new job with the same request) |
| `events.subscribe` | `{job?, since_seq?}`: with `job`, its stored events after `since_seq` first | then notifications: `job.status`, `job.progress`, `job.event`, `checkpoint.created` |
| `checkpoints.list` | `{subject}` | `[checkpoint]` |
| `checkpoints.get` | `{checkpoint}` | the checkpoint, with each output's `path` in `objects/` |
| `checkpoints.revert` | `{subject, checkpoint}` | `{checkpoint}` |
| `checkpoints.diff` | `{a, b}` | as in section 3 |
| `objects.path` | `{hash}` | `{path}` (read-only), for the app to open with AVPlayer or an image view |
| `store.stats` | none | bytes by subject and kind |
| `store.gc` | `{dry_run}` | as in section 3 |
| `understand.plan` | `{subject, input, dna, model?, max_cost_usd?, ...}` | `{plan, cost_upper_bound_usd, frames, images, expires_at}` |
| `understand.run` | `{plan, approved_max_usd, api_key}` | `{job}` |
| `pipelines.make_reels` | as the `make-reels` flags (`input`, `outliers`, `target_url`, ...), plus `dna?`, `understanding?`, `subject?` | `{stage, subject, ...}`: `decompile` with `jobs`, `plan` with `plan`, or `reels` with `reels`, `follow`, `notes` (see `docs/cli.md`) |
| `jobs.argv` | `{job}` | `{argv}`: the job as `reelctl` arguments, for "Copy as CLI command"; never a key |
| `shutdown` | none | `{}`; running jobs are cancelled cleanly |

Errors are JSON-RPC errors with `code` and `message`, and `data.exit_code` when a child's exit code is the cause (2 to 4, as on the command line).

- **The paid call takes two requests.** `understand.plan` runs `understand --dry-run` as a job and stores the bound, reported as the printed bound plus $0.0001. `understand.run` is refused unless it names an unused, unexpired plan (15 minutes) and `approved_max_usd` equals that bound ([ADR-031](../decisions/ADR-031-a-live-understand-call-needs-the-plans-bound-approved-exactly.md)), which is then passed as `--max-cost-usd`. `jobs.submit` refuses `understand`, and `jobs.retry` refuses a live one. The key comes in that request only. It is kept in memory for that one child's environment and never written to the catalogue, an event, a log or an error. The client (the Mac app) asks the user each time. A request without `approved_max_usd` cannot make a live call.
- **`pipelines.make_reels`** queues the M0 chain as jobs with `after` links, on one `make_reels` subject plus one `reel` subject per clip:
  1. decompile
  2. understand, through the plan and run pair: the pipeline stops at the plan and returns it, and the client continues with `understand.run`
  3. then, per clip: from-clip, cut-silence, auto-punch-in, reframe, bridge, captions, render and diff

  Every step is then a checkpoint that can be reverted, and a reel can be re-rendered from any earlier step. The step list and the clip choice are `crates/studio/src/pipeline.rs`, which `reelctl make-reels` uses too, so the command line and the service build the same steps. As built, it is three calls with the same params, adding `dna` and then `understanding` as each stage finishes (`docs/cli.md`, "reelctl serve"); each step reads the previous one's output by `{"from": job}`, which makes the chain. The presenter crop runs only when `weights` names a file and `ORT_DYLIB_PATH` is set; otherwise the crop is centred, with a note. Unlike the command line, the service does not try the presenter crop and fall back after a failure: it decides before queueing.

### 6. The command catalogue

`jobs.submit` takes only known commands, with typed arguments, never a raw argv. For each command, the catalogue gives:

- its arguments, with types and defaults, matching `docs/cli.md`;
- its input roles (for example `input: video`, `dna: edit_dna`) and output roles (for example `out: edit_dna`, `filmstrip_dir: tree`);
- its resource class;
- the environment names it may get.

This is the one place argv is built, and it is tested in Rust. `make-reels` is moved onto it.

### 7. `--progress json` (a global `reelctl` flag)

With `--progress json`, `reelctl` writes NDJSON to **stderr**, one object per line, schema version 1. stdout is unchanged, so scripts keep working.

```
{"v":1,"event":"stage","name":"decode"}
{"v":1,"event":"progress","stage":"decode","done":120,"total":1800}
{"v":1,"event":"warning","message":"..."}
{"v":1,"event":"result","command":"decompile","fields":{"signal_ms":"310","cuts":"4"}}
{"v":1,"event":"error","code":2,"message":"..."}
```

- **Where it comes from:** the decode loops (decompile, diff, look, qa), the whisper passes, the TransNet windows, render (from `ffmpeg -progress pipe:`), the compositor and reframe-measure frames, the manimgx stages, and each make-reels step.
- **Rate:** at most 10 progress events per second per stage.
- **`result`:** its `fields` are the same `key=value` pairs as the stdout line.
- **Without the flag,** stderr is exactly as today.
- **What is not an event:** clap usage errors (an unknown flag, a missing argument) are printed by clap before the flag is read. A child that writes to an inherited stderr, such as the whisper worker, could add a plain line; whisper.cpp prints nothing. The service treats a line that is not JSON as a `log` event.

### 8. Clean cancel

On SIGTERM, SIGINT or SIGHUP (through the `ctrlc` crate, whose API is safe; it is the one new dependency of this task), `reelctl` stops its own children (registered with `progress::track`) (ffmpeg, the whisper worker, manimgx and Smelter), removes its temporary outputs (render's and the compositor's `.tmp` files, and the caption sequence), writes no further result file, and exits 130. A second signal, or 10 s without stopping, forces the exit after the same clean-up ([ADR-030](../decisions/ADR-030-cancel-with-a-flag-a-child-registry-and-a-forced-exit.md)).

### 9. Security

- **No network port.** The only listener is the Unix socket: 0600, inside a 0700 directory. `jobs.submit` refuses `render` with the Smelter backend, whose server listens on every interface. Each connection's peer uid is checked (`SO_PEERCRED` on Linux, `getpeereid` on macOS), and any other user is refused.
- **Paths.** Paths in requests must be absolute. Imports are opened read-only, and the same "local regular file, no URL or protocol" rule as the CLI applies.
- **The key.** It is held in memory only, for one `understand` child, and its value is never logged. A test sets a key and checks that it is not in the database file, the events or any response.
- **The paid call** needs `understand.run` with a plan id and an approved amount (section 5).
- **Children** run with `env_clear` and a minimal environment ([ADR-027](../decisions/ADR-027-make-reels-runs-each-step-as-a-child-reelctl-with-a-minimal-environment.md)).

## Boundaries

- **Always:**
  - Append-only history: revert and branch add, never delete.
  - Outputs are immutable once stored.
  - One ONNX model per process.
  - Every live model call follows a plan and an approved amount.
  - Tests use synthetic fixtures, recorded replies and no network.
- **Ask first:**
  - changing the schema after version 1 is released;
  - collecting garbage without a dry run;
  - any listener other than the Unix socket.
- **Never:**
  - store or log a key;
  - accept a raw argv;
  - write inside `Reels/` or into an imported asset's path;
  - delete a checkpoint.

## Testing Strategy

- **Unit tests in `crates/studio`:**
  - **The store:** the same bytes give one object; tree objects; read-only files; a hash mismatch on read is an error.
  - **The catalogue:** migration from empty, refusing a newer version, and transactions under many producer threads.
  - **Checkpoints:** revert appends; a revert of a revert; branch; the head moves; cache hits and `force`.
  - **The queue,** with a fake runner (a closure in place of the child):
    - class limits are never exceeded;
    - priority, then age;
    - `after` chains, and a failure propagating along them;
    - cancel while queued, blocked and running;
    - restart turns running jobs into interrupted ones.
  - **The command catalogue:** argv and environment for each command, and an unknown command or argument refused.
  - **The protocol:** request and response round trips, and the error codes.
- **Integration tests in `crates/reelctl/tests/serve_fixture.rs`.** They start `reelctl serve` on a temporary workspace, speak JSON-RPC over the socket, and use `lecture70`, `draft12` and the recorded understand reply:
  - **The edit flow:** import, decompile, from-clip, cut-silence, render. Then revert the project to the from-clip checkpoint and render again.
  - **Cancel:** cancel a render and check that nothing is left in `jobs/`.
  - **Restart:** kill `serve` while a job runs and restart it; the job is `interrupted`.
  - **The paid call:** `understand.run` without a plan, or above the bound, is refused.
  - **`pipelines.make_reels`:** it ends with two reels whose steps are all checkpoints.
  - **The key:** set it and grep the database file for it.
  - **The peer check:** a client as another uid is refused, where the test can run as root and switch user; otherwise it is skipped with a note.
  - **`--progress json`:** every stderr line parses as an event, and the `result` event equals the stdout line.
- AGENTS.md's environment stripping applies to every test that spawns `reelctl`.

## Success Criteria

1. Over the socket, the edit flow above makes one checkpoint per job. A revert renders the earlier cut, and no object or checkpoint is deleted.
2. With `heavy` set to 1, two renders never run at the same time. With `light` set to 4, four edits do.
3. A killed `serve` loses no job: running jobs become `interrupted`, and queued ones run after the restart.
4. A cache hit makes a checkpoint without a child process, and `force` runs one.
5. The key is in no file of the workspace, and no live call is possible without an approved plan.
6. `reelctl` without `--progress` behaves byte for byte as before: stdout, stderr and exit codes.

## Resolved Questions

1. **Default workspace:** `~/Movies/ReelStudio` on macOS and `~/ReelStudio` on Linux, when `--workspace` is not given (the user's choice).
2. **Stopping:** `serve` runs until it is told to stop. The app sends `shutdown` when it quits with no job queued (the user's choice). There is no idle timer.
3. **Event retention:** every progress event is kept (the recommended answer, taken as the default).
4. **Workspaces:** one per `serve` (the recommended answer, taken as the default).
