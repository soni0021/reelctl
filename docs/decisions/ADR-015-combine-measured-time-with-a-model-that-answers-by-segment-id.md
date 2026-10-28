# ADR-015: Combine measured time with a model that answers by segment ID

## Status
Accepted

## Date
2026-10-28

## Context
`reelctl` measures a video exactly (cuts, pauses, timed words) but does not understand it. A creator with a 20–60 minute informational video wants 3–5 Reels cut from it, each opening on a hook, promising a payoff about 9 s in, and ending on an open loop (planer.md Component 3). A vision-language model understands content, but in prototype runs on a 28.6-minute Hinglish lecture and a 38.5 s reel (kept outside the repo) it got times wrong and rewrote speech. The requirements are in the [spec](../specs/long-to-short-clipper.md).

Prototype results (2026-10-28):
- **Raw video to Muse 1.2 ($0.12):** right topics within about 2 s, and it named the 10:06 pop-up that `reelctl` counted as a cut. It also rewrote quotes, invented a cut at 27:55, and its times drifted 0.5–2 s.
- **Frames plus numbered whisper segments, three models:** every quote verbatim and every time exact (77 of 77 lines), and no model invented a cut. But the models broke the clip rules (Muse kept 2 of 4 clips within 30–60 s, Qwen3.5-9B 2 of 5), and some hooks were mid-sentence pieces.
- **Once measured pauses are cut,** clip lengths came far closer to the target.
- **Cost per lecture:** Qwen3.5-9B $0.008, Gemma 4 26B $0.007, Muse 1.2 $0.12.

## Decision
- **The model answers by segment ID, never by time or quote.** `reelctl understand` numbers whisper's words into segments (split at sentence ends and measured pauses of at least 400 ms: 535 segments, median 1.7 s, on the lecture) and sends them with sampled frames. The model returns segment IDs, labels, and the frame time it saw an event in. Every quote in `understanding.json` is whisper's text, and every time is a whisper, `reelctl`, or frame-sample time. An event's time is snapped to the nearest sampled frame.
- **Frames come from a plan, not luck:** every 5 s, plus one 300 ms before and one 300 ms after every `reelctl` cut, so a pop-up between two samples is seen next to the cut that it caused. Frames are 448 px wide JPEGs, tiled 2×2 or 3×3 only when the request would not fit the model's context.
- **Clip rules are code, not prompt.** Priority: hook first, then keep the cliffhanger, then length. A rule flags or rejects instead of breaking a higher-priority one, and every change is listed in `adjustments`.
- **Hosted model through OpenRouter, behind two traits** (`VlmBackend`, `ModelCatalog`), so a local backend can follow. Every request has `data_collection: deny` and `require_parameters`, and there is no retry with looser settings: no provider means an error that says so.
- **The key is the environment's.** `OPENROUTER_API_KEY` is read from the process environment only, sent only as a header, hidden from `Debug`, scrubbed from errors, and never written. The default test suite removes it from the environment of every spawned `reelctl`.
- **Fail closed and say what it would cost.** Exit 3 without a key, exit 4 when the cost upper bound (input tokens at the highest eligible price, 1,445 tokens per image, plus `max_tokens` at the highest output price) is over `--max-cost-usd`. `--dry-run` prints the plan and the bound without calling the model.
- **A new crate, `understand`, independent of `decompile`.** Frames arrive as images with their times, so its tests need no ffmpeg or whisper build. `decompile::decode_frames_at` decodes them.
- **Default model `qwen/qwen3.5-9b`.** About 1¢ per lecture; `--model meta/muse-spark-1.2 --quant unknown` is the higher-quality option at about 12¢.

## Alternatives Considered

### Send the raw video to the model
- Best at seeing edits, but it rewrote quotes, invented a cut, and drifted by up to 2 s. It also sends the whole video off the machine and costs 15 times more.

### Ask the model for times and quotes, and correct them afterwards
- Fuzzy matching of a rewritten Hinglish quote to the transcript is a guess. Answering by ID makes it impossible to be wrong about the words.

### Put the clip rules in the prompt
- The prototype's models broke them (lengths, mid-sentence hooks). Code can be tested; a prompt can only be hoped at.

### A local vision-language model
- Not tried; the trait leaves room for it. Out of scope for v1.

### Encode the clip files
- Out of scope: `clips[]` and `dead_air` give exact times and the documented ffmpeg command cuts them. Cutting belongs to the P2 timeline engine.

## Consequences
- `reelctl` gains its first network-using command and its first dependencies on an HTTP client (`ureq` with rustls) and `base64`; `image` gains the `jpeg` feature. No other command touches the network.
- The cost bound overestimates: OpenRouter's pre-flight check counted about 1,445 tokens per 448 px image, while the tokens billed were 170–360. The actual cost is recorded next to the bound.
- The model's own `rank` field is ignored: the order of `reel_candidates` is the ranking.
- Quality depends on the model. The first measured results are in [status.md](../status.md#long-to-short-clipper-2026-10-28).
- The P3 "VLM" role in planer.md now has a first implementation behind a command, not a service.
