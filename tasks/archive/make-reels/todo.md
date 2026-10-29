# Todo: make-reels

Plan: [plan.md](plan.md). Spec: [docs/specs/make-reels.md](../docs/specs/make-reels.md). Tick each box in the commit that completes it. Stage explicit paths; never `git add -A` (`.env` holds a key).

## Task 0: Spec
- [x] User approves `docs/specs/make-reels.md` and ADR-025 (2026-10-29; captions customizable, reels "custom + picked")

## Task 1: Edits on a trimmed clip
- [x] `timeline from-clip` (refusals, metadata, frame snapping)
- [x] Every edit and the render tested on a clip that starts after 0 (auto-punch-in since the audit fix)

## Task 2: Bridge
- [x] `attach-bridge --append`; old records still read

## Task 3: Auto punch-in
- [x] Windows through the edits, scale from the outliers, idempotent (the curve is not used yet: see the spec)

## Task 4: Reframe
- [x] Compositor alpha-only entry (fake model test)
- [x] Pure plan: centroid, dead zone, speed, ease, cuts, edges, thinning
- [x] Render crop expression checked against ffmpeg; Smelter refuses
- [x] `--follow centre`; fail-closed messages

## Task 5: Captions
- [x] Mapping, grouping, style (presets, file, flags), drawing with `ab_glyph`, font lookup; other edits refuse; `overlay remove --captions`

## Task 6: make-reels
- [x] Child steps with a minimal environment, cost gate and prompt, `make_reels.json`, exit codes
- [x] `lecture70` with words written by the test, `--understand-replay` reply, end-to-end test

## Mac (user)
- [ ] Real lecture run; reframe ms per frame and the presenter-in-frame check recorded in status.md
