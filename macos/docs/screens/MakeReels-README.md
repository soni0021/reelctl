# Make reels screen

`MakeReelsScreen` (docs/specs/mac-app.md, screen 9): one long video to finished reels through `pipelines.make_reels`.

| File | What it holds |
|---|---|
| `MakeReelsScreen.swift` | The screen: the form on the left, the stages on the right (decompile progress, the plan and the paid-call sheet, understand progress, the reel cards and the run's `notes`), and the phase strip. |
| `MakeReelsRun.swift` | `MakeReelsRun.shared`, the run's state (the request as it grows, the last stage, the plan), and the flow: `makeReels` → `decompile` (wait, then again with `dna: .fromJob(job, "out")`) → `plan` (`PaidCallSheet`, then `runUnderstand(plan:approvedMaxUsd: plan.costUpperBoundUsd, apiKey: KeyStore.read())`, wait, then again with `understanding: .fromJob(job, "out")`) → `reels`. |
| `MakeReelsDraft.swift` | The form's values (video or its DNA, 1 to 3 outliers as assets or reel subjects, target URL, `reels`, `clips`, `allow_flagged`, `follow`, `card_ms`, `size`, model, cost cap) and the caption style keys of `CAPTION_STYLE`, with their checks. |
| `MakeReelsForm.swift` | The form and the compact caption style section (preset, then overrides). |
| `MakeReelsReelCard.swift` | One card per `ReelJobs`: the eight-step strip (`StatusDot` per job), the running step's `JobProgressView`, the failed step, Cancel reel (cancels every unfinished job of the reel), the rendered `out` in a `PlayerView`, and the `diff` `report` flags (a click seeks the reel). |

Every live call asks in `PaidCallSheet` for that exact plan, every time; a declined or refused plan offers "Ask again", which gets a new plan. The key is read from the Keychain only to pass to `runUnderstand`.

The run lives in `MakeReelsRun.shared` (rather than the view's `@State`), so leaving the screen keeps it and its waiting tasks.

Shared-code needs: see [NOTES.md](NOTES.md).
