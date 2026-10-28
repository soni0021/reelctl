# ADR-016: State the clip length to the model in segments, not seconds

## Status
Accepted. Refines [ADR-015](ADR-015-combine-measured-time-with-a-model-that-answers-by-segment-id.md); it does not supersede it.

## Date
2026-10-28

## Context
The first live run of `reelctl understand` on the 28.6-minute lecture (Qwen3.5-9B, $0.0082) returned no clip that met the rules: two picks were 130 and 234 s long against a 60 s maximum, with the payoff about 50 s in. The prompt said "30-60 s" and the transcript lines carry times, but the model has to add up times over 535 lines to know a span's length, and a 9B model does not. Adding a bolder "hard limit" sentence did not help (one pick of 92 s, others with segment IDs that do not exist). Counting IDs is something a model does reliably.

## Decision
`system_prompt` takes the segments and writes the length limit in segments: the video's mean segment length (first start to last end over the number of segments, pauses included) turns 50 s, 60 s and 9 s into "about N segments after `start_segment`", "never more than M" and "the payoff about P segments in". It also tells the model that pauses over 0.18 s are cut afterwards, so a span should be 45 to 60 s before cutting. The rules in code (ADR-015) are unchanged and still decide what passes.

## Consequences
- The prompt is built per video, so the input-token estimate uses the built prompt.
- Measured on the lecture, 2026-10-28, same frames and transcript, tight length 30–60 s: Qwen3.5-9B went from 0 of 3 clips to 4 of 5 (cost $0.0088); Muse 1.2 from 1 of 5 to 3 of 4 (cost $0.127). Clips with every rule met: Qwen 1 of 5, Muse 2 of 4. What still fails is mostly the payoff time (the model's payoff segment lands 3.7–15 s in, outside 6–12 s) and one mid-sentence start.
- The mean segment length is a guide, not a bound: segments vary from under a second to 28 s. The model is still allowed to be wrong, and the code still flags it.
- Counting IDs does not teach the model where the payoff is. A second pass that re-asks about a flagged clip would, but it would add a call and cost, and ADR-015 rejects silent retries. It is left for a later decision.
