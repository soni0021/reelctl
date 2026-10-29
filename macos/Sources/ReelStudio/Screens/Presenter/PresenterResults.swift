import AppKit
import AVFoundation
import ReelCore
import SwiftUI

/// What is missing, with a way to Settings and a re-check.
struct PresenterMissingView: View {
    @Environment(AppModel.self) private var model
    let missing: [PresenterMissingTool]
    let recheck: () -> Void

    var body: some View {
        ContentUnavailableView {
            Label("The presenter cut-out needs tools", systemImage: "wrench.and.screwdriver")
        } description: {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(missing) { tool in
                    Text("\(tool.title): \(tool.detail)")
                        .multilineTextAlignment(.leading)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Text("New tool paths apply when the service restarts (when no job runs).")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: 520)
        } actions: {
            Button("Settings") { model.selection = .settings }
            Button("Check again", action: recheck)
        }
    }
}

/// The source and the cut-out side by side.
struct PresenterBeforeAfter: View {
    let before: FramePlayer
    let after: FramePlayer
    let output: StoredFile?
    let mode: String?

    var body: some View {
        VStack(spacing: 8) {
            HStack(spacing: 8) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Before").font(.headline)
                    if before.url == nil {
                        PresenterPlaceholder(text: "Pick a video")
                    } else {
                        PlayerView(player: before.player)
                    }
                }
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text("After").font(.headline)
                        if let mode {
                            Text(mode).font(.caption).foregroundStyle(.secondary)
                        }
                    }
                    if after.url == nil {
                        PresenterPlaceholder(text: "Run the cut-out to see the result")
                    } else {
                        PlayerView(player: after.player)
                    }
                }
            }
            HStack {
                Button("Play both") {
                    before.seek(ms: 0)
                    after.seek(ms: 0)
                    before.player.play()
                    after.player.play()
                }
                .disabled(before.url == nil || after.url == nil)
                Button("Pause") {
                    before.player.pause()
                    after.player.pause()
                }
                Spacer()
                if let output {
                    Text(output.url.lastPathComponent)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                    Button("Reveal in Finder") {
                        NSWorkspace.shared.activateFileViewerSelecting([output.url])
                    }
                }
            }
        }
    }
}

struct PresenterPlaceholder: View {
    let text: String

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 6).fill(Color.secondary.opacity(0.12))
            Text(text).foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// The frame count of a running cut-out, from its progress events.
struct PresenterFrameCount: View {
    @Environment(AppModel.self) private var model
    let job: JobID

    var body: some View {
        if let progress = model.progress[job] {
            if let total = progress.total {
                Text(verbatim: "\(progress.stage): frame \(progress.done) of \(total)")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            } else {
                Text(verbatim: "\(progress.stage): frame \(progress.done)")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
    }
}
