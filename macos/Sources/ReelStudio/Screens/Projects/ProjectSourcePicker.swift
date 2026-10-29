import ReelCore
import SwiftUI
import UniformTypeIdentifiers

/// Where a DNA or understanding input comes from: an imported file, or the
/// head `out` of a subject (a reel's decompile, an understanding run).
enum ProjectSourceChoice: Hashable, Sendable {
    case none
    case asset(AssetID)
    case subject(SubjectID)

    var isNone: Bool {
        if case .none = self { return true }
        return false
    }

    /// The input for `jobs.submit`; nil when nothing is chosen or the subject
    /// has no `out` yet.
    @MainActor
    func resolve(in model: AppModel) async -> InputRef? {
        switch self {
        case .none:
            return nil
        case .asset(let id):
            return .asset(id)
        case .subject(let id):
            let ref: InputRef?? = await model.perform { studio -> InputRef? in
                guard let head = try await studio.subjects().first(where: { $0.id == id })?.headCheckpoint else { return nil }
                guard let hash = try await studio.checkpoint(head).outputs["out"] else { return nil }
                return InputRef.hash(hash)
            }
            return ref ?? nil
        }
    }
}

/// Pick an `edit_dna` or `understanding` source, or import a file.
struct ProjectSourcePicker: View {
    @Environment(AppModel.self) private var model
    let title: String
    let source: ProjectSourceKind
    @Binding var selection: ProjectSourceChoice
    var highlighted = false
    /// The project's recorded DNA (`metadata.reelctl.dna.path`), offered as a shortcut.
    var projectDNA: String? = nil

    var body: some View {
        HStack(spacing: 6) {
            Picker(title, selection: $selection) {
                Text("None").tag(ProjectSourceChoice.none)
                let files = model.assets.filter { $0.kind == source.assetKind }
                if !files.isEmpty {
                    SwiftUI.Section("Files") {
                        ForEach(files) { asset in
                            Text(asset.name).tag(ProjectSourceChoice.asset(asset.id))
                        }
                    }
                }
                let subjects = model.subjects(kind: source.subjectKind).filter { $0.headCheckpoint != nil }
                if !subjects.isEmpty {
                    SwiftUI.Section(source == .dna ? "Reels" : "Understanding runs") {
                        ForEach(subjects) { subject in
                            Text(subject.name).tag(ProjectSourceChoice.subject(subject.id))
                        }
                    }
                }
            }
            Menu {
                Button("Import a file…") { importFile() }
                if let projectDNA, source == .dna {
                    Button("Use the project's DNA (\(URL(fileURLWithPath: projectDNA).lastPathComponent))") {
                        Task { await use(URL(fileURLWithPath: projectDNA)) }
                    }
                }
            } label: {
                Image(systemName: "plus")
            }
            .menuStyle(.button)
            .buttonStyle(.borderless)
            .menuIndicator(.hidden)
            .fixedSize()
            .help("Import a file, or use the project's DNA")
        }
        .padding(.horizontal, 3)
        .overlay {
            RoundedRectangle(cornerRadius: 5).stroke(Color.red, lineWidth: highlighted ? 2 : 0)
        }
    }

    private func importFile() {
        guard let url = chooseFile([.json], title: title) else { return }
        Task { await use(url) }
    }

    private func use(_ url: URL) async {
        if let asset = await model.importFile(url, kind: source.assetKind) {
            selection = .asset(asset.id)
        }
    }
}
