# ADR-024: Build the P3 dataset collector and eval harness without training

## Status
Accepted

## Date
2026-10-29

## Context
planer.md's P3 is a Qwen3-VL LoRA fine-tune on collected `(edit_dna, tool_calls, retention_score)` data. The user chose "P3 with dataset collector and eval harness", then "skip fine tuning for now", then "p1-p3" again. The machine is a 16 GB Apple M3 with no GPU. Three facts shape what can honestly be built:

- `decompile`'s `timeline_view` PNGs are drawn at the cuts it found, with the cut marked in the picture and its frame number in the file name. Used as input with cut times as the target, they give the answer away.
- `reelctl` has no retention data and no tool-call log, so the planned triple does not exist.
- `understanding.json` picks are one hosted model's choices, checked by deterministic rules; most of that file (beats, topic, labels, titles) is model prose.

## Decision
- A new pure crate `dataset` and two commands, `reelctl dataset build` and `reelctl eval`, both offline and deterministic. No training code.
- Records are JSONL with `input`, `target`, scoring-only `context`, and `provenance`. Targets are measured facts or rule-checked segment IDs only; the teacher's prose and labels are never copied, and the teacher's model id is recorded.
- Only `clip_picks` (text in, segment IDs out) is `trainable`. `edit_events` and `pacing` are eval records, and filmstrip frames are not attached (reason above).
- A target field is `null` when the source file predates it or it was not measured, and `[]` only when measured and empty. `words_per_minute` is null unless whisper timed the words.
- The split is by reel, from SHA-256(salt, reel id) alone, so adding reels never moves one. SHA-256 is written by hand and tested against the NIST vectors rather than adding a dependency.
- Unknown schema versions are refused before anything is written; records that cannot be built are skipped with a reason code in the manifest and on stderr.
- Eval has no pass mark. Model-independent numbers (`valid_rate`, `rule_pass_rate`) are separate from agreement with the named teacher. Truth and answer go through the same `build_clips` rules. Undefined scores are `null`, never NaN and never 1.0; non-finite input is an error.
- The tests use synthetic text and `--replay`-style reply files; nothing calls a model.

## Consequences
- The vision tasks cannot be trained until a label-independent frame sampler exists. The data cannot say what makes a reel perform.
- Scores measure agreement with `reelctl`'s detectors and with one teacher; a model that matches them is not shown to be good.
- Records built from real reels contain verbatim transcripts and must stay out of git; the licence and consent cautions are in [finetune.md](../finetune.md).
- The hand-written SHA-256 is covered by NIST vectors and is in the mutation pass; it is not for secrets.
- The attach-filmstrips request is not met, on purpose; revisit when a sampler exists.
