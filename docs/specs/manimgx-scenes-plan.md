# Plan: manimgx chart overlays

The spec is [manimgx-scenes.md](manimgx-scenes.md); the decision is [ADR-022](../decisions/ADR-022-draw-chart-overlays-with-a-separately-installed-manimgx-and-key-them-over-the-video.md). Branch `feature/manimgx-scenes`. Written 2026-10-29 under the user's "p1-p3". It is kept here and not in `tasks/` so it does not collide with the plans of the parallel slices (RobustVideoMatting, Smelter, the P3 harness).

## Order

Vertical slices, each one test-first, each ending with `cargo check` and its tests green and the docs for it written.

1. **Step 0: find out what manimgx is.** Done. Read the repo, `llms.txt` and the reference pages; the tool is real, writes only x264 MP4, has `inspect` with exit 1 on layout problems, is Python 3.13/3.14 only, was not installable here (refused). Recorded in the spec and ADR-022, with the seven assumptions.
2. **Overlay track in `crates/timeline`** (pure): the marked second `Video` track, `plan_overlay`, `add_overlay`, `remove_overlay`, `overlays`, `refuse_overlays`; `Timeline::track`, `check_editable`, `single_media` and `media_info` skip the track; `cut-silence`, `punch-in` and `attach-bridge` refuse a project with overlays. Tests: shape, idempotence, overlap, bounds, remove restores the bytes, refusals.
3. **Compositing in `build_render_plan`** (pure): the `colorkey`/`overlay` filters and `RenderPlan.overlays`. Tests: the exact text of the overlay filters, the rest of the graph byte-identical to a project without overlays (the existing golden stays), NTSC time base, even widths, input indices after the cards.
4. **`crates/scenes`** (pure, then process): chart spec and validation, canonical text and id, number formatting, axis range; the fixed scene template and the single-literal embedding; `SceneRenderer`, the argument builders, `resolve_worker`, `ManimgxCli`. Tests: every bound, hostile text, the documented flags only, worker scripts for success, exit 1, other exits, time-out, missing output, a long log, no stdin.
5. **`reelctl overlay add-chart|remove`** and the `render` input for overlays. Tests with a fake `manimgx` script: documented argument lines, one overlay and the same bytes on a second run, a lost asset redrawn, pixel checks on the frames around the span at 30 and 29.97 fps, three positions, remove, overlap, order, missing worker, URL paths, bad specs, inspect refusal, failing and wrong-looking workers, time-out, hostile text, the API key not passed on.
6. **Docs and the mutation pass.** README, status, cli, timeline, CHANGELOG, AGENTS, p0-capability-map, `otio_check.py` (two video tracks), the by-hand recipe for the real tool. `cargo mutants` over the pure logic; survivors listed honestly in status.

## Commits

1. Spec, plan, ADR and the timeline crate (steps 1 to 3) with `docs/timeline.md`.
2. The `scenes` crate (step 4).
3. The command, its tests and the rest of the docs (steps 5 and 6).

## Checks before the end

`cargo fmt --all`; `cargo check --workspace --all-targets` with no new warning; `cargo test --workspace` once; the `audit` subagent on the committed branch.

## Risks

- The real worker may disagree with a documented behaviour (A1 to A7). Mitigation: every output is checked before it is used. Not mitigated: how good the keyed edges look.
- Shared files (`main.rs`, `Cargo.toml`, README, status, CHANGELOG, AGENTS) are edited additively and minimally; the other slices will touch the same ones, so expect merge conflicts there.
