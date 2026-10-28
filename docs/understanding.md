# understanding.json reference

`reelctl understand` writes one JSON object per long video. The Rust types are in [crates/understand/src/types.rs](../crates/understand/src/types.rs), and the JSON Schema generated from them is [crates/understand/schema/understanding.schema.json](../crates/understand/schema/understanding.schema.json). Regenerate it with `cargo run -p understand --example understanding_schema`.

Every quote in this file is whisper's text, and every time is a whisper time, a `reelctl` measurement, or the time of a sampled frame. The model supplies segment IDs, labels, and the frame it saw an event in, never a time or a quote ([ADR-015](decisions/ADR-015-combine-measured-time-with-a-model-that-answers-by-segment-id.md)). Times are whole milliseconds.

## Top level

| Field | Type | Meaning |
|---|---|---|
| `schema_version` | integer | 1 |
| `source` | object | `path` (the input path as given) and `duration_ms` (from `edit_dna.json`) |
| `model` | object | What the call used; see below |
| `settings` | object | `every_s` (frame spacing), `sheet` (`single`, `2x2` or `3x3`: what was sent), `segment_pause_ms` (400), `quant` (the quantizations allowed) |
| `segments` | array | The numbered transcript pieces the model chose from |
| `summary` | object | `topic`, `language` (as spoken), `speaker_count`, `visual_formats` |
| `beats` | array | Consecutive rhetorical parts of the video |
| `visual_events` | array | Editing events the model saw |
| `cuts` | array | `reelctl`'s cuts, each with what the model saw there |
| `clips` | array | The picks that became clips, in the model's order |
| `rejected` | array | The picks that could not become clips, with the reason |

### model

| Field | Meaning |
|---|---|
| `id` | The model ID asked for |
| `provider` | The provider that served the call |
| `quantization` | That provider's quantization from the listing, or `null` |
| `input_tokens`, `output_tokens` | As the response reports them |
| `cost_usd` | The cost the response reports, or `null` |
| `cost_upper_bound_usd` | The bound computed before the call. It overestimates image tokens, so `cost_usd` is lower |
| `elapsed_ms` | Time of the call |

## segments

Consecutive whisper words, split after a word ending in `.`, `?` or `!`, and where a measured silence gap of at least 400 ms lies between two words (with 150 ms of slack on whisper's word times). On a 28.6-minute lecture this gave 535 segments, median 1.7 s, longest 28 s.

| Field | Meaning |
|---|---|
| `id` | 1-based, contiguous |
| `start_ms`, `end_ms` | First word's start and last word's end |
| `text` | Whisper's words joined by single spaces, never changed |
| `ends_sentence` | The text ends with `.`, `?` or `!`. A sentence starts at segment 1 and after every segment with this set |

## beats

`start_segment`, `end_segment`, `start_ms`, `end_ms` (from those segments), `kind` (hook, premise, claim, evidence, story, example, reset, payoff, cta, tangent or filler), `summary`, `visual`, and `insight_density` (1 filler to 5 dense). A beat whose segment IDs do not exist, or run backwards, is dropped.

## visual_events

`seen_at_ms`, `kind`, `note`. `kind` is one of `hard_cut`, `punch_in`, `zoom_out`, `overlay_on`, `overlay_off`, `b_roll_on`, `b_roll_off`, `slide_change`, `caption_change`. `seen_at_ms` is the time of a sampled frame: the event happened at or before it, and after the previous frame. The model's own time is snapped to the nearest frame time and discarded. Events are in ascending order of `seen_at_ms`.

## cuts

`frame_index` and `time_ms` from `edit_dna.json`, and `vlm_label`: the `kind` and `note` of the event the model saw in the frame after the cut (300 ms later), or else the frame before it (300 ms earlier), or `null`. This is how the output says that a "cut" was a pop-up closing. `edit_dna.json` itself is not changed.

## clips

| Field | Meaning |
|---|---|
| `rank` | The model's ranking, 1 is best. Rejected picks keep theirs |
| `title`, `open_loop`, `visual_suggestion`, `why` | The model's words about the pick |
| `start_ms`, `end_ms`, `length_ms` | The cut points and the clip's length |
| `tight_length_ms` | `length_ms` minus every pause of 180 ms or more inside the clip: the length once cut tight |
| `dead_air` | Those pauses, as `{start_ms, end_ms, duration_ms}`, ready for a cut-silence step |
| `hook`, `payoff`, `last` | `{id, text}`: the hook segment, the payoff segment, and the last segment, with whisper's text |
| `payoff_at_ms` | The payoff's start into the clip on tight time, or `null` when the payoff segment is not inside the clip (which is flagged) |
| `adjustments` | What the clip rules changed in the model's pick |
| `flags` | The rules the clip breaks. Empty means it meets all of them |

The rules, in priority order: hook first, then the cliffhanger, then length. A rule flags or rejects; it does not break a higher one.

1. **Valid.** An unknown segment ID, a cutoff before the hook, or a start after the end puts the pick in `rejected`. It is never repaired.
2. **Open on the hook.** The clip starts at the start of the hook's sentence if that is at most 8 s before the hook; otherwise it starts at the hook and is flagged `starts mid-sentence`. Anything the model put before is dropped and listed in `adjustments`.
3. **End on the cliffhanger.** The clip ends at the end of `cutoff_segment`, never later (that would resolve the open loop) and never earlier.
4. **Length.** A tight length under 30 s or over 60 s is flagged with both lengths, never changed.
5. **Cut points.** The start is the end of the measured pause just before the first segment if that pause ends within 600 ms of it, else 100 ms before the first word. The end is 150 ms into the pause after the last segment (or half of it, if shorter), else 150 ms after the last word.
6. **Other flags.** A payoff not 6–12 s into the clip on tight time, a payoff outside the clip, and overlap with a higher-ranked clip.

To cut a clip with ffmpeg (encoding is not part of `reelctl`):

```bash
ffmpeg -ss <start_ms/1000> -to <end_ms/1000> -i long.mp4 -c:v libx264 -c:a aac clip.mp4
```

`dead_air` lists the pauses to remove for the tight version.

## rejected

`rank`, `title`, and `reason` (for example `cutoff_segment 3 is before hook_segment 5`).
