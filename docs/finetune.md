# Fine-tuning plan (P3), and what is built for it

Training is **not built and not scheduled**. planer.md's P3 asks for a compact Qwen3-VL LoRA model; the user said "skip fine tuning for now". What exists is the half that runs on a 16 GB Apple M3 with no GPU: a dataset collector (`reelctl dataset build`) and an eval harness (`reelctl eval`), so that when training happens there are records to train on and a way to tell whether it helped. See [the spec](specs/p3-dataset-eval.md) and [ADR-024](decisions/ADR-024-build-the-p3-dataset-and-eval-without-training.md).

## The plan, in order

1. **Collect.** Decompile reels with a whisper model (`reelctl decompile --whisper-model ...`), and run `reelctl understand` on long videos with one or more teacher models. Each `understand` run costs money and sends a transcript off the machine, so it needs a yes each time ([docs/understanding.md](understanding.md)).
2. **Build.** `reelctl dataset build --dna <dir> --understanding <runs> --out <dataset>` writes `train.jsonl`, `val.jsonl` and `manifest.json`. Read `manifest.json` first: it lists what was skipped and why, and warns when a split is empty.
3. **Baseline.** Give the **base** model each `val` record's `instruction` and `input` (nothing else), write its replies as an answers file, and run `reelctl eval --truth <dataset> --answers <file>`. Keep the JSON report. Do this before any training; without it no later number means anything.
4. **Train** a LoRA on the `trainable` records of `train.jsonl` (today only `clip_picks`). Not built here.
5. **Compare.** Run step 3 again with the fine-tuned model and diff the two reports. The fields and their order are the same for every model.
6. **Decide.** There is no pass mark in the code. A reasonable gate is: `rule_pass_rate` and `valid_rate` do not fall, agreement with the teacher rises, and the `val` split is large enough that the difference is not a few records (the reports carry counts, not confidence intervals).

## Hardware

**None of these numbers was measured or checked against Qwen's documentation; they are rough estimates from general knowledge of LoRA training, to be verified before buying or renting anything.**

| Model size (bf16 weights) | Method | Rough GPU memory | Notes |
|---|---|---|---|
| about 2B to 4B | LoRA, 4-bit base (QLoRA) | 8 to 16 GB plus activations | A 24 GB card is comfortable |
| about 7B to 8B | QLoRA | 16 to 24 GB | |
| about 7B to 8B | LoRA in bf16 | 40 GB or more | |

