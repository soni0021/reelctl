# Plan: audit fixes for `transnet-cuts`

`tasks/plan.md` belongs to another feature, so this plan lives beside the spec ([transnet-cuts](transnet-cuts.md)). It covers the findings of the audit of the first implementation. Each task was test-first (a failing test, then the fix), or, for behavior that was already right but unguarded, a characterization test whose red was shown by a hand-made mutant.

- [x] Probabilities outside 0 to 1 or not a number are an error naming the value and frame (`predict`). Red: NaN in a run, infinity, 1e30, an all-NaN output.
- [x] Bound memory: `ModelFrames` caps its up-front reservation and keeps at most 200,000 frames; the decode loop's frame and PCM reservations are capped; over the limit the detector fails before the model runs; a long run adds a note. Red: a huge expected count, frames past the limit, the limit error, the note.
- [x] Make the model injectable (`WindowModel`, `decompile_selected`) and test the pipeline offline: frames reach the model, `transnet` never falls back, `auto` falls back with the reason, the limit reaches the decode loop.
- [x] Pin the sampling positions, the cell rounding and the scene selection (survivors of the first mutation run).
- [x] A directory as weights says so; `ORT_DYLIB_PATH` must be absolute. Red first.
- [x] `generate.sh` usage lists `dissolve12`; `docs/status.md` and `docs/cli.md` corrected (test counts, the `as_str` wording, limits).
- [x] Real-model run on `textured_cuts12`, `draft12`, `dissolve12`: cuts unchanged (72, 144, 216 / 180 / 110, 208).
