import ReelCore
import SwiftUI

/// The first-launch sheet: the workspace, Doctor (with `brew install ffmpeg`
/// when it is missing), and how to open an unnotarized app.
struct OnboardingView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var step: OnboardingStep = .workspace
    @State private var folder: URL?
    @State private var doctor: [String: DoctorItem]?
    @State private var checking = false
    @State private var copied = false

    static let brewCommand = "brew install ffmpeg"

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text("Welcome to ReelStudio").font(.largeTitle.bold())
                Spacer()
                Text("Step \(step.rawValue + 1) of \(OnboardingStep.allCases.count)")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Divider()
            Group {
                switch step {
                case .workspace: workspaceStep
                case .doctor: doctorStep
                case .unnotarized: unnotarizedStep
                case .done: doneStep
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            Divider()
            HStack {
                if step != .workspace {
                    Button("Back") { go(-1) }
                }
                Spacer()
                if step == .done {
                    Button("Done", action: finish).keyboardShortcut(.defaultAction)
                } else {
                    Button("Skip setup", action: finish)
                    Button("Continue") { Task { await next() } }
                        .keyboardShortcut(.defaultAction)
                }
            }
        }
        .padding(24)
        .frame(width: 560, height: 440)
        .onAppear { if folder == nil { folder = model.workspace } }
    }

    // MARK: Steps

    private var workspaceStep: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Choose a workspace").font(.title2.bold())
            Text("ReelStudio keeps its catalogue, job history and outputs in a .reelstudio folder inside the workspace. Your videos stay where they are; they are imported by reference.")
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Image(systemName: "folder")
                Text((folder ?? model.workspace).path)
                    .font(.callout.monospaced())
                    .textSelection(.enabled)
                    .lineLimit(2)
                Spacer()
                Button("Choose…") {
                    if let url = chooseFolder(title: "Choose the ReelStudio workspace") { folder = url }
                }
            }
            .padding(10)
            .background(Color.secondary.opacity(0.1), in: RoundedRectangle(cornerRadius: 8))
            Text("The default is ~/Movies/ReelStudio. You can change it later in Settings.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private var doctorStep: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Check the tools").font(.title2.bold())
            if checking {
                ProgressView("Running Doctor…")
            } else if let doctor {
                ForEach(["ffmpeg", "ffprobe"], id: \.self) { key in
                    if let item = doctor[key] {
                        DoctorRow(name: key, item: item)
                    } else {
                        HStack(spacing: 8) {
                            Image(systemName: "xmark.circle.fill").foregroundStyle(Color.red)
                            Text(key)
                            Text("not reported").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                if ffmpegMissing {
                    Text("ReelStudio needs ffmpeg and ffprobe. Install them with Homebrew, then check again:")
                        .fixedSize(horizontal: false, vertical: true)
                    HStack {
                        Text(Self.brewCommand)
                            .font(.body.monospaced())
                            .textSelection(.enabled)
                            .padding(8)
                            .background(Color.secondary.opacity(0.1), in: RoundedRectangle(cornerRadius: 6))
                        Button(copied ? "Copied" : "Copy") {
                            serviceCopy(Self.brewCommand)
                            copied = true
                        }
                    }
                } else {
                    Label("ffmpeg is ready.", systemImage: "checkmark.seal").foregroundStyle(.green)
                }
                Text("Optional tools (manimgx, Smelter, ONNX Runtime and the model files) can be set later in Settings.")
                    .font(.caption).foregroundStyle(.secondary)
            } else {
                connectionText
            }
            Button("Check again") { Task { await runDoctor() } }
                .disabled(checking)
        }
    }

    @ViewBuilder
    private var connectionText: some View {
        switch model.connection {
        case .failed(let reason):
            Text("Could not reach reelctl serve: \(reason)").foregroundStyle(.red).textSelection(.enabled)
        case .connecting:
            ProgressView("Starting reelctl serve…")
        case .disconnected:
            Text("Not connected yet.").foregroundStyle(.secondary)
        case .connected:
            Text("Doctor has not run yet.").foregroundStyle(.secondary)
        }
    }

    private var unnotarizedStep: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Opening an unnotarized app").font(.title2.bold())
            Text("ReelStudio is signed ad hoc and not notarized by Apple, so macOS blocks a plain double-click the first time.")
                .fixedSize(horizontal: false, vertical: true)
            VStack(alignment: .leading, spacing: 6) {
                Label("In Finder, right-click (or Control-click) ReelStudio in Applications.", systemImage: "1.circle")
                Label("Choose Open from the menu.", systemImage: "2.circle")
                Label("Click Open in the dialog. macOS remembers this, so you do it once.", systemImage: "3.circle")
            }
            Text("If macOS still refuses, open System Settings › Privacy & Security and click Open Anyway next to ReelStudio. A new version you install needs the same once.")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var doneStep: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("You're set").font(.title2.bold())
            Text("Import a reel in Library, or press ⌘K to jump anywhere. Jobs shows the queue, and History lets you revert or branch any step.")
                .fixedSize(horizontal: false, vertical: true)
            if ffmpegMissing {
                Label("ffmpeg is still missing; decompile and render will fail until it is installed.", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
            }
        }
    }

    // MARK: Flow

    private var ffmpegMissing: Bool {
        guard let doctor else { return false }
        return doctor["ffmpeg"]?.found != true || doctor["ffprobe"]?.found != true
    }

    private func go(_ delta: Int) {
        let index = min(max(step.rawValue + delta, 0), OnboardingStep.allCases.count - 1)
        step = OnboardingStep(rawValue: index) ?? .workspace
    }

    private func next() async {
        switch step {
        case .workspace:
            let chosen = folder ?? model.workspace
            let changed = chosen.standardizedFileURL.path != model.workspace.standardizedFileURL.path
            model.workspace = chosen
            go(1)
            if changed || !model.isConnected { await model.connect() }
            await runDoctor()
        case .doctor, .unnotarized:
            go(1)
        case .done:
            finish()
        }
    }

    private func runDoctor() async {
        guard model.isConnected else { return }
        checking = true
        let found: [String: DoctorItem]? = await model.perform { try await $0.doctor() }
        if let found { doctor = found }
        checking = false
    }

    private func finish() {
        UserDefaults.standard.set(true, forKey: "onboarded")
        model.showOnboarding = false
        dismiss()
    }
}

enum OnboardingStep: Int, CaseIterable {
    case workspace, doctor, unnotarized, done
}
