import ReelCore
import SwiftUI

/// The inspector form of one operation, built from the catalogue's arguments
/// and input roles. Submitting makes a job on the project; it becomes a
/// checkpoint when it succeeds.
struct ProjectOperationForm: View {
    @Environment(AppModel.self) private var model
    let operation: ProjectOperation
    let editor: ProjectEditor
    @State private var values: ProjectFormValues
    @State private var sources: [String: ProjectSourceChoice] = [:]
    @State private var localProblem: ProjectFormProblem?
    @State private var submitting = false

    init(operation: ProjectOperation, editor: ProjectEditor) {
        self.operation = operation
        self.editor = editor
        _values = State(initialValue: ProjectFormValues(specs: operation.args))
    }

    /// The problem to show: a local check, or the service's for this command.
    private var problem: ProjectFormProblem? {
        if let localProblem { return localProblem }
        if let shared = editor.problem, shared.command == operation.command { return shared }
        return nil
    }

    private var keys: [String] { operation.args.map(\.key) + operation.inputs.map(\.role) }

    var body: some View {
        Form {
            SwiftUI.Section {
                Text(operation.summary)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            } header: {
                Label(operation.title, systemImage: operation.symbol).font(.headline)
            }

            if !operation.inputs.isEmpty {
                SwiftUI.Section("Inputs") {
                    ForEach(operation.inputs) { slot in
                        ProjectSourcePicker(
                            title: slot.required ? slot.title + " *" : slot.title,
                            source: slot.source,
                            selection: sourceBinding(slot.role),
                            highlighted: problem?.key == slot.role,
                            projectDNA: editor.lanes.dnaPath
                        )
                    }
                }
            }

            if operation == .punchIn {
                SwiftUI.Section("Range") {
                    if let range = editor.selectionMs {
                        LabeledContent("Selected", value: "\(formatMs(range.start))–\(formatMs(range.end))")
                        Button("Use the selected range") { fillRange() }
                    } else {
                        Text("Drag across the track to select the span.").foregroundStyle(.secondary)
                    }
                }
            }

            if operation == .removeOverlay, !editor.lanes.overlayIDs.isEmpty {
                SwiftUI.Section("Overlays in the project") {
                    ForEach(editor.lanes.overlayIDs, id: \.self) { id in
                        Button(id) { values.text["id"] = id }
                            .buttonStyle(.link)
                    }
                }
            }

            if !operation.args.isEmpty {
                SwiftUI.Section(operation == .captions ? "Style" : "Options") {
                    ProjectArgList(specs: operation.args, values: $values, problem: problem)
                }
            }

            if let problem {
                SwiftUI.Section {
                    Label(problem.message, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            SwiftUI.Section {
                HStack {
                    Spacer()
                    Button {
                        Task { await submit() }
                    } label: {
                        Label(operation.title, systemImage: operation.symbol)
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(submitting || editor.projectHash == nil)
                }
            }
        }
        .formStyle(.grouped)
        .overlay {
            RoundedRectangle(cornerRadius: 8)
                .stroke(Color.red, lineWidth: problem != nil && problem?.key == nil ? 2 : 0)
                .padding(2)
        }
        .onAppear {
            for slot in operation.inputs where slot.role == "dna" && sources["dna"] == nil {
                sources["dna"] = editor.lastDNA
            }
            if operation == .punchIn { fillRange() }
        }
        .onChange(of: editor.selection) { _, _ in
            if operation == .punchIn { fillRange() }
        }
    }

    private func sourceBinding(_ role: String) -> Binding<ProjectSourceChoice> {
        Binding<ProjectSourceChoice>(
            get: { sources[role] ?? .none },
            set: { sources[role] = $0 }
        )
    }

    private func fillRange() {
        guard let range = editor.selectionMs else { return }
        values.text["start_ms"] = String(range.start)
        values.text["end_ms"] = String(range.end)
    }

    private func submit() async {
        localProblem = nil
        editor.problem = nil
        let args: [String: JSONValue]
        switch values.arguments(for: operation.args, command: operation.command) {
        case .success(let built): args = built
        case .failure(let refused):
            localProblem = refused
            return
        }
        var inputs: [String: InputRef] = [:]
        for slot in operation.inputs {
            let choice = sources[slot.role] ?? .none
            if choice.isNone {
                if slot.required {
                    localProblem = ProjectFormProblem(command: operation.command, key: slot.role, message: "\(slot.title) is required.")
                    return
                }
                continue
            }
            guard let ref = await choice.resolve(in: model) else {
                localProblem = ProjectFormProblem(command: operation.command, key: slot.role, message: "\(slot.title) has no output yet.")
                return
            }
            inputs[slot.role] = ref
        }
        if let dna = sources["dna"], !dna.isNone { editor.lastDNA = dna }
        submitting = true
        _ = await editor.submit(command: operation.command, args: args, inputs: inputs, keys: keys, model)
        submitting = false
    }
}
