# Todo: visual-motion

Plan: [plan.md](plan.md). Spec: [docs/specs/visual-motion.md](../docs/specs/visual-motion.md). Tick each box in the commit that completes it. Stage explicit paths; never `git add -A` (`.env` holds a key).

## Task 1: Type and diff
- [x] `VisualEvent` and `visual_events` (defaulted); version 3; version 2 files parse; schema regenerated; `docs/edit-dna.md`
- [x] `static_window` splits at events; no events means today's output; tests

## Task 2: Detector
- [x] Overlay rules (appear, grow, fade, vanish within persistence, brightness shift, moving block)
- [x] Scale fit (about centre and corner, eased over frames, below floor, noise)
- [x] Mutation checks on each threshold

## Task 3: Fixtures
- [x] `motion12`, `still12`, `motion12_wide`, `bench60_textured` and the planted-event sidecar

## Task 4: Wiring
- [x] Events in `edit_dna.json`; `signal_ms` includes them; other fields unchanged on every fixture

## Task 5: End to end
- [x] Planted events found within two frames; `textured_cuts12` probe recorded; `diff` splits

## Task 6: Real footage
- [x] NEET video: first twenty events checked by eye; cost on `bench60_textured` and real reels; docs, status, capability map, README, CHANGELOG
