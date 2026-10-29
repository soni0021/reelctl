import AppKit
import ReelCore
import SwiftUI
import UniformTypeIdentifiers

/// Settings and Doctor (spec screen 13).
struct SettingsScreen: View {
    @Environment(AppModel.self) private var model
    @AppStorage("captionPreset") private var captionPreset = "bold"
    @State private var heavy = 1
    @State private var light = 4
    @State private var network = 1
    @State private var limitsLoaded = false
    @State private var limitsNote: String?
    @State private var restarting = false
    @State private var restartNote: String?

    static let captionPresets = ["bold", "clean", "boxed", "karaoke", "minimal"]
    static let releasesURL = "https://github.com/soni0021/reelctl/releases"

    var body: some View {
        Form {
            DoctorSection()
            toolsSection
            APIKeySection()
            workspaceSection
            limitsSection
            SwiftUI.Section("Captions") {
                Picker("Default caption preset", selection: $captionPreset) {
                    ForEach(Self.captionPresets, id: \.self) { preset in
                        Text(preset.capitalized).tag(preset)
                    }
                }
                Text("New caption forms in Projects and Make reels start from this preset.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            updatesSection
        }
        .formStyle(.grouped)
        .navigationTitle("Settings")
        .onAppear(perform: loadLimits)
        .onChange(of: model.info?.limits) { _, _ in
            if !limitsLoaded { loadLimits() }
        }
    }

    // MARK: Tool paths

    private var toolsSection: some View {
        SwiftUI.Section {
            ForEach(ToolSetting.allCases) { tool in
                ToolPathRow(tool: tool)
            }
            HStack {
                Button(restarting ? "Restarting…" : "Apply (restart the service)") {
                    Task { await restart() }
                }
                .disabled(restarting)
                if let restartNote {
                    Text(restartNote).font(.caption).foregroundStyle(.secondary)
                }
            }
        } header: {
            Text("Tool paths")
        } footer: {
            Text("Paths only, passed to reelctl serve as environment variables. The service restarts with them when no job is running.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    private func restart() async {
        restarting = true
        restartNote = nil
        let done = await model.restartService()
        restarting = false
        restartNote = done ? "Restarted with the new paths." : "Not restarted; see the banner."
    }

    // MARK: Workspace

    private var workspaceSection: some View {
        SwiftUI.Section("Workspace") {
            LabeledContent("Folder") {
                Text(model.workspace.path).textSelection(.enabled).lineLimit(2)
            }
            if let info = model.info {
                FieldRow(label: "Service", value: "reelctl \(info.version), schema \(info.schemaVersion), pid \(info.pid)")
            }
            HStack {
                Button("Choose…") {
                    guard let url = chooseFolder(title: "Choose the ReelStudio workspace") else { return }
                    model.workspace = url
                    Task { await model.connect() }
                }
                Button("Reveal in Finder") { serviceReveal([model.workspace]) }
                Button("Reconnect") { Task { await model.connect() } }
            }
            Text("The service stores its catalogue and outputs in .reelstudio inside this folder. Videos are imported by reference, not copied.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    // MARK: Queue limits

    private var limitsSection: some View {
        SwiftUI.Section("Queue limits") {
            Stepper("Heavy jobs at once: \(heavy)", value: $heavy, in: 1...16)
            Stepper("Light jobs at once: \(light)", value: $light, in: 1...32)
            Stepper("Network jobs at once: \(network)", value: $network, in: 1...8)
            HStack {
                Button("Apply limits") { Task { await applyLimits() } }
                    .disabled(!model.isConnected)
                if let limitsNote {
                    Text(limitsNote).font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }

    private func loadLimits() {
        guard let limits = model.info?.limits else { return }
        heavy = max(1, limits.heavy)
        light = max(1, limits.light)
        network = max(1, limits.network)
        limitsLoaded = true
    }

    private func applyLimits() async {
        let h = max(1, heavy)
        let l = max(1, light)
        let n = max(1, network)
        let limits: Limits? = await model.perform { try await $0.setLimits(heavy: h, light: l, network: n) }
        if let limits {
            heavy = limits.heavy
            light = limits.light
            network = limits.network
            limitsNote = "Applied: heavy \(limits.heavy), light \(limits.light), network \(limits.network)."
            await model.refresh()
        }
    }

    // MARK: Updates

    private var updatesSection: some View {
        SwiftUI.Section("Updates") {
            Button("Check for updates…") {
                if let url = URL(string: Self.releasesURL) { NSWorkspace.shared.open(url) }
            }
            Text("Opens the Releases page in your browser. The repository is private, so the app does not query GitHub's API (that needs a token); it checks only when you click, and sends nothing.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }
}

/// One tool path: a field, Choose…, and Clear. Saved to UserDefaults as typed.
struct ToolPathRow: View {
    let tool: ToolSetting
    @State private var text: String

    init(tool: ToolSetting) {
        self.tool = tool
        _text = State(initialValue: tool.value)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text(tool.title)
                Spacer()
                Text(tool.rawValue).font(.caption.monospaced()).foregroundStyle(.secondary)
            }
            HStack {
                TextField("Path", text: $text, prompt: Text("Not set"))
                    .textFieldStyle(.roundedBorder)
                    .font(.callout.monospaced())
                    .onSubmit { tool.value = text }
                    .onChange(of: text) { _, value in tool.value = value }
                Button("Choose…") {
                    if let url = chooseFile([.item], title: "Choose \(tool.title)") {
                        text = url.path
                        tool.value = url.path
                    }
                }
                Button("Clear") {
                    text = ""
                    tool.value = ""
                }
                .disabled(text.isEmpty)
            }
            if !text.isEmpty, !text.hasPrefix("/") {
                Text("Use an absolute path.").font(.caption).foregroundStyle(.red)
            } else if !text.isEmpty, !FileManager.default.fileExists(atPath: text) {
                Text("Nothing is at this path.").font(.caption).foregroundStyle(.orange)
            }
        }
        .padding(.vertical, 2)
    }
}
