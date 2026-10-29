# Shared-code needs (Make reels)

Nothing outside this folder was changed.

1. **Stored videos have no extension.** The card plays the rendered reel
   through a symbolic link `$TMPDIR/ReelStudio-reels/<hash>.mp4` to the
   read-only object (`MakeReelsReelCard.playableURL`), as the Service screens
   do. A shared helper in `Shared/Components.swift` (or a media type from the
   service) should replace both.
2. **The run is in a shared object, not `AppModel`.** `MakeReelsRun.shared`
   survives leaving the screen, but not a relaunch: after a restart the reels'
   jobs are still in Jobs and the `make_reels` subject, but this screen starts
   empty. Restoring it would need the service to report a `make_reels`
   subject's stage (or the app to store the request).
3. **Whisper and weights paths.** The request's `whisper_model` and `weights`
   come from Settings (`ToolSetting.whisperModel`, `ToolSetting.rvmWeights`),
   because the catalogue passes them as arguments, not environment.
4. **`pipelines.make_reels` notes are run-level.** `MakeReelsStage.notes` is
   shown above the cards; per-reel notes (for example which reel fell back to
   the centre crop) would need a `notes` field on `ReelJobs`.
5. **`README.md` and `NOTES.md`** need `exclude:` in `Package.swift` for a
   warning-free `swift build` (see Overlays/NOTES.md).
6. **Not compiled.** Written without a Swift toolchain; compile on the Mac.
