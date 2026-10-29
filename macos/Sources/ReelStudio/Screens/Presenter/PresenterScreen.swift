import AppKit
import ReelCore
import SwiftUI
import UniformTypeIdentifiers

/// Presenter (docs/specs/mac-app.md, screen 5): `compositor.isolate-presenter`
/// on a `presenter` subject. Checks Doctor first; without RVM weights or ONNX
/// Runtime it says what is missing and does not submit.
struct PresenterScreen: View {
    @Environment(AppModel.self) private var model

    @State private var input: AssetID?
    @State private var subject: SubjectID?
    @State private var options = PresenterOptions()
    @State private var checked = false
    @State private var checking = false
    @State private var missing: [PresenterMissingTool] = []
    @State private var weightsPath: String?
    @State private var submitting = false
    @State private var job: JobID?
    @State private var jobSubject: SubjectID?
    @State private var jobMode: String?
    @State private var output: StoredFile?
    @State private var outputError: String?
    @State private var before = FramePlayer()
    @State private var after = FramePlayer()

    init() {}

    var body: some View {
        HSplitView {
            form
                .frame(minWidth: 320, idealWidth: 380, maxWidth: 460, maxHeight: .infinity)
            results
                .padding(12)
                .frame(minWidth: 520, maxWidth: .infinity, maxHeight: .infinity)
        }
        .task { _ = await checkTools() }
        .task(id: subject) { await loadHead() }
        .task(id: inputURL) { before.load(inputURL) }
        .onDisappear {
            before.player.pause()
            after.player.pause()
        }
    }

    // MARK: Form

    private var form: some View {
        Form {
            SwiftUI.Section("Video") {
                AssetPicker(title: "Presenter video", kind: "video", types: UTType.videoTypes, selection: $input)
                Text("SDR only: HLG or PQ input is refused.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            SwiftUI.Section("Cut-out") {
                SubjectPicker(kind: "presenter", selection: $subject)
                PresenterOptionsForm(options: $options)
            }
            SwiftUI.Section("Model") {
                FieldRow(label: "RVM weights", value: weightsPath ?? (checked ? "not found" : "checking…"))
                if checking {
                    ProgressView().controlSize(.small)
                }
            }
            SwiftUI.Section {
                Button("Cut out the presenter") {
                    Task { await run() }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!canRun)
                if let job {
                    JobProgressView(job: job)
                    PresenterFrameCount(job: job)
                }
                if let outputError {
                    Text(outputError).foregroundStyle(.red).textSelection(.enabled)
                }
            } footer: {
                Text("RobustVideoMatting on the CPU: not real time, about 80 to 180 ms per 720p frame.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }

    @ViewBuilder
    private var results: some View {
        if !missing.isEmpty {
            PresenterMissingView(missing: missing) {
                Task { _ = await checkTools() }
            }
        } else {
            PresenterBeforeAfter(before: before, after: after, output: output, mode: jobMode)
        }
    }

    // MARK: State

    private var inputURL: URL? {
        guard let input else { return nil }
        return model.assets.first(where: { $0.id == input })?.url
    }

    private var running: Bool {
        if submitting { return true }
        guard let job, let current = model.job(job) else { return false }
        return !current.status.finished
    }

    private var canRun: Bool {
        model.isConnected && input != nil && options.problem == nil && missing.isEmpty && !running && !checking
    }

    // MARK: Actions

    /// `doctor.check`; returns the weights path to send when nothing is missing.
    private func checkTools() async -> String? {
        checking = true
        let found = await model.perform { try await $0.doctor() }
        checking = false
        checked = true
        guard let found else { return nil }
        let setting = ToolSetting.rvmWeights.value
        missing = PresenterTools.missing(doctor: found, setting: setting)
        weightsPath = PresenterTools.weights(doctor: found, setting: setting)
        guard missing.isEmpty else { return nil }
        return weightsPath
    }

    private func ensureSubject() async -> SubjectID? {
        if let subject { return subject }
        let name = model.assets.first(where: { $0.id == input })?.name ?? "presenter"
        guard let created = await model.perform({ try await $0.createSubject(kind: "presenter", name: name) }) else {
            return nil
        }
        await model.refreshSubjects()
        jobSubject = created.id
        subject = created.id
        return created.id
    }

    private func run() async {
        guard let input, options.problem == nil else { return }
        submitting = true
        outputError = nil
        // Doctor again, right before submitting: never submit without the tools.
        guard let weights = await checkTools(), let subjectID = await ensureSubject() else {
            submitting = false
            return
        }
        let args = options.args(weights: weights)
        let inputs: [String: InputRef] = ["input": .asset(input)]
        let submitted = await model.perform {
            try await $0.submit(subject: subjectID, command: "compositor.isolate-presenter", args: args, inputs: inputs)
        }
        submitting = false
        guard let submitted else { return }
        job = submitted.job
        jobSubject = subjectID
        jobMode = options.mode.rawValue
        output = nil
        after.player.pause()
        after.load(nil)
        guard let finished = await model.waitFor(submitted.job) else { return }
        // A failure shows in JobProgressView (status and error).
        guard finished.status == .succeeded else { return }
        guard let file = await model.output(of: submitted.job, role: "out") else {
            outputError = "The job succeeded but its checkpoint has no `out` file."
            return
        }
        output = file
        after.load(file.url)
    }

    /// Show the head checkpoint's cut-out when a subject is chosen.
    private func loadHead() async {
        if jobSubject != subject {
            job = nil
            jobSubject = nil
            jobMode = nil
            output = nil
            outputError = nil
            after.player.pause()
            after.load(nil)
        }
        guard let chosen = subject, output == nil else { return }
        guard let file = await model.headFile(of: chosen, role: "out") else { return }
        guard subject == chosen, output == nil else { return }
        output = file
        after.load(file.url)
    }
}
