# ADR-025: Reframe to 9:16 on the presenter from the RobustVideoMatting alpha centroid

## Status
Accepted (with [make-reels](../specs/make-reels.md), 2026-10-29)

## Date
2026-10-29

## Context
`make-reels` turns a 16:9 lecture into 9:16 reels. A fixed centre crop cuts off a presenter who stands to one side or walks across a board. The user chose "follow the presenter".

The repo already runs RobustVideoMatting on the CPU through `ort` ([ADR-021](ADR-021-matte-presenters-with-rvm-through-a-second-onnx-session.md)), with a fake `MattingModel` for tests. It costs about 80–180 ms per 720p frame under load at the default ratio. The other ways to find the presenter are:
- a face detector, which adds a model and identifies faces, something the project does not do
- saliency, which follows board text and pop-ups
- a person detector, which adds a model

## Decision
- **Find the presenter** with the RVM alpha. Take its alpha-weighted horizontal centre of mass per frame, ignoring pixels under alpha 0.5. The matte's mass is what is followed; no face or identity is involved.
- **Measure** at a low downsample ratio (0.25 by default) and keep only the alpha. Nothing is written but the plan.
- **Smooth in pure code**, with a dead zone, a maximum pan speed, an ease, jumps at the DNA's cuts, and clamping to the frame edges. Store at most one keyframe every 500 ms, plus one per cut, in `.otio` metadata. `render` turns them into a piecewise-linear `crop` x expression.
- **Fail closed.** `transform reframe --follow presenter` without a model exits 2. Only `make-reels` falls back to a centre crop, and it says so in `make_reels.json`.
- **One ONNX model per process**, in line with AGENTS.md: `reframe` loads only RVM.

## Consequences
- Reframing a 60 s clip costs a full RVM pass. At 0.25 that is expected to be well under the 80–180 ms per frame measured at the default ratio, but it is unmeasured until the first manual run on the Mac.
- Two people, or a presenter beside a large moving graphic, pull the centroid between them. The plan cannot tell them apart, and the manual check will show how often it matters.
- Tests in the cloud cover the planning and the render expression with synthetic alpha frames. Real-model quality is a manual check on the Mac.
- Only x moves, so a vertical source, or a crop that needs a vertical move, is out of scope.
