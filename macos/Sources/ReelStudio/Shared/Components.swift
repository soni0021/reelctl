import AppKit
import AVFoundation
import AVKit
import ReelCore
import SwiftUI
import UniformTypeIdentifiers

// Pieces every screen uses: file pickers, the asset picker, job progress, the
// frame-exact player, the paid-call sheet, and formatting.

/// `12.345 s`, or `1:02.3`.
func formatMs(_ ms: Int64) -> String {
    let seconds = Double(ms) / 1000
    if seconds < 60 { return String(format: "%.3f s", seconds) }
    let minutes = Int(seconds) / 60
    return String(format: "%d:%04.1f", minutes, seconds - Double(minutes * 60))
}

func formatBytes(_ bytes: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file)
}

func formatUsd(_ usd: Double) -> String { String(format: "$%.4f", usd) }

/// Open panel for one file of the given types.
@MainActor
func chooseFile(_ types: [UTType], title: String = "Choose") -> URL? {
    let panel = NSOpenPanel()
    panel.title = title
    panel.allowedContentTypes = types
    panel.allowsMultipleSelection = false
    panel.canChooseDirectories = false
    return panel.runModal() == .OK ? panel.url : nil
}

/// Open panel for a folder.
@MainActor
func chooseFolder(title: String = "Choose a folder") -> URL? {
    let panel = NSOpenPanel()
    panel.title = title
    panel.canChooseFiles = false
    panel.canChooseDirectories = true
    panel.canCreateDirectories = true
    return panel.runModal() == .OK ? panel.url : nil
}

extension UTType {
    static let otio = UTType(filenameExtension: "otio") ?? .json
    static let cube = UTType(filenameExtension: "cube") ?? .data
    static let onnx = UTType(filenameExtension: "onnx") ?? .data
    static let ggml = UTType(filenameExtension: "bin") ?? .data
    static let videoTypes: [UTType] = [.movie, .mpeg4Movie, .quickTimeMovie]
}

/// Pick an imported asset of `kind`, or import a new file.
struct AssetPicker: View {
    @Environment(AppModel.self) private var model
    let title: String
    let kind: String
    let types: [UTType]
    @Binding var selection: AssetID?

    var body: some View {
        HStack {
            Picker(title, selection: $selection) {
                Text("None").tag(AssetID?.none)
                ForEach(model.assets.filter { $0.kind == kind }) { asset in
                    Text(asset.name).tag(AssetID?.some(asset.id))
                }
            }
            Button("Import…") {
                guard let url = chooseFile(types, title: title) else { return }
                Task {
                    if let asset = await model.importFile(url, kind: kind) { selection = asset.id }
                }
            }
        }
    }
}

/// A drop target that imports dropped files of `kind`.
struct DropImport: ViewModifier {
    @Environment(AppModel.self) private var model
    let kind: String
    let imported: (Asset) -> Void
    @State private var targeted = false

    func body(content: Content) -> some View {
        content
            .overlay {
                if targeted {
                    RoundedRectangle(cornerRadius: 8).stroke(.tint, lineWidth: 3).padding(4)
                }
            }
            .onDrop(of: [.fileURL], isTargeted: $targeted) { providers in
                for provider in providers {
                    _ = provider.loadObject(ofClass: URL.self) { url, _ in
                        guard let url else { return }
                        Task { @MainActor in
                            if let asset = await model.importFile(url, kind: kind) { imported(asset) }
                        }
                    }
                }
                return true
            }
    }
}

extension View {
    func dropImport(kind: String, imported: @escaping (Asset) -> Void) -> some View {
        modifier(DropImport(kind: kind, imported: imported))
    }
}

/// One job's state: status, stage, a progress bar, and cancel.
struct JobProgressView: View {
    @Environment(AppModel.self) private var model
    let job: JobID

    var body: some View {
        let current = model.job(job)
        HStack(spacing: 8) {
            StatusDot(status: current?.status ?? .queued)
            Text(current?.shortCommand ?? "job \(job)").font(.callout.monospaced())
            if let progress = model.progress[job] {
                if let fraction = progress.fraction {
                    ProgressView(value: fraction).frame(maxWidth: 200)
                } else {
                    ProgressView().controlSize(.small)
                }
                Text(progress.stage).font(.caption).foregroundStyle(.secondary)
            } else {
                Text(current?.status.rawValue ?? "queued").font(.caption).foregroundStyle(.secondary)
            }
            if current?.checkpoint?.cached == true {
                Text("cached").font(.caption2).padding(.horizontal, 4).background(.blue.opacity(0.15), in: Capsule())
            }
            if let status = current?.status, !status.finished {
                Button("Cancel") { Task { await model.perform { try await $0.cancel(job) } } }
                    .buttonStyle(.borderless)
            }
            if let error = current?.error, current?.status == .failed {
                Text(error).font(.caption).foregroundStyle(.red).lineLimit(2).help(error)
            }
        }
    }
}

