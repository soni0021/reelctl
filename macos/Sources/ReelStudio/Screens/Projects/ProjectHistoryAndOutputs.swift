import AppKit
import AVKit
import ReelCore
import SwiftUI

/// The project's checkpoints, compactly: label, cached, time, and "Revert to here".
struct ProjectCheckpointList: View {
    @Environment(AppModel.self) private var model
    let editor: ProjectEditor

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("History").font(.headline)
            if editor.checkpoints.isEmpty {
                Text("No checkpoints yet.").font(.callout).foregroundStyle(.secondary)
            }
            ForEach(editor.checkpoints) { checkpoint in
                row(checkpoint)
                Divider()
            }
        }
    }

    private func row(_ checkpoint: Checkpoint) -> some View {
        let isHead = checkpoint.id == editor.head?.id
        let isShown = checkpoint.id == editor.current?.id
        return HStack(spacing: 6) {
            Image(systemName: isHead ? "largecircle.fill.circle" : "circle")
                .font(.caption2)
                .foregroundStyle(isHead ? Color.accentColor : Color.secondary)
            Text("#\(checkpoint.id)").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            Text(checkpoint.label)
                .font(.callout.monospaced())
                .fontWeight(isShown ? .semibold : .regular)
                .lineLimit(1)
                .help(checkpoint.note.isEmpty ? checkpoint.label : "\(checkpoint.label)\n\(checkpoint.note)")
            if checkpoint.cached {
                Text("cached")
                    .font(.caption2)
                    .padding(.horizontal, 4)
                    .background(Color.blue.opacity(0.15), in: Capsule())
            }
            Spacer(minLength: 4)
            Text(Self.time(checkpoint.createdAt)).font(.caption).foregroundStyle(.secondary)
            Button("Revert to here") {
                Task { await editor.revert(to: checkpoint.id, model) }
            }
            .buttonStyle(.borderless)
            .font(.caption)
            .disabled(isHead)
        }
    }

    static func time(_ ms: Int64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
        return date.formatted(date: .abbreviated, time: .shortened)
    }
}

/// The latest render, playing in the app.
struct ProjectRenderPane: View {
    let editor: ProjectEditor

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Render").font(.headline)
                if let render = editor.render {
                    Text("checkpoint #\(render.checkpoint)").font(.caption).foregroundStyle(.secondary)
                    if render.stale {
                        Label("older than the edit shown", systemImage: "clock.badge.exclamationmark")
                            .font(.caption)
                            .foregroundStyle(.orange)
                    }
                    Spacer()
                    Button("Show in Finder") {
                        NSWorkspace.shared.activateFileViewerSelecting([render.url])
                    }
                    .buttonStyle(.borderless)
                    .font(.caption)
                }
            }
            if editor.render != nil {
                PlayerView(player: editor.player)
                    .frame(minHeight: 260, idealHeight: 360)
                    .clipShape(RoundedRectangle(cornerRadius: 6))
            } else {
                Text("Not rendered yet. Render (on the toolbar) writes an mp4 that plays here.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 80)
                    .background(Color.secondary.opacity(0.06), in: RoundedRectangle(cornerRadius: 6))
            }
        }
    }
}

/// The QA run's pictures, one per join.
struct ProjectQAGallery: View {
    let editor: ProjectEditor
    @State private var enlarged: ProjectQAImage?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("Joins").font(.headline)
                if let checkpoint = editor.galleryCheckpoint {
                    Text("\(editor.gallery.count) views · checkpoint #\(checkpoint)").font(.caption).foregroundStyle(.secondary)
                }
            }
            if editor.gallery.isEmpty {
                Text(editor.galleryCheckpoint == nil ? "Inspect joins (on the toolbar) draws each join's frames, waveform and words." : "The QA run has no pictures (the project has no joins).")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } else {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 220, maximum: 360), spacing: 10)], alignment: .leading, spacing: 10) {
                    ForEach(editor.gallery) { image in
                        Button {
                            enlarged = image
                        } label: {
                            ProjectQAThumbnail(image: image)
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .sheet(item: $enlarged) { image in
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text(image.name).font(.headline)
                    Spacer()
                    Button("Done") { enlarged = nil }.keyboardShortcut(.defaultAction)
                }
                ScrollView([.horizontal, .vertical]) {
                    ProjectQAPicture(url: image.url)
                }
            }
            .padding(16)
            .frame(minWidth: 700, minHeight: 500)
        }
    }
}

struct ProjectQAThumbnail: View {
    let image: ProjectQAImage

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            ProjectQAPicture(url: image.url)
                .frame(maxWidth: .infinity, minHeight: 90, maxHeight: 160)
                .clipShape(RoundedRectangle(cornerRadius: 4))
            Text(image.name).font(.caption).lineLimit(1).foregroundStyle(.secondary)
        }
    }
}

/// A stored picture (no extension, so read as data).
struct ProjectQAPicture: View {
    let url: URL
    @State private var picture: NSImage?

    var body: some View {
        Group {
            if let picture {
                Image(nsImage: picture)
                    .resizable()
                    .interpolation(.high)
                    .aspectRatio(contentMode: .fit)
            } else {
                Rectangle().fill(Color.secondary.opacity(0.1))
                    .overlay { ProgressView().controlSize(.small) }
            }
        }
        .task(id: url) {
            let data = try? Data(contentsOf: url)
            picture = data.flatMap { NSImage(data: $0) }
        }
    }
}
