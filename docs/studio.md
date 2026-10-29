# The studio service: API reference

`reelctl serve` without the app, for scripts or a shared Mac. The design is in [docs/specs/studio-service.md](specs/studio-service.md), and the flags, exit codes and rules are in [docs/cli.md](cli.md#reelctl-serve).

## Connect

```bash
reelctl serve --workspace ~/ReelStudio
```

It prints `serve: workspace=… socket=… pid=…`. The socket is `<workspace>/.reelstudio/reelctl.sock`; only the same user can connect. Each line sent is one JSON-RPC 2.0 request, and each line back is a reply or a notification:

```json
{"jsonrpc":"2.0","id":1,"method":"workspace.info","params":{}}
```

A quick client in Python:

```python
import json, socket
s = socket.socket(socket.AF_UNIX); s.connect("/Users/me/ReelStudio/.reelstudio/reelctl.sock")
f = s.makefile("rw")
def call(method, **params):
    f.write(json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}) + "\n"); f.flush()
    while True:
        message = json.loads(f.readline())
        if message.get("id") == 1: return message
```

## A session

1. `assets.import {path, kind: "video"}` returns `{id, hash, ...}`. The file is imported by reference, and its size and hash are checked again by every job that reads it.
2. `subjects.create {kind: "project", name}` returns `{id}`.
3. `jobs.submit {subject, command: "timeline.init", inputs: {input: {asset: <id>}}}` returns `{job, status, cached}`.
4. `jobs.submit {subject, command: "timeline.cut-silence", args: {min_gap_ms: 180}, inputs: {project: {from: <init job>, role: "out"}, dna: {asset: <dna id>}}}` is `blocked` until the init succeeds.
5. `events.subscribe {}` (on its own connection) sends `job.status`, `job.progress`, `job.event` and `checkpoint.created`.
6. `checkpoints.list {subject}` and `checkpoints.get {checkpoint}`: the second gives each output's read-only `path`.
7. `checkpoints.revert {subject, checkpoint}` appends a copy of an earlier state. `subjects.branch {checkpoint, name}` starts a new subject from it.

## Methods

| Method | Params | Result |
|---|---|---|
| `workspace.info` | none | `{path, version, schema_version, limits, pid}` |
| `workspace.set_limits` | `{heavy?, light?, network?}` | the limits |
| `doctor.check` | none | each tool and file: `{found, path, version}` |
| `assets.import` | `{path, kind, name?, copy?}` | the asset |
| `assets.list` | `{kind?}` | assets |
| `subjects.create` / `subjects.list` / `subjects.branch` | `{kind, name}` / `{kind?}` / `{checkpoint, name}` | subject(s) |
| `jobs.submit` | `{subject, command, args?, inputs, after?, priority?, force?}` | `{job, status, cached}` |
| `jobs.get` / `jobs.list` | `{job}` / `{subject?, status?, limit?}` | the job, with its last 50 events and checkpoint / jobs |
| `jobs.cancel` / `jobs.retry` / `jobs.argv` | `{job}` | `{status}` / `{job}` / `{argv}` |
| `events.subscribe` | `{job?, since_seq?}` | `{subscribed}`, then notifications |
| `checkpoints.list` / `checkpoints.get` / `checkpoints.revert` / `checkpoints.diff` | `{subject}` / `{checkpoint}` / `{subject, checkpoint}` / `{a, b}` | |
| `objects.path` | `{hash}` | `{path}` |
| `store.stats` / `store.gc` | none / `{dry_run}` (default true) | sizes / what is (or would be) removed |
| `understand.plan` | `{subject, input, dna, model?, every_s?, ...}` | `{plan, cost_upper_bound_usd, frames, images, expires_at}` |
| `understand.run` | `{plan, approved_max_usd, api_key}` | `{job}` |
| `pipelines.make_reels` | see below | `{stage, subject, ...}` |
| `shutdown` | none | `{}` |

**Commands for `jobs.submit`:** the names and their typed arguments are `crates/studio/src/commands.rs`. Arguments are the flag names with `_`, for example `min_gap_ms` for `--min-gap-ms`. A raw argv is never accepted. `understand` goes through `understand.plan` and `understand.run` only, and `render` with the Smelter backend is refused, because it would listen on the network.

**Inputs** name a stored hash, `{"from": <job>, "role": "<output>"}` or `{"asset": <id>}`. Output roles are `out`, plus `views` for decompile and qa, and `report` for diff and eval.

## The paid call

1. `understand.plan` runs a dry run, with no key and no spend, and returns the bound.
2. Show it to the user. On their yes, send `understand.run` with `approved_max_usd` equal to that bound and the key.

A plan is used once and expires after 15 minutes. The key is kept for that one child process only ([ADR-031](decisions/ADR-031-a-live-understand-call-needs-the-plans-bound-approved-exactly.md)).

## Make reels as jobs

Call `pipelines.make_reels` with `{input, outliers: [...], target_url, ...}` three times. Each time, add what the previous call produced:

1. **No `dna`:** it returns `{stage: "decompile", jobs: [id]}`. Wait for that job.
2. **Add `dna: {from: <id>, role: "out"}`:** the DNA needs whisper words, so decompile with `whisper_model`. It returns `{stage: "plan", plan}`. Approve the plan with `understand.run`.
3. **Add `understanding: {from: <run job>, role: "out"}`:** it returns `{stage: "reels", reels: [{clip, subject, jobs: [{step, job}]}], follow, notes}`.

Every step of every reel is a job, and each job is a checkpoint on that reel's subject.