struct StatusDot: View {
    let status: JobStatus

    var body: some View {
        Circle().fill(color).frame(width: 9, height: 9).help(status.rawValue)
    }

    var color: Color {
        switch status {
        case .queued, .blocked: return .gray
        case .running: return .blue
        case .succeeded: return .green
        case .failed: return .red
        case .cancelled, .interrupted: return .orange
        }
    }
}

/// An AVPlayer that seeks frame-exactly and scrubs with the chase-time pattern
/// (one seek in flight; the latest target kept).
@MainActor
@Observable
final class FramePlayer {
    let player = AVPlayer()
    private(set) var url: URL?
    var currentMs: Int64 = 0
    private var seeking = false
    private var target: CMTime?
    private var observer: Any?

    init() {
        observer = player.addPeriodicTimeObserver(forInterval: CMTime(value: 1, timescale: 30), queue: .main) { [weak self] time in
            MainActor.assumeIsolated {
                self?.currentMs = Int64(time.seconds * 1000)
            }
        }
    }

    func load(_ url: URL?) {
        guard url != self.url else { return }
        self.url = url
        // Stored outputs have no extension; play them through a typed link.
        player.replaceCurrentItem(with: url.map { AVPlayerItem(url: Playable.url(for: $0)) })
    }

    /// Seek to exactly `ms` (tolerance zero).
    func seek(ms: Int64) {
        let time = CMTime(value: ms, timescale: 1000)
        target = time
        if !seeking { chase() }
    }

    private func chase() {
        guard let target else { return }
        seeking = true
        self.target = nil
        player.seek(to: target, toleranceBefore: .zero, toleranceAfter: .zero) { [weak self] _ in
            Task { @MainActor in
                guard let self else { return }
                self.seeking = false
                if self.target != nil { self.chase() }
            }
        }
    }
}

struct PlayerView: NSViewRepresentable {
    let player: AVPlayer

    func makeNSView(context: Context) -> AVPlayerView {
        let view = AVPlayerView()
        view.player = player
        view.controlsStyle = .floating
        view.showsFrameSteppingButtons = true
        return view
    }

    func updateNSView(_ view: AVPlayerView, context: Context) {
        if view.player !== player { view.player = player }
    }
}

/// The paid-call sheet: shows the bound and what leaves the Mac, and returns a
/// yes only for exactly that amount (ADR-031).
struct PaidCallSheet: View {
    let plan: UnderstandPlan
    let model: String
    let approve: () -> Void
    let cancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Spend up to \(plan.approvalText)?").font(.title2.bold())
            Text("This sends \(plan.images) images (\(plan.frames) frames) and the transcript to \(model) through OpenRouter. Data collection is denied on the request.")
                .fixedSize(horizontal: false, vertical: true)
            Text("The amount is the upper bound; the call is refused if it could cost more.")
                .font(.callout).foregroundStyle(.secondary)
            if !KeyStore.isSet {
                Label("No API key is set. Add it in Settings first.", systemImage: "key.slash").foregroundStyle(.red)
            }
            HStack {
                Spacer()
                Button("Cancel", role: .cancel, action: cancel).keyboardShortcut(.cancelAction)
                Button("Spend up to \(plan.approvalText)", action: approve)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!KeyStore.isSet)
            }
        }
        .padding(24)
        .frame(width: 460)
    }
}

/// A labelled row for inspectors.
struct FieldRow: View {
    let label: String
    let value: String

    var body: some View {
        LabeledContent(label) {
            Text(value).textSelection(.enabled).multilineTextAlignment(.trailing)
        }
    }
}

/// A subject picker over one kind, with "New…".
struct SubjectPicker: View {
    @Environment(AppModel.self) private var model
    let kind: String
    @Binding var selection: SubjectID?
    @State private var naming = false
    @State private var name = ""

    var body: some View {
        HStack {
            Picker(kind.replacingOccurrences(of: "_", with: " ").capitalized, selection: $selection) {
                Text("None").tag(SubjectID?.none)
                ForEach(model.subjects(kind: kind)) { subject in
                    Text(subject.name).tag(SubjectID?.some(subject.id))
                }
            }
            Button("New…") { naming = true }
        }
        .alert("New \(kind)", isPresented: $naming) {
            TextField("Name", text: $name)
            Button("Create") {
                let name = self.name.isEmpty ? kind : self.name
                Task {
                    if let subject = await model.perform({ try await $0.createSubject(kind: kind, name: name) }) {
                        await model.refreshSubjects()
                        selection = subject.id
                    }
                }
            }
            Button("Cancel", role: .cancel) {}
        }
    }
}
