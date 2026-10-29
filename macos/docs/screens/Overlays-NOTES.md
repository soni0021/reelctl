# Shared-code needs (Overlays)

Nothing outside this folder was changed.

1. **The chart spec is imported by reference from `$TMPDIR`.** macOS may purge
   `$TMPDIR` after a few days, and a `jobs.retry` of an old `overlay.add-chart`
   would then be refused (the asset changed). `AppModel.importFile` has no
   `copy` parameter; an `importFile(_:kind:copy:)` (passing `copy: true` to
   `assets.import`) would keep the spec in the store.
2. **`README.md` and `NOTES.md` in `Sources/ReelStudio`.** SwiftPM warns about
   unhandled files in a target. `Package.swift` needs
   `exclude: ["Screens/Overlays/README.md", "Screens/Overlays/NOTES.md", ...]`
   (or a glob of every screen's `.md` files) on the `ReelStudio` target for a
   warning-free `swift build`.
3. **Not compiled.** Written without a Swift toolchain; compile on the Mac.
