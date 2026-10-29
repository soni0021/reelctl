import ReelCore
import SwiftUI

/// `checkpoints.diff` of two checkpoints, and a changed role opened side by
/// side: two players for videos, two texts for JSON and other text.
struct CheckpointCompareSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let comparison: HistoryComparison

    @State private var role: String?
    @State private var pair: ComparedPair?
    @State private var loading = false
    @State private var left: FramePlayer?
    @State private var right: FramePlayer?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Checkpoint #\(comparison.a) against #\(comparison.b)").font(.title2.bold())
                Spacer()
                Button("Done") { dismiss() }.keyboardShortcut(.cancelAction)
            }
            HStack(alignment: .top, spacing: 16) {
                roleList("Changed", comparison.diff.changed, color: .orange, openable: true)
                roleList("Same", comparison.diff.same, color: .green, openable: false)
                roleList("Only in #\(comparison.a)", comparison.diff.onlyInA, color: .blue, openable: false)
                roleList("Only in #\(comparison.b)", comparison.diff.onlyInB, color: .purple, openable: false)
            }
            .frame(maxHeight: 160)
            Divider()
            detail
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .padding(20)
        .frame(minWidth: 900, idealWidth: 1100, minHeight: 600, idealHeight: 720)
        .onDisappear {
            left?.player.pause()
            right?.player.pause()
        }
    }

    private func roleList(_ title: String, _ roles: [String], color: Color, openable: Bool) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title).font(.headline)
            if roles.isEmpty {
                Text("None").foregroundStyle(.secondary)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(roles.sorted(), id: \.self) { name in
                        HStack {
                            Circle().fill(color).frame(width: 7, height: 7)
                            Text(name).font(.callout.monospaced())
                            if openable {
                                Button(role == name ? "Shown" : "Open both") {
                                    Task { await open(name) }
                                }
                                .buttonStyle(.link)
                                .disabled(loading)
                            }
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private var detail: some View {
        if loading {
            ProgressView("Opening…").frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let pair {
            switch pair.content {
            case .videos:
                HStack(spacing: 12) {
                    side("#\(comparison.a)", file: pair.a) {
                        if let left { PlayerView(player: left.player) }
                    }
                    side("#\(comparison.b)", file: pair.b) {
                        if let right { PlayerView(player: right.player) }
                    }
                }
            case .texts(let a, let b):
                HStack(spacing: 12) {
                    side("#\(comparison.a)", file: pair.a) { textPane(a) }
                    side("#\(comparison.b)", file: pair.b) { textPane(b) }
                }
            case .unsupported(let reason):
                VStack(spacing: 10) {
                    Text(reason).foregroundStyle(.secondary)
                    Button("Reveal both in Finder") { serviceReveal([pair.a.url, pair.b.url]) }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        } else if comparison.diff.changed.isEmpty {
            Text("No output changed between these checkpoints.")
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            Text("Choose a changed output to open both versions side by side.")
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func side<Content: View>(_ title: String, file: StoredFile, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(title).font(.headline)
                Text(String(file.hash.prefix(12))).font(.caption.monospaced()).foregroundStyle(.secondary)
                Spacer()
                Button("Reveal") { serviceReveal([file.url]) }.buttonStyle(.link)
            }
            content()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    private func textPane(_ text: String) -> some View {
        ScrollView([.vertical, .horizontal]) {
            Text(text)
                .font(.caption.monospaced())
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .padding(8)
        }
        .background(Color(nsColor: .textBackgroundColor))
        .border(Color.secondary.opacity(0.3))
    }

    private func open(_ name: String) async {
        loading = true
        defer { loading = false }
        role = name
        left?.player.pause()
        right?.player.pause()
        let a = comparison.a
        let b = comparison.b
        let found: (StoredFile?, StoredFile?)? = await model.perform { (studio: Studio) -> (StoredFile?, StoredFile?) in
            let first = try await studio.checkpoint(a)
            let second = try await studio.checkpoint(b)
            return (first.files?[name], second.files?[name])
        }
        guard let found, let fileA = found.0, let fileB = found.1 else {
            pair = nil
            model.banner = "Output \(name) has no stored file in one of the checkpoints."
            return
        }
        let kindA = ServiceObjectKind.of(fileA)
        let kindB = ServiceObjectKind.of(fileB)
        switch (kindA, kindB) {
        case (.video(let qtA), .video(let qtB)):
            let playerA = left ?? FramePlayer()
            let playerB = right ?? FramePlayer()
            playerA.load(ServiceObjectKind.playableURL(for: fileA, qt: qtA))
            playerB.load(ServiceObjectKind.playableURL(for: fileB, qt: qtB))
            left = playerA
            right = playerB
            pair = ComparedPair(a: fileA, b: fileB, content: .videos)
        case (.text, .text):
            pair = ComparedPair(a: fileA, b: fileB, content: .texts(ServiceObjectKind.readText(fileA), ServiceObjectKind.readText(fileB)))
        case (.folder, .folder):
            pair = ComparedPair(a: fileA, b: fileB, content: .unsupported("Both are folders; open them in Finder to compare."))
        default:
            pair = ComparedPair(a: fileA, b: fileB, content: .unsupported("These outputs are not video or text; open them in Finder to compare."))
        }
    }
}

/// One changed role, opened.
struct ComparedPair {
    enum Content {
        case videos
        case texts(String, String)
        case unsupported(String)
    }

    let a: StoredFile
    let b: StoredFile
    let content: Content
}
