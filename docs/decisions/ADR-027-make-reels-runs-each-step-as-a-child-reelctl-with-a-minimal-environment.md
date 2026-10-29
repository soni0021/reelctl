# ADR-027: make-reels runs each step as a child reelctl with a minimal environment, and gates the paid call before asking

## Status
Accepted (2026-10-29, with [make-reels](../specs/make-reels.md); written after the audit of 32fbc07 asked for it)

## Date
2026-10-29

## Context
`reelctl make-reels` chains about ten commands. Three facts shape how:
- Two of those commands load an ONNX model: TransNetV2 in `decompile` when it is asked for, and RVM in `reframe`. AGENTS.md forbids loading both in one process.
- Only `understand` needs the OpenRouter key. A live call spends money and sends frames off the machine, so each one needs the user's yes.
- A failure in one clip should not lose the others.

## Decision
- **One child per step.** Each step is a child `reelctl` (`std::env::current_exe()`, so there is no PATH lookup of reelctl), run with `env_clear()`. Only `PATH`, `TMPDIR`, `HOME` and the dynamic-library path variables are passed on. `OPENROUTER_API_KEY` goes to the live `understand` child only, and `ORT_DYLIB_PATH` to `reframe` only. Model and weight paths go as flags. The child's stdin is null.
- **The gate comes before the question.** Before anyone is asked, the cost upper bound from `understand --dry-run` is compared with `--max-cost-usd` (exit 4), and the key's presence is checked (exit 3). The yes comes from a terminal or `--yes`. With no terminal and no `--yes`, the run exits 4. A folder that already has `understanding.json` uses it again, so a re-run never pays twice. Its source file name must match `--input`.
- **One reel's failure is that reel's.** A failed step is recorded in that reel's entry in `make_reels.json` with its exit code and stderr tail, and the next reel goes on. When no reel renders, the run exits with the first failing step's code if that was a refusal (2 to 4), else 1.
- **Arguments are built, never rewritten.** The centred fallback for `reframe` has its own argument list, so no path that happens to read "presenter" can be changed.

## Consequences
- **Cost.** A run spawns one process per step: roughly 10 per reel, plus about 3 for the whole run. That is negligible next to decoding and rendering.
- **What a summary shows.** Each reel's steps are re-runnable by hand from the argument lists recorded in `make_reels.json`, which never hold a key.
- **The card counts as dead air.** `diff` on a rendered reel flags the silent card at the end as dead air and as a static window. This is expected, and the end-to-end test checks only the footage before the card.
