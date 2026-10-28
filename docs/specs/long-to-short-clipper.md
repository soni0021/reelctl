# Spec: Long-to-short clipper (`reelctl understand`)

Status: approved 2026-10-28 (built; first live run 2026-10-28, see [status](../status.md)) by the user's message "invoke multiple agent and complete it", with the recommended answer to each open question, the three new dependencies, and the plan written from this spec. The paid live run (criterion 5) is not covered by that approval and needs its own yes. Branch: `feature/long-to-short-clipper`, cut from `main` after `chore/ignore-env` was merged. Implements the first slice of [planer.md](../../planer.md) Component 3 ("Long-to-Short Open-Loop Clipper") and the VLM role planer.md gives to P3. It does not add `visual-motion` (P0 capability map), which stays open.

## Objective

A creator has a 20–60 minute informational video and wants 3–5 Reels cut from it, each 30–60 s. Each Reel should:
- open on a hook
- promise a payoff around 9 s in
- end on an open loop that sends viewers back to the full video

`reelctl` measures time exactly but does not understand content. A vision-language model understands content but gets times wrong and rewrites speech. This command combines them, so each does only what it is accurate at:

| Layer | Job | Where it runs |
|---|---|---|
| `decompile` + whisper (existing) | Cuts, pauses, timed words | Local |
| Segmenter (new) | Split words at punctuation or a measured pause of at least 400 ms, then number the pieces | Local |
| Frame sampler (new) | A JPEG every N seconds, plus one just before and one just after each `reelctl` cut | Local |
| VLM backend (new trait) | From the frames and numbered segments: topic, beats, named visual events, ranked clip picks **by segment ID** | Hosted (OpenRouter) first; local later behind the same trait |
| Clip rules (new) | Open on the hook's sentence, end on the cliffhanger, check the length after pauses are cut, place cuts inside measured pauses | Local |

Every quote in the output is whisper's exact text, and every time is a whisper, `reelctl` or frame-sample time. The model supplies segment IDs, labels and the frame it saw an event in, never a time or a quote.

## Evidence (prototype, 2026-10-28)

A Python prototype in `Reels/meta_vu/` (git-ignored) was run on a 28.6-min Hinglish lecture and a 38.5 s reel. Full results are in `Reels/out/vlm_compare.md`.

- **Muse 1.2 on raw video ($0.12):**
  - It found the right topics, within about 2 s of where they're said.
  - It named the 10:06 pop-up that `reelctl` had counted as a cut.
  - But it rewrote quotes, invented a cut at 27:55, and its times drifted 0.5–2 s.
- **Frames + numbered segments:**
  - Every quote was verbatim and every time exact: 77 of 77 lines across three models. No model invented a cut.
  - The models broke the clip rules. Muse kept 2 of 4 clips within 30–60 s (4 of 4 on raw video), and Qwen3.5-9B 2 of 5. Some hooks were mid-sentence pieces. Hence clip rules in code.
  - **Lengths once measured pauses are cut** were far closer to the target: Qwen's 72, 74 and 90 s clips came to 52, 61 and 52 s, and Muse's 65 s to 52 s.
  - All three frames runs missed the 7 s pop-up, which fell between 5 s samples. Hence frames around every `reelctl` cut.
- **Cost per lecture:** Qwen3.5-9B $0.008 (108 s); Gemma 4 26B-A4B $0.007 (40 s, poor on long input, n=1); Muse 1.2 $0.12 (32–43 s). `decompile` takes 18.7 s and whisper 9.4 min of CPU, both free.
- **Segments:** whisper's punctuation-only sentences are too coarse (longest 44 s; 14 run past 15 s). Splitting also at measured pauses of 400 ms or more gives 535 segments: median 1.7 s, longest 28 s.
- **Image limits:** OpenRouter's pre-flight check counted about 1,445 tokens per 448 px image. This was observed in one run and isn't documented. The tokens actually billed were about 170–360. So 344 single frames exceeded a 262k-context model, while 2×2 sheets (86 images) fit.

## Tech Stack

