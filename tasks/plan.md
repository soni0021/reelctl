# Plan: ReelStudio, the Mac app (M2–M8)

Spec: [docs/specs/mac-app.md](../docs/specs/mac-app.md) (approved 2026-10-29). ADR: [ADR-032](../docs/decisions/ADR-032-the-mac-app-is-a-thin-client-of-reelctl-serve.md). The service it drives (M1) is archived in `tasks/archive/studio-service/`. Branch: `claude/keen-clarke-1a8w5h`, merged to `main` per milestone.

The cloud session writes the Swift but cannot compile it (there is no toolchain, and swift.org is blocked). Compiling happens on the user's Mac: over Remote Control when that folder is attached, or by the user from [mac-todo.md](mac-todo.md).

## Order

1. **M2, the foundation.**
   - `ReelCore`: the client, models, daemon launcher, Keychain, `.cube` reader, and the `Playable` links.
   - `ReelCoreTests`.
   - The app shell: the sidebar, the banner and the connection badge.
   - Jobs, History, Storage, Settings and Doctor, onboarding, and the ⌘K palette.
   - `make -C macos test app dmg`, and the Release workflow.
2. **M3, analyse:** Library (decompile lanes, frame-exact seek, filmstrip) and Compare (diff).
3. **M4, edit:** Projects, the track view and every edit operation as a checkpointed job, with undo and branch.
4. **M5:** Look (a live `.cube` wipe) and Presenter.
5. **M6:** Understand (plan, then the paid-call sheet, then clips) and Datasets/Eval.
6. **M7:** Overlays (the chart spec form).
7. **M8:** Make reels (`pipelines.make_reels` in three stages).
8. **Build on the Mac:** `swift build` until clean, then `swift test`, then `make dmg`, then the app checks in `mac-todo.md`.

M2 to M8 were written together on 2026-10-29: ReelCore and the shell by the main session, and the screens by six parallel agents, one folder each. That was followed by a cross-file review for compile errors.