- A `clip_picks` record is a whole transcript: the 28-minute lecture in the clipper's evidence has 535 segments, tens of thousands of tokens. Activation memory grows with that length, and long sequences are what make training hard to fit. Chunking a transcript into windows would change the task (a pick near a window edge) and is not done.
- The 16 GB M3 is the development machine, not a training machine. Inference of a small quantised model, and scoring with `reelctl eval`, are fine there. Training on it (for example with Apple's MLX) is not ruled out for a 2B model on short sequences, but it is not tested and the long transcripts probably do not fit.
- Cloud GPU rental is the realistic route. Check the price and Qwen3-VL's current model sizes then; both change.

## The data format the collector writes

`<out>/train.jsonl` and `<out>/val.jsonl`: one JSON object per line, UTF-8, keys in the order below, records sorted by task, reel id, record id. `<out>/manifest.json` describes the run. The JSON Schemas are in [crates/dataset/schema/](../crates/dataset/schema/) (`record.schema.json`, `manifest.schema.json`; regenerate with `cargo run -p dataset --example dataset_schema`).

| Field | Meaning |
|---|---|
| `dataset_schema_version` | 1. A reader must refuse any other value |
| `id` | `<task>-<reel_id>-<12 hex>`; the hex is from the SHA-256 of the inputs, so the id changes when an input file does |
| `task` | `clip_picks`, `edit_events` or `pacing` |
| `reel_id` | The `edit_dna.json` file name without `.edit_dna.json` |
| `split` | `train` or `val`, from the reel id and the salt only, so every record of a reel is in one split |
| `trainable` | `true` only for `clip_picks` |
| `instruction` | Fixed text per task, the same in every record of the task |
| `input` | What the model may see |
| `target` | The answer |
| `context` | For scoring only: **do not show it to the model** (for `clip_picks`, the measured pauses) |
| `provenance` | `tool`, `tool_version`, `reel_id`, `edit_dna_schema_version`, `edit_dna_sha256`, `cut_detector`, `asr_backend`, and for `clip_picks` an `understanding` object with the run's `schema_version`, `sha256`, `teacher_model` and `teacher_provider` |

| Task | `input` | `target` |
|---|---|---|
| `clip_picks` | `{duration_ms, segments: [{id, start_ms, end_ms, text, ends_sentence}]}` | `{picks: [{hook_segment, payoff_segment, cutoff_segment}]}`, best first |
| `edit_events` | `{duration_ms, fps, frame_size}` | `{cuts: [{time_ms}], visual_events: [{time_ms, kind, scale, area}] or null, zoom_curves: [{event_index, bezier}] or null}` |
| `pacing` | `{duration_ms, fps, frame_size}` | `{cut_count, mean_shot_ms, longest_shot_ms, dead_air_count, words_per_minute or null, integrated_lufs, payoff_has_cue, payoff_cue_ms or null}` |

`null` means not measured (an older `edit_dna.json` schema, or burst words, whose rate is not a word rate); `[]` means measured and none. No record holds an absolute path, a directory, a media byte, or any text the teacher wrote (title, reasons, topic, beats, labels).

**A chat mapping** for a trainer (not enforced by anything): system = `instruction`; user = `input` as compact JSON; assistant = `target` as compact JSON. Train on the assistant part only. Never put `context` in the prompt.

**Answers for scoring.** For a dataset: a JSONL file, one line per record: `{"id": "<record id>", "answer": <a value shaped like that task's target>}`. A `clip_picks` answer may also give `start_segment` and `end_segment`. For an `understanding.json`: the reply file `reelctl understand --replay` reads (or a bare reply) plus `--dna` of the same video. Details and the metric definitions are in the [spec](specs/p3-dataset-eval.md) and [cli.md](cli.md#reelctl-eval).

## What this data can and cannot teach

- **Only `clip_picks` can train a model today.** `edit_events` and `pacing` are for scoring a model that was given the video some other way (a hosted video model, or a later frame sampler). The `timeline_view` PNGs `decompile` writes are drawn at the cuts it found, with the cut marked in the picture and its frame number in the file name, so pairing them with cut times would teach nothing but reading the label. The collector does not attach them. A vision training set needs a frame sampler that does not depend on the labels (uniform times), which does not exist yet.
- **The labels are `reelctl`'s measurements, not a human's.** A model scored on them is scored on agreement with the scene-score or TransNetV2 cut detector, the motion detector and whisper, errors included. The detector is named in `provenance`.
- **The clip picks are one teacher's choices.** They are the picks of a hosted model that met every clip rule (open on a hook, end on the cliffhanger, 30 to 60 s once pauses are cut). Rule-compliant is not the same as good or engaging. The model-independent numbers are `valid_rate` and `rule_pass_rate`; the hook, payoff, cutoff and interval numbers are agreement with the named teacher.
- **There is no retention label and no tool-call log.** planer.md's `(edit_dna, tool_calls, retention_score)` triple cannot be collected: `reelctl` has no engagement data and no agent writes tool calls. Nothing here predicts what makes a reel perform.
- **The data is small.** The corpus a user has today is a handful of reels and runs. Splits are by reel, so `val` can be a few records and a difference between two reports can be noise.
- **Near-duplicate reels can leak** across the split if they are given different ids (a re-upload, a re-encode of the same video). The split cannot see that.

## Licence, consent and privacy cautions

This is not legal advice. Treat all of it as open questions to settle before training or sharing anything.

- **Reels are other people's work.** Downloading Instagram or TikTok Reels and training a model on them may breach the platform's terms and the creator's copyright. The decompiled `edit_dna.json` (timings, loudness, colour histograms) is a derived measurement, but the transcripts are verbatim speech.
- **Transcripts are personal data.** `clip_picks` records contain whisper's verbatim text of what a person said, which can include names, opinions and anything else. Depending on where the creator lives, that may fall under data-protection law (for example the GDPR in the EU, or India's DPDP Act), which can require a lawful basis and allow deletion requests. `reel_id` and `edit_dna_sha256` in every record are there so that a creator's records can be found and removed.
- **Teacher models have terms too.** The picks come from a hosted model reached through OpenRouter. Some providers restrict using their outputs to train other models; check the terms of each provider that served a run (`provenance.understanding.teacher_provider`) before using its picks as training targets. This was not checked.
- **A fine-tuned model inherits the question.** Weights trained on scraped data may carry the same obligations as the data. Do not publish the dataset or the weights without settling the above.
- **Keep it out of git.** Datasets built from real reels stay with `Reels/` (git-ignored); only synthetic fixtures may be committed. `reelctl dataset build` writes to the directory you name; name one outside the repository or under `Reels/`.
- **Sending data off the machine.** `reelctl understand` sends frames and a transcript to a hosted model with data collection denied in the request, and each live call needs the user's yes. `dataset build` and `eval` never use the network.

## Not done, on purpose

- No training code, no training run, no model download, no GPU use.
- No filmstrip frames in records (see above).
- No retention or engagement label.
- No live model call in any test or command of this slice.