The Rust workspace as today. New dependencies, which need approval (see Boundaries):
- **`ureq`**, a blocking HTTPS client with rustls, for OpenRouter.
- **`base64`**, for data URLs.
- **The `jpeg` feature of the existing `image` crate**, for frames.

Frame decoding reuses `ffmpeg-next` through `crates/decompile`.

## Commands

```
reelctl decompile --input talk.mp4 --out talk.edit_dna.json --whisper-model <ggml.bin>   # existing; needs whisper words
reelctl understand --input talk.mp4 --dna talk.edit_dna.json --out talk.understanding.json \
    [--model qwen/qwen3.5-9b] [--quant fp8,bf16,fp16] [--every-s 5] [--sheet auto|1|2|3] \
    [--effort low|medium|high] [--max-cost-usd 0.50] [--dry-run]
cargo test -p understand
cargo test -p reelctl --test p0_fixture
cargo run -p understand --example understanding_schema
```

- **The key.** `OPENROUTER_API_KEY` is read from the environment, for example via `set -a; source .env; set +a`. It is never printed, logged, or written.
- **`--dry-run`.** Plans the frames and segments, and fetches the model's public context and price listing (unauthenticated; no key needed). It then prints the image count, segment count, and the cost upper bound. It does not call the model.
- **Exit codes.** Existing codes keep their meaning ([cli.md](../cli.md#exit-codes)).
  - 1: runtime errors, including a failed or unusable backend reply
  - 2: argument errors, including a `--dna` without whisper words (`asr_backend` is not `whisper_cpp`), which exits with a message as `diff` does for a fourth `--outlier`
  - 3 (new): no `OPENROUTER_API_KEY`
  - 4 (new): the cost upper bound exceeds `--max-cost-usd`

## Project Structure

- `crates/understand/` (new crate):
  - `segments.rs`: the segmenter, pure.
  - `frames.rs`: the sampling plan, and 2×2 or 3×3 sheet tiling with `image`.
  - `catalog.rs`: the `ModelCatalog` trait (context length and prices per eligible endpoint), with `OpenRouterCatalog` and `FixedCatalog` for tests.
  - `backend.rs`: the `VlmBackend` trait, with `RecordedBackend` for tests.
  - `openrouter.rs`: the `OpenRouterBackend`.
  - `request.rs`: the prompt, the strict schema, and the cost bound.
  - `clips.rs`: the clip rules, pure.
  - `lib.rs`: the `Understanding` output types, with serde and schemars derives.
  - **`understand` does not depend on `decompile`** (or ffmpeg or whisper.cpp). Frames reach it as JPEG bytes with their times, so `cargo test -p understand` builds fast and offline. `reelctl` connects the two.
  - `examples/schema.rs`.
- `crates/decompile`: a public function that decodes one frame at each of a list of times. This is new; it reuses the existing decoder.
- `crates/reelctl/src/main.rs`: the `understand` subcommand, with two hidden flags (like `whisper-worker`, not in `--help`, documented in `cli.md`):
  - `--replay <reply.json>`: use a recorded model reply instead of calling the backend.
  - `--catalog <catalog.json>`: use a fixed model catalog instead of fetching the public listing.
  They exist so the end-to-end test, which spawns the `reelctl` binary, runs with no network.
- **Tests:**
  - unit tests in `crates/understand`
  - an end-to-end test in `crates/reelctl/tests/understand_fixture.rs` (recorded backend, fixed catalog)
- **Docs:**
  - `docs/understanding.md`: every `understanding.json` field, the schema, and the ffmpeg command to cut a clip
  - `docs/cli.md`, README, `docs/status.md`, CHANGELOG, and AGENTS.md (P3 VLM role, "P1–P3 not implemented")
  - ADR-015
  - `tasks/plan.md` and `tasks/todo.md`

## Behavior

**Inputs.**
- `--input`: the video, which `decompile` already reads.
- `--dna`: its `edit_dna.json`, from `decompile` with a whisper model. Its words, `silence_gaps`, `cuts` and `duration_ms` are used, and the audio is not decoded again.

**Segments.** Consecutive whisper words, split:
- after a word ending in `.`, `?` or `!`, or
- where a `silence_gap` of at least `SEGMENT_PAUSE_MS` (400) lies between two words, allowing 150 ms of slack on whisper's word times.

Each segment has an `id` (1-based), `start_ms`, `end_ms`, `text` and `ends_sentence`. A *sentence start* is segment 1 or any segment after one with `ends_sentence`.

**Frames.**
- **Times:**
  - every `--every-s` seconds (default 5 s) from 0 to the duration
  - plus, for each `reelctl` cut at time *t*, the frames at *t* − 300 ms and *t* + 300 ms (merged when two times fall within 100 ms of each other)
- **Format:** scaled to 448 px wide, JPEG.
- **Sheets.** `--sheet auto` sends single frames when the request fits, and otherwise uses the smallest sheet size (2×2, then 3×3) that does. A request fits when image count × `IMAGE_TOKEN_ESTIMATE` (1,445), plus segment and prompt tokens, plus `max_tokens`, is within the model's context from the catalog.
- **Labels.** Each image is preceded by a text part giving its time or times. Sheets list theirs in reading order.

**Request.**
- **Content:** a system prompt with the Component 3 rubric. The user content is the frames, then the numbered segments, one per line (`G12 [0:41.2–0:43.0] text`, with `…` marking a segment that ends at a pause mid-sentence).
- **Response format:** strict json_schema, asking for:
  - summary: topic, language, visual formats
  - beats, by segment range
  - visual events, each with the frame time it is first seen in, a kind (from the existing enum, `hard_cut` … `caption_change`) and a note
  - 3–5 clip picks: `start_segment`, `hook_segment`, `payoff_segment`, `cutoff_segment`, `end_segment`, title, `open_loop`, `visual_suggestion`, `why`
- **Provider settings:** always `{require_parameters: true, data_collection: "deny", quantizations: --quant}`. The default is fp8, bf16 and fp16. Meta's own Muse endpoint reports `unknown` and needs `--quant unknown`.
- **Settings:** `max_tokens` 16,000; `reasoning` is sent only with `--effort`.
- **No silent retry.** If no provider qualifies, or the call fails, the command exits 1 with the backend's message. It never retries with looser settings.

**Cost bound.** The bound is (estimated input tokens × the *highest* input price among eligible endpoints) + (`max_tokens` × the highest output price). It uses the same 1,445 per image, so it overestimates. It is checked against `--max-cost-usd` (default 0.50) before sending, and is labelled an upper bound wherever it is printed. The actual cost from the response is recorded in the output.

**Clip rules.** Applied to each pick in this priority order: **hook first, then keep the cliffhanger, then length.** A rule never breaks a higher-priority one; when it would have to, it flags instead.
1. **Validity.** Any segment ID out of range, or `cutoff_segment` before `hook_segment`: the pick goes to `rejected` with the reason. It is never repaired.
2. **Open on the hook.** The clip starts at the sentence start of the sentence containing `hook_segment`, if that is no more than `HOOK_BACKUP_MAX_MS` (8,000) before the hook. Otherwise it starts at `hook_segment` itself and is flagged `starts mid-sentence`. Anything the model put before the hook's sentence is dropped, and recorded in `adjustments`.
3. **End on the cliffhanger.** The clip ends at the end of `cutoff_segment`. It is never extended past it, since that would resolve the open loop, and never trimmed before it.
4. **Length.**
   - **Tight length:** the clip length minus every measured pause of at least 180 ms inside it. The pauses are listed in `dead_air`, ready for a cut-silence step. This is the length of the Reel once cut tight.
   - **Out of range:** a tight length under `CLIP_MIN_MS` (30,000) or over `CLIP_MAX_MS` (60,000) is flagged with both lengths, never changed.
5. **Cut points.**
   - Start: the end of the measured pause just before the first segment, within 600 ms; otherwise 100 ms before the first word.
   - End: 150 ms into the measured pause after the last segment (or half the pause, if shorter); otherwise 150 ms after the last word.
6. **Other flags.** Payoff not 6–12 s into the clip, measured on tight time. Overlap with a higher-ranked clip.

**Visual event times.** The model reports the frame time it first saw an event in. That time is snapped to the nearest sampled frame time and stored as `seen_at_ms`, so the output never carries a time the model made up. The event happened at or before that frame, after the previous frame.

**Cut labels.** Each `reelctl` cut gets a `vlm_label`: the kind and note of the visual event seen in the frame at *t* + 300 ms or *t* − 300 ms, else `null`. This is how the output can say that the 10:14.1 "cut" is a pop-up closing. `edit_dna.json` is not changed.

**Output: `understanding.json`, `schema_version` 1.**
- **Run details:** `source` (input path, duration), `model` (id, provider, quantization, tokens, actual cost in USD, cost upper bound, elapsed ms), and `settings` (`every_s`, sheet mode, `segment_pause_ms`, `quant`).
- **Content:**
  - `segments`, `summary`
  - `beats`, with segment IDs and times
  - `visual_events`, with `seen_at_ms`
  - `cuts`, with `vlm_label`
  - `clips`: rank, title, `start_ms`, `end_ms`, `length_ms`, `tight_length_ms`, `dead_air`, hook/payoff/last segment IDs with their exact text, `payoff_at_ms` (tight), `open_loop`, `visual_suggestion`, `why`, `adjustments`, `flags`
  - `rejected`
- **Rule:** every text field that quotes speech is copied from `segments`.

**Out of scope for v1:**
- Encoding clip files. `clips[]` and `dead_air` give exact times, and `docs/understanding.md` shows the ffmpeg command. Cutting belongs to the P2 timeline engine.
- Extra frames at scene-score changes, beyond the frames around `reelctl` cuts.
- A local VLM backend.
- Sending audio.
- Correcting whisper's words from slide text.
- Short-reel analysis.
- Changes to `decompile` output or `diff`.

## Code Style

Match `crates/diff` and `crates/decompile`:
- plain structs with serde and schemars derives
- constants named for their limits (`SEGMENT_PAUSE_MS`, `HOOK_BACKUP_MAX_MS`, `CLIP_MIN_MS`, `CLIP_MAX_MS`, `IMAGE_TOKEN_ESTIMATE`)
- `thiserror` errors in the library and `anyhow` only in `reelctl`
- no `unwrap` in library code
- the backend and the catalog as traits, like `CutDetector`:

```rust
pub trait VlmBackend {
    fn analyze(&self, request: &VlmRequest) -> Result<VlmReply, BackendError>;
}

pub trait ModelCatalog {
    fn endpoints(&self, model: &str) -> Result<Vec<EndpointInfo>, CatalogError>; // context, prices, quantization
}
```

## Testing Strategy

Test-first. No test uses the network: the live run (criterion 5) is done by hand with the user's yes. Unit and end-to-end tests use `RecordedBackend` and `FixedCatalog`.

- **Segmenter.** Splits at punctuation; splits at a 400 ms pause and not at a 399 ms one; respects the 150 ms slack; keeps the first and last words; gives empty output for no words; IDs are 1-based and contiguous; marks sentence starts.
- **Clip rules.** Each one on hand-built segments and pauses:
  - backs up from a mid-sentence hook to its sentence start within 8 s; flags `starts mid-sentence` beyond 8 s
  - drops what came before the hook's sentence
  - never ends after the cutoff
  - computes the tight length and `dead_air`
  - flags (does not change) tight lengths under 30 s and over 60 s
  - rejects out-of-range and reversed IDs
  - places cut points inside pauses, with the 600 ms and 150 ms rules
  - flags overlaps
  - flags a payoff outside 6–12 s of tight time
- **Frames and request.**
  - frames at every *n* s plus *t* ± 300 ms around each cut, merging close times
  - `--sheet auto` picks single frames, 2×2 or 3×3 from the context
  - text labels match the frame times
  - visual event times snap to frame times
  - `vlm_label` comes from the event in the frames around each cut
  - the cost bound matches a hand count using the highest price
- **Mapping.** A recorded reply becomes `understanding.json` with verbatim text, and validates against the generated schema.
- **Fail closed.**
  - no key: exit 3, and the backend is never called
  - `--dna` without whisper words: exit 2
  - a bound over the cap: exit 4, and the backend is never called
  - a backend error: exit 1
  - `--dry-run`: never calls the backend
- **The key.** It never appears in stdout, stderr or the output file (the test uses a dummy key). The `reelctl()` test helper removes `OPENROUTER_API_KEY` from the child's environment, like `REELCTL_WHISPER_MODEL`, so a shell that has sourced `.env` can never make a paid call from the default suite; AGENTS.md says so.
- **End to end** in `understand_fixture.rs`, through `--replay` and `--catalog`: a fixture video, a hand-written `edit_dna.json` with words and one cut, a fixed catalog file, and a recorded reply produce the expected clips and a non-null `vlm_label` for the cut.
- **Live:** by hand, never in the suite: the lecture through `qwen/qwen3.5-9b` with the user's yes (criterion 5).
- **Manual, not committed:** the NEET lecture with Qwen3.5-9B and with Muse 1.2, recorded in `docs/status.md`.

## Boundaries

- **Always:**
  - `data_collection: "deny"` on every request
  - fail closed, with a clear message and the exit codes above
  - tests offline by default
  - the key never written anywhere
  - docs in the same commit
  - quotes and times only from whisper, `reelctl` or frame samples
- **Ask first:**
  - adding `ureq`, `base64` and the `image` `jpeg` feature (this spec asks)
  - changing the default model
  - sending audio or raw video
  - changing `edit_dna.json`
  - encoding clip files
  - using `:free` endpoints
- **Never:**
  - commit videos, frames, model replies or keys
  - retry with looser provider settings without saying so
  - let the model's times or quotes into the output
  - weaken CONSTRAINTS.md

## Success Criteria

1. `reelctl understand` with `RecordedBackend` and `FixedCatalog` on the fixture writes an `understanding.json` that validates against the schema. Every quoted text equals the text of its segments, and every time is a segment, pause, cut, or frame-sample time.
2. Every clip either meets all of these or carries a flag naming the rule it breaks:
   - starts at a sentence start, at most 8 s before its hook
   - ends on its cutoff segment: 150 ms into the pause after it (or half of it, if shorter), else 150 ms after its last word (rule 5; no word after the cutoff is kept)
   - tight length 30–60 s
   - payoff 6–12 s in
   - cut points inside measured pauses, or within the stated distance of a word
3. Fail-closed behavior and exit codes 1–4 work as specified. No model request is sent without a key, over the cost cap, or in `--dry-run`.
4. No key appears in any output. Every request carries `data_collection: deny`.
5. **Real run.** On the 28.6-min NEET lecture:
   - Qwen3.5-9B costs $0.02 or less and returns at least 3 valid clips.
   - The 10:14.1 cut gets a `vlm_label` naming the pop-up, with Qwen3.5-9B or Muse 1.2.
   - The pass rate of each clip rule is recorded in `docs/status.md`, with a date and machine, for Qwen3.5-9B and Muse 1.2.
6. ADR-015 records the decision. `docs/understanding.md`, `cli.md`, README, `status.md`, CHANGELOG and AGENTS.md match the code. `cargo test --workspace` passes offline.

## Resolved Questions (approved 2026-10-28 with the recommended answers)

1. **Where does the key come from?** The environment only (`OPENROUTER_API_KEY`), loaded from `.env` by the shell.
2. **Default model?** `qwen/qwen3.5-9b`, with `--model meta/muse-spark-1.2 --quant unknown` documented as the higher-quality option.
3. **Encode the clip files in v1?** No. Exact times and `dead_air`, plus the documented ffmpeg command.
4. **Priority.** Built before `visual-motion`, which stays next.
5. **Branch.** `chore/ignore-env` merged into `main` first (2026-10-28), then this branch cut from `main`. `.env` holds a live key and is ignored there; every commit on this branch stages explicit paths, never `git add -A`.
