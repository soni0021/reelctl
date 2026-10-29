# Spec: P3 offline half, a dataset collector and an eval harness (`dataset`)

Status: written 2026-10-29 under the user's "p1-p3" (after "P3 with dataset collector and eval harness" and "skip fine tuning for now"); the user has not reviewed it line by line. Branch: `feature/p3-dataset-eval`, cut from `main` at b312ffd. Implements the part of P3 in [planer.md](../../planer.md) section 7 that needs no GPU and no training: turning decompiled reels into instruction-tuning records, and scoring a model's answers against them. The LoRA training itself is **not built** and the user has deferred it ([finetune.md](../finetune.md)). Module `dataset-eval` in the [P0 capability map](p0-capability-map.md).

## Objective

planer.md's P3 fine-tunes a compact Qwen3-VL LoRA on collected `(edit_dna, tool_calls, retention_score)` data. Two things can be built and verified on a 16 GB Apple M3 with no GPU:

1. **A collector** (`reelctl dataset build`) that reads `edit_dna.json` files and `understanding.json` runs and writes JSONL records whose targets are measured facts.
2. **An eval harness** (`reelctl eval`) that scores a model's answers against those records, or against one `understanding.json`, so a base model and a later fine-tune are compared by the same numbers.

Both are offline and deterministic. Nothing calls a model or the network.

## What the data can and cannot claim

These limits decide the record format; they are stated here so nothing downstream over-reads the data.

1. **Cut-anchored filmstrips leak the answer.** `reelctl decompile` writes one `timeline_view` PNG per detected cut, named `..._cut_NN_f<frame>.png`, with a red cut line drawn into the pixels. A record that used them as input and cut times as the target would be circular, through the file name and through the image. **The collector therefore does not attach filmstrip frames.** The user asked for optional filmstrip paths; this is the one place the build is less than the request, and why. A vision training set needs a frame sampler that does not depend on the labels (uniform times); `decompile` does not have one, and it is not built here.
2. **Only one task can train a model today.** `clip_picks` is text in, text out: numbered whisper segments in, segment IDs out. `edit_events` and `pacing` have no non-leaking input yet, so they are **eval records** (`"trainable": false`): ground truth for scoring a model that was given the video some other way (a hosted video model, or a later frame sampler).
3. **No retention score and no tool calls exist.** planer.md's `(edit_dna, tool_calls, retention_score)` triple cannot be collected: `reelctl` has no retention data and no agent logs tool calls. The records carry measured facts and rule-checked picks, never an engagement label.
4. **A measured label is a detector's output, not a human's.** Cuts come from the scene score or TransNetV2, events from the motion detector, words from whisper. A model scored against them is scored on agreement with `reelctl`; the detector's errors are in the truth. Each record names its detector in `provenance`.
5. **Clip picks are one teacher model's choices.** `understanding.json` picks come from a hosted model and were checked by deterministic rules. Only picks that meet every rule (`flags` empty) become targets, and only their segment IDs: `title`, `why`, `open_loop`, `visual_suggestion`, beats, `visual_events`, and `vlm_label` are model prose or model labels and are **never copied**. The teacher's `model.id` and provider go into `provenance`. The model-independent headline metrics are validity and rule-pass rate; agreement with the teacher's IDs is reported as agreement with that teacher, not as accuracy.
6. **"Not measured" is not "none".** `edit_dna.json` versions 1 and 2 have no `visual_events` (the field reads as `[]`), and versions before 4 have `zoom_curves.status: unimplemented`. A target field is `null` when the file predates it or the curve was not measured; it is `[]` only when it was measured and empty. `words_per_minute` is emitted only when `asr_backend` is `whisper_cpp`; under `silence_bursts` it is a burst rate and is `null`.
7. **Real reels are sensitive.** Records built from real reels hold verbatim whisper transcripts. They are derived data of someone else's content, stay out of git like `Reels/`, and are covered by the licence cautions in [finetune.md](../finetune.md). The repository's tests use synthetic text only.

