import ReelCore
import SwiftUI
import UniformTypeIdentifiers

/// Make a project subject and its first `.otio`: `timeline.init` over a
/// whole video, or `timeline.from-clip` over one clip an understanding run
/// picked.
struct ProjectNewSheet: View {
    enum Start: String, CaseIterable, Identifiable {
        case whole = "timeline.init"
        case clip = "timeline.from-clip"

        var id: String { rawValue }

        var title: String {
            switch self {
            case .whole: return "Whole video"
            case .clip: return "One clip"
            }
        }

        var args: [ProjectArgSpec] {
            switch self {
            case .whole: return ProjectCatalogue.initArgs
            case .clip: return ProjectCatalogue.fromClipArgs
            }
        }
    }

    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let editor: ProjectEditor
    @State private var name = ""
    @State private var start: Start = .whole
    @State private var video: AssetID?
    @State private var dna: ProjectSourceChoice = .none
    @State private var understanding: ProjectSourceChoice = .none
    @State private var values = ProjectFormValues(specs: ProjectCatalogue.fromClipArgs)
    @State private var problem: ProjectFormProblem?
    /// The subject made on a first try, reused when the job was refused.
    @State private var made: SubjectID?
    @State private var submitting = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("New project").font(.title2.bold())
            Form {
                TextField("Name", text: $name, prompt: Text("my reel"))
                Picker("Start from", selection: $start) {
                    ForEach(Start.allCases) { start in
                        Text(start.title).tag(start)
                    }
                }
                .pickerStyle(.segmented)
                AssetPicker(title: "Video *", kind: "video", types: UTType.videoTypes, selection: $video)
                    .padding(.horizontal, 3)
                    .overlay {
                        RoundedRectangle(cornerRadius: 5).stroke(Color.red, lineWidth: problem?.key == "input" ? 2 : 0)
                    }
                ProjectSourcePicker(
                    title: start == .clip ? "Edit DNA *" : "Edit DNA",
                    source: .dna,
                    selection: $dna,
                    highlighted: problem?.key == "dna"
                )
                if start == .clip {
                    ProjectSourcePicker(
                        title: "Understanding *",
                        source: .understanding,
                        selection: $understanding,
                        highlighted: problem?.key == "understanding"
                    )
                    ProjectArgList(specs: start.args, values: $values, problem: problem)
                }
            }
            .formStyle(.grouped)
            if let problem {
                Label(problem.message, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Text(start.rawValue).font(.caption.monospaced()).foregroundStyle(.secondary)
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Create") { Task { await create() } }
                    .keyboardShortcut(.defaultAction)
                    .disabled(submitting)
            }
        }
        .padding(20)
        .frame(width: 520)
        .frame(minHeight: 360)
    }

    private func create() async {
        problem = nil
        let command = start.rawValue
        guard let video else {
            problem = ProjectFormProblem(command: command, key: "input", message: "Choose the video.")
            return
        }
        let args: [String: JSONValue]
        switch values.arguments(for: start.args, command: command) {
        case .success(let built): args = built
        case .failure(let refused):
            problem = refused
            return
        }
        var inputs: [String: InputRef] = ["input": .asset(video)]
        let needed: [(role: String, choice: ProjectSourceChoice, required: Bool)] = start == .clip
            ? [("dna", dna, true), ("understanding", understanding, true)]
            : [("dna", dna, false)]
        for slot in needed {
            if slot.choice.isNone {
                if slot.required {
                    problem = ProjectFormProblem(command: command, key: slot.role, message: "Choose the \(slot.role == "dna" ? "edit DNA" : "understanding").")
                    return
                }
                continue
            }
            guard let ref = await slot.choice.resolve(in: model) else {
                problem = ProjectFormProblem(command: command, key: slot.role, message: "That subject has no output yet.")
                return
            }
            inputs[slot.role] = ref
        }
        submitting = true
        defer { submitting = false }
        let subject: SubjectID
        if let made {
            subject = made
        } else {
            let fallback = model.assets.first(where: { $0.id == video })?.name ?? "project"
            let title = name.trimmingCharacters(in: .whitespaces).isEmpty ? fallback : name
            guard let created = await model.perform({ try await $0.createSubject(kind: "project", name: title) }) else { return }
            made = created.id
            subject = created.id
            await model.refreshSubjects()
        }
        if !dna.isNone { editor.lastDNA = dna }
        let keys = start.args.map(\.key) + ["input", "dna", "understanding"]
        if let refused = await editor.start(subject: subject, command: command, args: args, inputs: inputs, keys: keys, model) {
            problem = refused
            return
        }
        model.focusedSubject = subject
        dismiss()
    }
}
