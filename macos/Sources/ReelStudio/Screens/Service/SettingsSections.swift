import ReelCore
import SwiftUI

/// `doctor.check`, in a fixed order with readable names.
enum DoctorCatalog {
    static let order = [
        "ffmpeg", "ffprobe", "ffmpeg_libraries", "manimgx", "smelter",
        "onnx_runtime", "whisper_model", "transnet_weights", "rvm_weights",
    ]

    static func title(_ key: String) -> String {
        switch key {
        case "ffmpeg": return "ffmpeg"
        case "ffprobe": return "ffprobe"
        case "ffmpeg_libraries": return "FFmpeg libraries (reelctl built with)"
        default:
            if let tool = ToolSetting.allCases.first(where: { $0.doctorKey == key }) { return tool.title }
            return ServiceFormat.kindTitle(key)
        }
    }

    static func sortedKeys(_ items: [String: DoctorItem]) -> [String] {
        let known = order.filter { items[$0] != nil }
        let rest = items.keys.filter { !order.contains($0) }.sorted()
        return known + rest
    }
}

/// One Doctor line: found or missing, version, path, and the variable that names it.
struct DoctorRow: View {
    let name: String
    let item: DoctorItem

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: item.found ? "checkmark.circle.fill" : "xmark.circle.fill")
                .foregroundStyle(item.found ? Color.green : Color.red)
                .help(item.found ? "Found" : "Missing")
            VStack(alignment: .leading, spacing: 2) {
                HStack {
                    Text(name)
                    if let version = item.version, !version.isEmpty {
                        Text(version).font(.caption.monospaced()).foregroundStyle(.secondary)
                    }
                }
                if let path = item.path, !path.isEmpty {
                    Text(path).font(.caption.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
                } else if !item.found {
                    Text("Not found").font(.caption).foregroundStyle(.secondary)
                }
                if let variable = item.variable, !variable.isEmpty {
                    Text("set with \(variable)").font(.caption2).foregroundStyle(.tertiary)
                }
            }
            Spacer()
        }
    }
}

/// The Doctor section of Settings.
struct DoctorSection: View {
    @Environment(AppModel.self) private var model
    @State private var items: [String: DoctorItem]?
    @State private var checking = false

    var body: some View {
        SwiftUI.Section {
            if let items {
                ForEach(DoctorCatalog.sortedKeys(items), id: \.self) { key in
                    if let item = items[key] {
                        DoctorRow(name: DoctorCatalog.title(key), item: item)
                    }
                }
            } else if !model.isConnected {
                Text("Connect to reelctl serve to run Doctor.").foregroundStyle(.secondary)
            } else {
                Text(checking ? "Checking…" : "Not checked yet").foregroundStyle(.secondary)
            }
            Button(checking ? "Checking…" : "Run Doctor again") {
                Task { await check() }
            }
            .disabled(checking || !model.isConnected)
        } header: {
            Text("Doctor")
        } footer: {
            Text("ffmpeg and ffprobe are required. The other tools and model files are optional; set their paths below. Nothing is downloaded.")
                .font(.caption).foregroundStyle(.secondary)
        }
        .task(id: model.isConnected) {
            if model.isConnected { await check() }
        }
    }

    private func check() async {
        checking = true
        let found: [String: DoctorItem]? = await model.perform { try await $0.doctor() }
        if let found { items = found }
        checking = false
    }
}

/// The OpenRouter key: Set or Not set, a secure field to store a new one, and
/// Delete. The value is never shown, logged or copied.
struct APIKeySection: View {
    @State private var isSet = false
    @State private var newKey = ""
    @State private var storedIn: String?
    @State private var failure: String?
    @State private var confirmDelete = false

    var body: some View {
        SwiftUI.Section {
            LabeledContent("OpenRouter key") {
                HStack(spacing: 6) {
                    Image(systemName: isSet ? "key.fill" : "key.slash")
                        .foregroundStyle(isSet ? Color.green : Color.secondary)
                    Text(isSet ? "Set" : "Not set")
                }
            }
            if let storedIn {
                Text("Stored in the \(storedIn).").font(.caption).foregroundStyle(.secondary)
            }
            HStack {
                SecureField(isSet ? "Replace with a new key" : "Paste the key", text: $newKey)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit(save)
                Button("Save", action: save)
                    .disabled(newKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                Button("Delete", role: .destructive) { confirmDelete = true }
                    .disabled(!isSet)
            }
            if let failure {
                Text(failure).font(.caption).foregroundStyle(.red)
            }
        } header: {
            Text("API key")
        } footer: {
            Text("Kept in the Keychain (service ReelStudio, account openrouter). It is sent only with an Understand call you approve, and is never written to the workspace, the preferences or a log.")
                .font(.caption).foregroundStyle(.secondary)
        }
        .onAppear { isSet = KeyStore.isSet }
        .confirmationDialog("Delete the API key from the Keychain?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Delete", role: .destructive) {
                KeyStore.delete()
                isSet = KeyStore.isSet
                storedIn = nil
            }
            Button("Cancel", role: .cancel) {}
        }
    }

    private func save() {
        let key = newKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else { return }
        do {
            storedIn = try KeyStore.write(key)
            failure = nil
        } catch {
            failure = "Not stored: \(error.localizedDescription)"
        }
        newKey = ""
        isSet = KeyStore.isSet
    }
}
