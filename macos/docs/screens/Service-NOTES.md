# Shared-code needs (Service screens)

Requests for code outside this folder. Nothing here was changed outside it.

1. **Store objects have no file extension.** AVFoundation picks a format from
   the extension, so `FramePlayer.load(objectURL)` may not play a stored video.
   `CheckpointCompareSheet` works around it with a symbolic link
   `$TMPDIR/ReelStudio-compare/<hash>.mp4` (or `.mov`) to the read-only object,
   chosen by sniffing the `ftyp` box (`ServiceObjectKind`). Other screens that
   play stored outputs need the same; better would be `objects.path` (or
   `checkpoints.get`'s `files`) returning the object's `media_type`, and a
   `FramePlayer.load(_:mimeType:)` in Components using
   `AVURLAssetOverrideMIMETypeKey`, or the helper moved to Shared.
2. **Switching workspace.** Settings and Onboarding set `model.workspace` and
   call `model.connect()`. The old clients are released (their `deinit` closes
   the sockets) and the old workspace's service keeps running, as the spec's
   stopping rule allows. An `AppModel.switchWorkspace(_:)` that sends
   `shutdown` to the old service when nothing runs there, then connects, would
   be cleaner.
3. **"cached" in the job list.** `Job.checkpoint` (and so `cached`) comes only
   from `jobs.get`, so a row shows the badge only after `AppModel.reloadJob`
   (which runs when a job finishes while the app is connected). `jobs.list`
   returning `cached` (or the checkpoint id) would show it for older jobs too.
4. **Import video from the palette.** It only switches to Library. A flag such
   as `AppModel.requestImport` that LibraryScreen observes to open its panel
   would make the action complete.
5. **The caption preset.** Settings stores `captionPreset` in UserDefaults
   (`bold`, `clean`, `boxed`, `karaoke`, `minimal`; default `bold`). The
   caption forms in Projects and Make reels should read it as their default.
6. **Revert and branch notifications.** History reloads itself after a revert
   or a branch, and on `model.checkpointTick`. If the service does not push
   `checkpoint.created` for those, other screens showing the head (Projects)
   should also call `model.refreshSubjects()` or watch `checkpointTick`.
7. **Retry of `understand`.** The Retry button is disabled for any command
   starting with `understand`, since the service refuses a live one; the plan
   job could be retried, but planning again from Understand is clearer.
