# Understand (paid)

Screen 6 of [the app spec](../../../../../docs/specs/mac-app.md); the paid-call rule is [ADR-031](../../../../../docs/decisions/ADR-031-a-live-understand-call-needs-the-plans-bound-approved-exactly.md).

Files:

- `UnderstandScreen.swift`: `UnderstandScreen`, the two columns, the `PaidCallSheet` presentation, and clip playback (load the source, seek to `start_ms`, pause at `end_ms` through a 100 ms check that arms once the player is inside the range).
- `UnderstandFormView.swift`: the form (subject, video, DNA, model, `every_s`, sheet, effort, the dry run's cap) and `UnderstandPlanCard` (frames, images, bound, a 1 s countdown to `expires_at`, the "Spend up to $X" button).
- `UnderstandSession.swift`: the DNA check (`EditDNA.load`, `hasWhisperWords`), `understand.plan`, `understand.run` and the result (`Understanding.load` of the job's `out`, or the subject head's `out`).
- `UnderstandResultView.swift`: the model header (id, provider, cost, bound), the player, the clip rows (flags in orange) and the rejected list.
- `UnderstandInputs.swift`: `UnderstandForm`, `UnderstandDNACheck` and its status line, and `UnderstandDNAPicker` (an imported `edit_dna` asset or a reel subject's head `out`), which the Datasets screen reuses.

The paid call:

- `runUnderstand` is called only from `PaidCallSheet`'s approve button, for the plan the sheet showed, with `approvedMaxUsd: plan.costUpperBoundUsd` and `apiKey: KeyStore.read()`. The key is a local constant in that call; it is never stored, logged or shown.
- The plan is dropped when any form value changes, when it is used (whatever the answer), and when it expires (15 minutes); the Spend button is hidden after expiry.
- No key: a banner, and `model.selection = .settings`.

Shared code needs: none. `.sheet(item:)` would be tidier if `UnderstandPlan` were `Identifiable` in ReelCore (`id` = `plan`); the screen uses `.sheet(isPresented:)` instead so it adds no retroactive conformance.

Not compiled here (no Swift toolchain in the container).
