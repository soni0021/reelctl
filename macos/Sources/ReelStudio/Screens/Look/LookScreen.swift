import AppKit
import ReelCore
import SwiftUI
import UniformTypeIdentifiers

/// Look (docs/specs/mac-app.md, screen 4): pick a draft and an outlier, run
/// `look` on a `look` subject, and preview the `.cube` on the draft as a split
/// wipe (`LookPreviewPane`).
struct LookScreen: View {
    @Environment(AppModel.self) private var model

    @State private var draft: AssetID?
    @State private var outlier: AssetID?
    @State private var subject: SubjectID?
    /// The catalogue's `size` is a choice of text values.
    @State private var size = "33"
    @State private var samples = 60
    @State private var decoder = "auto"
    @State private var job: JobID?
    @State private var jobSubject: SubjectID?
    @State private var submitting = false
    @State private var cubeFile: StoredFile?
    @State private var cube: CubeLUT?
    @State private var cubeError: String?

    static let sizes: [String] = ["17", "33", "65"]
    static let decoders: [String] = ["auto", "software"]

    init() {}

    var body: some View {
        HSplitView {
            form
                .frame(minWidth: 320, idealWidth: 380, maxWidth: 460, maxHeight: .infinity)
            LookPreviewPane(draftURL: draftURL, cube: cube, cubeTag: cubeFile?.hash)
                .frame(minWidth: 480, maxWidth: .infinity, maxHeight: .infinity)
        }
        .task(id: subject) { await loadHead() }
    }

    // MARK: Form

    private var form: some View {
        Form {
            SwiftUI.Section("Videos") {
                AssetPicker(title: "Draft", kind: "video", types: UTType.videoTypes, selection: $draft)
                AssetPicker(title: "Outlier", kind: "video", types: UTType.videoTypes, selection: $outlier)
            }
            SwiftUI.Section("Look") {
                SubjectPicker(kind: "look", selection: $subject)
                Picker("LUT size", selection: $size) {
                    ForEach(Self.sizes, id: \.self) { value in
                        Text(value).tag(value)
                    }
                }
                .pickerStyle(.segmented)
                Stepper(value: $samples, in: 1...600, step: 10) {
                    LabeledContent("Frames sampled", value: "\(samples)")
                }
                Picker("Decoder", selection: $decoder) {
                    ForEach(Self.decoders, id: \.self) { value in
                        Text(value).tag(value)
                    }
                }
            }
            SwiftUI.Section {
                Button("Match the outlier's look") {
                    Task { await run() }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!canRun)
                if let job {
                    JobProgressView(job: job)
                }
            } footer: {
                Text("Per-channel histogram matching: it moves tone and colour cast, not hue-selective grades.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            if let cube, let cubeFile {
                SwiftUI.Section("Cube") {
                    FieldRow(label: "Title", value: cube.title.isEmpty ? "(untitled)" : cube.title)
                    FieldRow(label: "Size", value: "\(cube.size) × \(cube.size) × \(cube.size)")
                    FieldRow(label: "File", value: cubeFile.url.lastPathComponent)
                    HStack {
                        Button("Reveal in Finder") {
                            NSWorkspace.shared.activateFileViewerSelecting([cubeFile.url])
                        }
                        Button("Save a copy…") { saveCopy(cubeFile) }
                    }
                }
            }
            if let cubeError {
                SwiftUI.Section {
                    Text(cubeError).foregroundStyle(.red).textSelection(.enabled)
                }
            }
        }
        .formStyle(.grouped)
    }

    // MARK: State

    private var draftURL: URL? {
        guard let draft else { return nil }
        return model.assets.first(where: { $0.id == draft })?.url
    }

    private var running: Bool {
        if submitting { return true }
        guard let job, let current = model.job(job) else { return false }
        return !current.status.finished
    }

    private var canRun: Bool {
        model.isConnected && draft != nil && outlier != nil && !running
    }

    private func assetName(_ id: AssetID?) -> String {
        guard let id, let asset = model.assets.first(where: { $0.id == id }) else { return "video" }
        return asset.name
    }

    // MARK: Actions

    private func ensureSubject() async -> SubjectID? {
        if let subject { return subject }
        let name = "\(assetName(draft)) → \(assetName(outlier))"
        guard let created = await model.perform({ try await $0.createSubject(kind: "look", name: name) }) else {
            return nil
        }
        await model.refreshSubjects()
        jobSubject = created.id
        subject = created.id
        return created.id
    }

    private func run() async {
        guard let draft, let outlier else { return }
        submitting = true
        cubeError = nil
        guard let subjectID = await ensureSubject() else {
            submitting = false
            return
        }
        let args: [String: JSONValue] = [
            "size": .string(size),
            "samples": .int(Int64(samples)),
            "decoder": .string(decoder),
        ]
        let inputs: [String: InputRef] = ["draft": .asset(draft), "outlier": .asset(outlier)]
        let submitted = await model.perform {
            try await $0.submit(subject: subjectID, command: "look", args: args, inputs: inputs)
        }
        submitting = false
        guard let submitted else { return }
        job = submitted.job
        jobSubject = subjectID
        guard let finished = await model.waitFor(submitted.job) else { return }
        // A failure shows in JobProgressView (status and error).
        guard finished.status == .succeeded else { return }
        guard let file = await model.output(of: submitted.job, role: "out") else {
            cubeError = "The look job succeeded but its checkpoint has no `out` file."
            return
        }
        await load(file)
    }

    /// Show the head checkpoint's cube when a subject is chosen.
    private func loadHead() async {
        if jobSubject != subject {
            job = nil
            jobSubject = nil
        }
        cube = nil
        cubeFile = nil
        cubeError = nil
        guard let chosen = subject else { return }
        guard let file = await model.headFile(of: chosen, role: "out") else { return }
        guard subject == chosen else { return }
        await load(file)
    }

    private func load(_ file: StoredFile) async {
        let url = file.url
        do {
            let lut = try await Task.detached { () throws -> CubeLUT in
                try CubeLUT(url: url)
            }.value
            cube = lut
            cubeFile = file
            cubeError = nil
        } catch {
            cube = nil
            cubeFile = nil
            cubeError = "Could not read \(url.lastPathComponent): \(error)"
        }
    }

    /// Copy the read-only stored file to a place the user picks (a new,
    /// writable file, not a link into the store).
    private func saveCopy(_ file: StoredFile) {
        let panel = NSSavePanel()
        panel.title = "Save a copy of the cube"
        panel.allowedContentTypes = [UTType.cube]
        panel.canCreateDirectories = true
        let base = "\(assetName(outlier))".replacingOccurrences(of: "/", with: "-")
        panel.nameFieldStringValue = (base as NSString).deletingPathExtension + ".cube"
        guard panel.runModal() == .OK, let destination = panel.url else { return }
        do {
            let data = try Data(contentsOf: file.url)
            try data.write(to: destination, options: .atomic)
        } catch {
            model.banner = "Could not save the cube: \(error.localizedDescription)"
        }
    }
}
