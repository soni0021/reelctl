# Plan: make-reels

Spec: [docs/specs/make-reels.md](../docs/specs/make-reels.md) (approved 2026-10-29). ADR: [ADR-025](../docs/decisions/ADR-025-reframe-on-the-presenter-from-the-rvm-alpha-centroid.md). Branch: `claude/keen-clarke-1a8w5h`. Built in a cloud session (Linux). The checks that need the Mac are named at the end.

This is milestone M0 of the ReelStudio plan. The service (`reelctl serve`, a job queue, SQLite and checkpoints) and the SwiftUI app follow in their own specs.

## Order

Each task is one commit, with its docs in the same commit, `cargo test` before the commit, and the `audit` subagent after it.

1. **Edits on a trimmed clip.**
   - `timeline from-clip`.
   - Tests that `cut-silence`, `punch-in`, `attach-bridge` and the render work on a clip that starts after 0, with any fixes they need.
   - Risk: today's ops are tested only on `init` output. A wrong offset there would break every later step, so this goes first.
2. **`attach-bridge --append`.** It reads old bridge records.
3. **`transform auto-punch-in`.**
   - Static windows through the edits, reusing `crates/diff`'s rule.
   - Scale and curve from the outliers.
   - Idempotent.
4. **`transform reframe`.**
   - The compositor's alpha-only entry point.
   - The pure plan in `crates/timeline/src/reframe.rs`.
   - The crop expression in render, checked against ffmpeg's evaluator.
   - Smelter refuses a reframed project.
   - `--follow centre`.
5. **`overlay captions`.**
   - Mapping words through the edits, grouping, the style, drawing with `ab_glyph` ([ADR-026](../docs/decisions/ADR-026-draw-captions-in-reelctl-with-ab-glyph.md)) and the font lookup.
   - The other edits refuse a captioned project.
   - `overlay remove --captions`.
6. **`reelctl make-reels`.**
   - Children run with a minimal environment.
   - The cost gate and the prompt.
   - Each reel's folder, `make_reels.json`, and the exit codes.
   - The `lecture90` fixture with its DNA sidecar and `--replay` reply.
   - The end-to-end test.

## Docs touched

`docs/cli.md`, `docs/timeline.md`, `README.md`, `docs/status.md`, `CHANGELOG.md`, `AGENTS.md` (the module list), and the spec, kept current.

## Mac-only checks (left to the user)

- A real lecture with the Apex whisper model, RVM weights and one live `understand` call after a yes.
- The reframe's ms per frame.
- Whether the presenter stays in frame through the cuts.
- Captions drawn with the macOS default font.
