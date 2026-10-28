# Tasks: long-to-short clipper

Plan: [tasks/plan.md](plan.md). Spec: [docs/specs/long-to-short-clipper.md](../docs/specs/long-to-short-clipper.md). Tick each box in the commit that completes it. Stage explicit paths; never `git add -A` (`.env` holds a key).

## Task 0: Crate skeleton and contract

- [x] `crates/understand` with the types, traits, constants, and stubs from the spec; `ureq`, `base64`, and `image`'s `jpeg` feature added; compiles with no warnings; `cargo clippy` clean
- [x] The model's prompt and reply schema copied into the crate (`prompt.md`, `reply_schema.json`)

## Task 1: Segmenter

- [ ] Splits after `.`, `?`, `!`; splits at a 400 ms pause and not a 399 ms one; 150 ms slack on word times; keeps the first and last words; empty input gives empty output; IDs 1-based and contiguous; `ends_sentence` set; text is whisper's words joined by single spaces
- [ ] Red commit compiles; green commit passes; no `unwrap` in library code

## Task 2: Clip rules

- [ ] Opens on the hook's sentence start within 8 s, else starts at the hook and flags `starts mid-sentence`; drops material before the hook's sentence and records it in `adjustments`
- [ ] Ends at the cutoff segment's end, never extended or trimmed
- [ ] Tight length and `dead_air` (pauses of 180 ms or more inside); flags, never changes, tight lengths under 30 s or over 60 s
- [ ] Rejects out-of-range IDs and a cutoff before the hook, with the reason; never repairs
- [ ] Cut points: start at the end of a pause within 600 ms before the first segment, else 100 ms before the first word; end 150 ms into the pause after the last segment (or half of it), else 150 ms after the last word
- [ ] Flags a payoff outside 6–12 s of tight time, and overlap with a higher-ranked clip
- [ ] Red commit compiles; green commit passes; no `unwrap` in library code

## Task 3: Frame plan, sheets, JPEG, prompt, estimate, cost bound

- [ ] `sample_times`: every n s plus 300 ms before and after each cut, merging times within 100 ms, ascending, no duplicates, none past the duration
- [ ] `tile` (2×2, 3×3, order, empty tiles in the last sheet, times in reading order); `encode_jpeg`; `choose_layout` (auto picks single, then 2×2, then 3×3 by context; fixed modes return themselves)
- [ ] `segment_lines` (`G12 [0:41.2-0:43.0] text`, trailing `…` for a mid-sentence pause); `estimate_input_tokens`; `eligible_endpoints`; `cost_upper_bound` matches a hand count with the highest price
- [ ] `response_schema` parses and matches the reply types
- [ ] Red commit compiles; green commit passes; no `unwrap` in library code

## Task 4: Assembly

- [ ] Beats get times from their segments; event `t_s` snaps to the nearest sampled frame time; each cut gets the `vlm_label` of the event in the frame at its time ± 300 ms, else `null`
- [ ] Every quote equals the text of its segments; every time is a segment, pause, cut, or frame-sample time; the result serializes and deserializes
- [ ] Red commit compiles; green commit passes; no `unwrap` in library code

### Checkpoint A
- [ ] Four branches merged; `cargo test -p understand` passes; `git check-ignore -v .env` holds

## Task 5: `decompile::decode_frames_at`

- [ ] Decodes one frame at each time in a list (nearest frame at or after the time), scaled to 448 px wide, as RGB; exact times; works for 8-bit and 10-bit input; tested on fixtures
- [ ] Docs: `docs/status.md` note, CHANGELOG not needed (no user-visible change)

## Task 6: OpenRouter backend and catalog

- [ ] Backend: key from `OPENROUTER_API_KEY` only; `data_collection: deny` and `require_parameters` on every request; strict json_schema; no retry with looser settings; the key is only in the header
- [ ] Catalog: the public endpoint listing (no key), parsed into `EndpointInfo`
- [ ] Offline tests against recorded response bodies; a dummy key never appears in an error message

## Task 7: `reelctl understand`

- [ ] Flags as in the spec; hidden `--replay` and `--catalog`; exit codes 1, 2, 3 (no key), 4 (cost bound over `--max-cost-usd`); `--dry-run` never calls the model
- [ ] End-to-end test through `--replay` and `--catalog`; the helper removes `OPENROUTER_API_KEY`; the key never appears in stdout, stderr, or the output file
- [ ] Schema file generated (`cargo run -p understand --example schema`) and a test that it is up to date

### Checkpoint B
- [ ] `cargo test --workspace` passes offline; `--dry-run` on the real lecture quotes a cost bound

## Task 8: Docs, ADR-015, audit

- [ ] `docs/understanding.md`, `docs/cli.md`, README, `docs/status.md`, CHANGELOG, AGENTS.md (the VLM role, "P1–P3 not implemented"), ADR-015, capability map
- [ ] The `audit` subagent run on the final commit; findings fixed

## Task 9: Live run (needs the user's yes)

- [ ] Qwen3.5-9B on the lecture: cost at most $0.02, at least 3 valid clips; the pop-up cut labelled; Muse 1.2 for comparison; rule pass rates in `docs/status.md`

### Checkpoint C
- [ ] Criteria 1–4 and 6 verified, 5 verified or marked pending; audit run; reviewed with the user, merged into `main`, and this plan moved to `tasks/archive/long-to-short-clipper/`
