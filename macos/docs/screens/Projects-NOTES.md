# Shared-code notes from Projects

Changes outside this folder that the editor would benefit from. None of them is needed for it to build.

1. **`render` and `qa.inspect-boundaries` move a project's head off its `.otio`.** These jobs run on the project subject, as the spec says. Their checkpoint becomes the head, and its `out` is then the mp4, or there is no `out` at all for QA. The editor resolves the `.otio` itself (see README.md). History, Compare and any other screen that reads "head `out`" of a project as an `.otio` will get the mp4 after a render. There are two possible fixes:
   - The service could carry the parent's other outputs forward.
   - Rendering could happen on a child subject.

   `pipelines.make_reels` has the same shape: after its `diff` step, a reel subject's head `out` is missing and `report` is the diff.
2. **`FramePlayer.load` cannot always open store objects.** It uses `AVPlayerItem(url:)`, but store paths (`objects/xx/<hash>`) have no extension, so AVFoundation may refuse the file. The editor plays renders through its own `AVPlayer`, with `AVURLAsset(url:options: [AVURLAssetOverrideMIMETypeKey: "video/mp4"])`. A `FramePlayer.load(_:mimeType:)` would let the other screens do the same.
3. **`AppModel.perform` keeps the error to itself.** It reports the error and returns nil, so a form cannot see which field is at fault. The editor works around this by capturing the error inside the closure. Either of these would let every screen highlight a field the same way:
   - a `perform` that returns `Result`;
   - a `lastError` (or `badField`) on `AppModel`.
4. **The argument lists are copied by hand.** `ProjectCatalogue.swift` copies the argument keys and kinds from `crates/studio/src/commands.rs`, and it must be kept in step with that file. A `jobs.catalogue` RPC method that returns `COMMANDS` (key, kind, choices, required, input roles) would let the forms be built from the service.
5. **The caption presets' values are not available.** `overlay captions --list-presets` is not in the job catalogue, so the form shows preset names but not what each one sets.
6. **Banner wording.** `AppModel.report` writes "Check the highlighted field" for exit code 2. The editor uses the same words when a job (not the submit) fails with exit code 2.