## Tech stack

Rust workspace as today. A new pure crate `crates/dataset` depends on `edit-dna`, `understand` (types, `build_clips`, constants), `diff` (`reel_metrics`), `serde`, `serde_json`, `schemars`, `thiserror`. **No new dependency:** SHA-256 is written by hand and tested against the NIST vectors (`sha2` is in the local registry cache but not in `Cargo.lock`, and adding it could need a fetch and would churn the lock file other branches edit). The `reelctl` subcommands read and write the files; the crate takes bytes and returns bytes, so `cargo test -p dataset` needs no ffmpeg.

## Commands

```
reelctl dataset build --dna <dir|file>... [--understanding <file>...] --out <dir>
                      [--val-percent 10] [--salt reelctl-dataset-v1]
reelctl eval --truth <understanding.json | dataset dir | .jsonl> --answers <file>
             [--dna <edit_dna.json>] [--split val|train|all]
             [--cut-tolerance-ms 100] [--event-tolerance-ms 250] [--format json|text]
cargo test -p dataset
cargo run -p dataset --example dataset_schema
```

Exit codes: 0 success; 1 runtime error (unreadable or unparseable input, unknown schema version, an empty split to score); 2 argument error (clap's, plus the cases named below).

## Contract: `dataset build`

### Inputs

- `--dna`: a directory (every regular file ending `.edit_dna.json` directly inside it, not recursive) or files. The **reel id** is the file name without `.edit_dna.json` (else without `.json`). An id must be 1 to 96 characters of `A-Za-z0-9._-` and not start with `.`; otherwise that file is **skipped** with reason `bad_reel_id`. Two files with the same id are an error.
- `--understanding`: `understanding.json` files. A run belongs to the reel whose id equals the file stem of `source.path` (both `/` and `\` separate); `source.path` itself is never copied.
- Every file is parsed from the raw JSON first: `schema_version` must be present and in `1..=4` for `edit_dna.json` (`edit_dna::SCHEMA_VERSION`) and equal to 1 for `understanding.json` (`understand::SCHEMA_VERSION`). Anything else (missing, 0, above, or unparseable) is an **error**: nothing is written.

### Output

`<out>/train.jsonl`, `<out>/val.jsonl` (one record per line, possibly empty) and `<out>/manifest.json`. Existing files of those names are replaced; other files in `<out>` are left alone. Output is deterministic: input order and file-system order do not matter (records are sorted by `(task, reel_id, id)`), and the same inputs give byte-identical files.

### Record (`dataset_schema_version: 1`)

```json
{"dataset_schema_version": 1, "id": "clip_picks-lecture-3f9a1c0b27de", "task": "clip_picks",
 "reel_id": "lecture", "split": "train", "trainable": true,
 "instruction": "...", "input": {...}, "target": {...}, "context": {...}, "provenance": {...}}
```

`id` is `<task>-<reel_id>-<first 12 hex of input_hash>`; `input_hash` is the SHA-256 of the dataset schema version, the task, the reel id and the SHA-256 of each input file used (the `edit_dna.json` bytes and, for `clip_picks`, the `understanding.json` bytes). `instruction` is a fixed text per task (versioned with the schema). `input` is what a model may see; `target` is the answer; `context` is for scoring only and a trainer must not show it. No field holds an absolute path, a directory, or media bytes.

| Task | `input` | `target` | `context` | `trainable` |
|---|---|---|---|---|
| `clip_picks` | `duration_ms`, `segments` (`id`, `start_ms`, `end_ms`, `text`, `ends_sentence`, from `understanding.json`) | `picks`: `[{hook_segment, payoff_segment, cutoff_segment}]` in the teacher's rank order, only picks with empty `flags` | `silence_gaps` of the reel's `edit_dna.json` (the clip rules need the measured pauses) | yes |
| `edit_events` | `duration_ms`, `fps`, `frame_size` | `cuts`: `[{time_ms}]`; `visual_events`: `[{time_ms, kind, scale, area}]` or `null`; `zoom_curves`: `[{event_index, bezier}]` or `null` | `{}` | no |
| `pacing` | `duration_ms`, `fps`, `frame_size` | `cut_count`, `mean_shot_ms`, `longest_shot_ms`, `dead_air_count`, `words_per_minute` (or `null`), `integrated_lufs`, `payoff_has_cue`, `payoff_cue_ms` (or `null`) | `{}` | no |

`pacing` numbers are `diff::reel_metrics`, so they match `reelctl diff`. `visual_events` is `null` before schema 3; `zoom_curves` is `null` unless the file is version 4 or later with `status: measured`.

`provenance`: `tool` (`reelctl-dataset`), `tool_version`, `reel_id`, `edit_dna_schema_version`, `edit_dna_sha256`, `cut_detector`, `asr_backend`, and for `clip_picks` an `understanding` object with `schema_version`, `sha256`, `teacher_model`, `teacher_provider`.

### Skips (recorded in the manifest, printed on stderr)

A skip names the reel id (or the input file name, never its directory), the task, a reason code, and a detail. Reasons: `bad_reel_id`; `zero_duration` (an `edit_dna.json` with `duration_ms` 0); `non_finite_value` (a number the record needs is NaN or infinite); `no_matching_edit_dna` (an `understanding.json` whose reel id has no `--dna` file); `duration_mismatch` (`source.duration_ms` differs from the DNA's); `no_rule_compliant_clips` (every clip has flags, or there are none); `inconsistent_understanding` (a kept clip names a segment ID that is not in `segments`); `duplicate_understanding` (the same bytes given twice). The number of flagged clips left out of targets is counted in the manifest. A reel whose `edit_dna.json` is fine and whose understanding is skipped still gets its `edit_events` and `pacing` records.

### Splits

`split_for(reel_id, salt, val_percent)`: the first 8 bytes (big-endian `u64`) of SHA-256(`salt`, byte 0, `reel_id`), modulo 100, is below `val_percent` means `val`, else `train`. `val_percent` is an integer 0 to 50 (else exit 2). The split depends on the reel id and the salt only: all records of a reel, of every task and every understanding run, land in the same split; adding or removing other reels never moves a reel. A split with no records is written as an empty file and reported as a manifest warning and on stderr; with few reels it is likely.

### Manifest

`manifest.json`: `dataset_schema_version`, `tool_version`, `salt`, `val_percent`, `reels` (`reel_id`, `split`, `edit_dna_sha256`, sorted), `files` (`train.jsonl` and `val.jsonl`: `records`, `sha256`), `records` (per task and split counts), `skipped`, `flagged_clips_left_out`, `warnings`.

## Contract: `eval`

**Truth kinds.** A `.json` file is an `understanding.json` (`--dna` of the same reel is required, exit 2 without it, because the clip rules need the measured pauses and that file has none; its `duration_ms` must equal `source.duration_ms`, else exit 1). A directory is a dataset written by `dataset build` (`train.jsonl` and `val.jsonl` per `--split`, default `val`); a `.jsonl` file is read as one split. Only one answer format exists per truth kind:

- **Understanding truth:** the answers file is a `VlmReply` JSON (what `reelctl understand --replay` reads) or a bare `RawReply`. Only `reel_candidates` is read: the five segment IDs of each pick, in order.
- **Dataset truth:** a JSONL file, one line per record: `{"id": "<record id>", "answer": <a value shaped like that task's target>}`. For `clip_picks` an answer pick may also give `start_segment` and `end_segment`; absent, they default to the hook and the cutoff (the rules do not use them to decide the clip). A record with no line is **unanswered**; a line that does not parse is **unparseable** (listed in `problems`, at most 20, then counted); an `id` twice is an error (exit 1); an `id` not in the truth is counted as `extra_answers` and ignored.

An empty split to score is an error (exit 1, "split val has no records"), not a score.

### Scoring `clip_picks`

For each record (or the one `understanding.json`), the truth picks are the segment IDs of its clips with empty `flags` (understanding truth) or its `target.picks` (dataset truth). Both are rebuilt with `understand::build_clips` on the truth segments, the measured pauses and the duration, so truth and answer go through the same rules. A truth pick that does not rebuild into a clip with no flags (a different `--dna` than the run used) is counted in `truth_unreproducible` and left out of the agreement metrics.

Model-independent (about the answer alone), over all answer picks of all records: `picks` (count), `valid_rate` (picks that become a clip, i.e. not rejected, over picks), `rule_pass_rate` (picks that become a clip with empty `flags`, over picks). A rate with no picks is `null`.

Agreement with the teacher run(s) in the truth (`teacher` lists the model ids), micro-averaged over records:

- `hook`: precision, recall, F1 of the set of hook segment IDs (answer picks' distinct hooks against truth distinct hooks, exact ID match).
- `payoff_given_hook` and `cutoff_given_hook`: of the hooks both have, the share whose answer pick (the first pick in the answer with that hook) has the same payoff, and the same cutoff, segment ID.
- `interval`: precision, recall, F1 with a clip's `[start_ms, end_ms)` as the interval, matched at IoU at least 0.5 (inclusive). Answer intervals are the clips that were built (rejected picks have none).

### Scoring `edit_events` (dataset truth)

Tolerances are parameters: `--cut-tolerance-ms` (default 100) and `--event-tolerance-ms` (default 250; a zoom move takes up to 12 frames).

- `cuts`: precision, recall, F1 by one-to-one matching of times within the cut tolerance, plus `mean_abs_error_ms` and `median_abs_error_ms` over matched pairs.
- `visual_events`: the same, matching only events of the same `kind` within the event tolerance; plus `scale_mean_abs_error` over matched pairs where both have a scale. A record whose truth `visual_events` is `null` is not scored for them (`visual_events_unscored` counts such records); a truth `[]` against an answer with events scores them all as false positives.
- `zoom_curves`: `bezier_mean_abs_error`, the mean absolute difference of `y1` and `y2` over matched events where both sides have a curve with a `bezier` (an answer curve is found by its `event_index` into the answer's `visual_events`); `pairs` is the count.

### Scoring `pacing` (dataset truth)

Per field, over records where the truth and the answer both have a value: `mean_abs_error` for `mean_shot_ms`, `longest_shot_ms`, `words_per_minute`, `integrated_lufs`, `cut_count` and `dead_air_count`, and `exact_rate` for the two counts; `payoff_has_cue_accuracy`; each with the count of pairs. Fields the truth has as `null` are not scored; fields the answer leaves out are counted as `missing`.

### Matching, ties, degenerate cases

- **One-to-one matching.** Candidate pairs are every (truth `i`, answer `j`) within the tolerance (inclusive) and, for events, of one kind. Sort by absolute difference, then `i`, then `j`; accept a pair when neither side is used yet. For intervals the sort key is IoU descending, then `i`, then `j`, and a pair needs IoU at least the threshold.
- **Precision, recall, F1** from totals `tp`, `fp`, `fn` (never averaged per record): precision is `tp/(tp+fp)`, recall `tp/(tp+fn)`, F1 `2tp/(2tp+fp+fn)`; each is `null` when its denominator is 0. So empty truth against an empty answer is `null` (nothing to score), not 1.0; an answer with only false positives has recall `null` if the truth is empty and F1 0 when `fp > 0`.
- **IoU** of `[s1,e1)` and `[s2,e2)` is intersection over union; `null` if either interval has `end < start` or the union is 0.
- **Means and medians** of no values are `null`; the median of an even count is the mean of the two middle values.
- **Non-finite numbers are refused.** A NaN or infinite value fed to any metric is an error (`MetricError::NotFinite`), never a score; JSON cannot carry them, so a value that overflows to infinity while being summed is the case this guards. No metric returns NaN, so no report contains a `null` that stands for NaN.
- **Rates** `num/den`: `null` when `den` is 0; `num > den` is an error.

### Report

`eval` prints one JSON object (or `--format text`): `eval_schema_version` (1), `truth_kind`, `split`, `records`, `answered`, `unanswered`, `unparseable`, `extra_answers`, `problems`, `tolerances`, and one entry per task present: `clip_picks`, `edit_events`, `pacing`. The same fields in the same order for every model, so two reports are compared by diffing them. A report is not a ranking: nothing here defines a pass mark.

## Boundaries

- **Always:** targets are measured or rule-checked, never model prose; no absolute path, no media bytes, no `.env` and no key in any output; split by reel; refuse an unknown schema version; say why a record was skipped.
- **Ask first:** attaching frames to records; changing a metric's definition after a baseline report exists; any command that calls a model.
- **Never:** call the network or a model; train; copy anything from `Reels/` into the repository; compute a retention score.

## Testing strategy

- **SHA-256:** the NIST vectors (empty, `abc`, the 56-byte two-block message, one million `a`).
- **Split:** known assignments; same reel, same split across tasks; adding reels never moves a reel; a salt change moves some; 0 percent gives no `val`; the share of `val` over 2,000 ids is within 3 points of the percentage.
- **Metrics:** hand-made cases for every definition above, including empty, all-wrong, ties (two equally close candidates), the inclusive tolerance edge, NaN and infinity refused, `num > den`.
- **Builder:** hand-made `EditDna` (`EditDna::empty`) and `Understanding` values. Schema gating (0, 5, missing, a v2 file gives `null` events, a v3 file gives `null` curves), `silence_bursts` gives `null` words per minute, flagged clips left out, teacher prose never in the output (search the bytes), an absolute `source.path` and the temp-directory root appearing in no output byte, input order shuffled gives byte-identical output, skips carry their reasons, JSONL has one record per line and sorted keys order is stable.
- **Round trip:** an `Understanding` built from `build_clips` on a hand-made transcript, its own picks fed back (hook, payoff, last IDs in rank order) as a `VlmReply`, must reproduce each clip's `start_ms`, `end_ms` and `flags` and score 1.0 on every agreement metric and 1.0 on `rule_pass_rate`; an empty reply scores recall 0 and `rule_pass_rate` `null`; a reply of an invalid ID lowers `valid_rate`.
- **CLI, end to end** (`crates/reelctl/tests/dataset_fixture.rs`): fixtures from `fixtures/generate.sh` decompiled by the real binary, a hand-made `understanding.json` over synthetic "Sentence N is about topic." text, `build` twice with the arguments in different orders gives the same bytes, `eval` of the dataset's own targets scores 1.0 and a perturbed answer file scores lower, unknown schema exits 1, an empty `val` exits 1. The child's environment loses `REELCTL_WHISPER_MODEL`, `REELCTL_TRANSNET_WEIGHTS`, `ORT_DYLIB_PATH` and `OPENROUTER_API_KEY`.
- **Mutation pass:** hand-made mutants (the machine has no `cargo-mutants`, and installing it needs the network) on the metrics, the matching, the split and the hash; the survivors are listed in [status.md](../status.md).

## Open questions, with the recommended answer taken

1. **Attach filmstrip frames?** No (limit 1). Revisit when a label-independent frame sampler exists.
2. **One record per understanding run, or one per reel?** One per run: two teachers' picks are two records with the same input and different targets, both rule-checked. A trainer that wants one target per input should pick one teacher with `provenance.understanding.teacher_model`.
3. **Pass marks?** None. Which numbers a fine-tune must beat is the user's call once a base-model report exists.
