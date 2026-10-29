# Shared changes the Presenter screen needs

1. **`rgba` mode cannot run through the service.** `crates/studio/src/commands.rs` gives
   `compositor.isolate-presenter` the fixed output `file("out", "--out", "presenter.mp4")`, and
   `reelctl compositor isolate-presenter --mode rgba` requires `--out` to end in `.mov` (exit 2).
   The screen lists RGBA but refuses to submit it and says why. Fix in the service: pick the
   output name from `mode` (`presenter.mov` for `rgba`), or let `jobs.submit` take an output
   extension. Then drop the `.rgba` case from `PresenterOptions.problem`.
2. **A background file is not a job input.** `background` is an `ArgKind::Str`, so an image or
   video background is passed as a path string: it is not hashed into the cache key (replacing
   the file at the same path can give a cached, stale result) and is not recorded in the
   checkpoint. A `maybe("background", "--background")` input role for files, next to the
   colour string, would fix both.
