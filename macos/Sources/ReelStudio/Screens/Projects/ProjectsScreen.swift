import ReelCore
import SwiftUI

/// Projects, the editor (docs/specs/mac-app.md, screen 3). A project is a
/// `project` subject; its head checkpoint's `out` is the current `.otio`.
/// Every operation is a job on the subject with that `.otio` as `project`, so
/// every edit is a checkpoint; undo reverts to the parent, and "Try another
/// version" branches.
struct ProjectsScreen: View {
    @Environment(AppModel.self) private var model
    @State private var editor = ProjectEditor()
    @State private var operation: ProjectOperation?
    @State private var showInspector = false
    @State private var creating = false
    @State private var naming = false
    @State private var branchName = ""

    init() {}

    var body: some View {
        HSplitView {
            ProjectSidebar(editor: editor, creating: $creating)
                .frame(minWidth: 190, idealWidth: 220, maxWidth: 320)
            detail
                .frame(minWidth: 520, maxWidth: .infinity, maxHeight: .infinity)
        }
        .inspector(isPresented: $showInspector) {
            Group {
                if let operation {
                    ProjectOperationForm(operation: operation, editor: editor)
                        .id(operation)
                } else {
                    Text("Choose an operation on the toolbar.").foregroundStyle(.secondary)
                }
            }
            .inspectorColumnWidth(min: 300, ideal: 360, max: 520)
        }
        .toolbar { toolbar }
        .sheet(isPresented: $creating) {
            ProjectNewSheet(editor: editor)
        }
        .alert("Try another version", isPresented: $naming) {
            TextField("Name", text: $branchName)
            Button("Branch") {
                let name = branchName.trimmingCharacters(in: .whitespaces)
                let fallback = (editor.subjectID.flatMap { model.subject($0)?.name } ?? "project") + " (another version)"
                Task { await editor.branch(name: name.isEmpty ? fallback : name, model) }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("A new project starts from the current head; this one stays as it is.")
        }
        .task {
            await open(model.focusedSubject)
        }
        .onChange(of: model.focusedSubject) { _, subject in
            Task { await open(subject) }
        }
        .onChange(of: model.checkpointTick) { _, _ in
            Task { await editor.reload(model) }
        }
    }

    /// Open `subject` when it is a project (the focused subject may be any kind).
    private func open(_ subject: SubjectID?) async {
        guard let subject else { return }
        let kind = model.subject(subject)?.kind
        guard kind == nil || kind == "project" else { return }
        await editor.open(subject, model)
    }

    @ViewBuilder
    private var detail: some View {
        if let id = editor.subjectID {
            ScrollView(.vertical) {
                VStack(alignment: .leading, spacing: 14) {
                    header(id)
                    if let job = editor.job {
                        JobProgressView(job: job)
                    }
                    GroupBox {
                        if let message = editor.timelineError {
                            Label(message, systemImage: "exclamationmark.triangle").foregroundStyle(.orange)
                        } else if editor.timeline != nil {
                            ProjectTrackView(lanes: editor.lanes, selection: $editor.selection)
                        } else {
                            Text(editor.checkpoints.isEmpty ? "The project's first job has not finished yet." : "Loading…")
                                .foregroundStyle(.secondary)
                                .frame(maxWidth: .infinity, minHeight: 60)
                        }
                    }
                    if !editor.lanes.notes.isEmpty {
                        VStack(alignment: .leading, spacing: 2) {
                            ForEach(editor.lanes.notes, id: \.self) { note in
                                Text(note).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                            }
                        }
                    }
                    HStack(alignment: .top, spacing: 16) {
                        ProjectRenderPane(editor: editor)
                            .frame(minWidth: 280, maxWidth: .infinity)
                        ProjectCheckpointList(editor: editor)
                            .frame(minWidth: 260, idealWidth: 320, maxWidth: 380)
                    }
                    ProjectQAGallery(editor: editor)
                }
                .padding(16)
            }
        } else {
            ContentUnavailableView {
                Label("No project open", systemImage: "timeline.selection")
            } description: {
                Text("Choose a project, or make one from a video or from a clip an understanding run picked.")
            } actions: {
                Button("New project…") { creating = true }
            }
        }
    }

    private func header(_ id: SubjectID) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(model.subject(id)?.name ?? "Project \(id)").font(.title2.bold())
            if let parent = model.subject(id)?.parentSubject, let from = model.subject(parent) {
                Text("a version of \(from.name)").font(.callout).foregroundStyle(.secondary)
            }
            Spacer()
            if let head = editor.head {
                Text("head #\(head.id) · \(head.label)").font(.caption.monospaced()).foregroundStyle(.secondary)
            }
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .primaryAction) {
            Button {
                Task { await editor.undo(model) }
            } label: {
                Label("Undo", systemImage: "arrow.uturn.backward")
            }
            .help("Revert to the checkpoint before the edit shown")
            .disabled(editor.undoTarget == nil)

            Button {
                branchName = ""
                naming = true
            } label: {
                Label("Try another version", systemImage: "arrow.triangle.branch")
            }
            .help("Branch: a new project from the head")
            .disabled(editor.head == nil)
        }
        ToolbarItemGroup(placement: .automatic) {
            ForEach(ProjectOperation.allCases) { item in
                Button {
                    choose(item)
                } label: {
                    Label(item.title, systemImage: item.symbol)
                }
                .help(item.title)
                .disabled(editor.projectHash == nil)
            }
        }
    }

    private func choose(_ item: ProjectOperation) {
        operation = item
        showInspector = true
    }
}

/// The project subjects, with "New project…".
struct ProjectSidebar: View {
    @Environment(AppModel.self) private var model
    let editor: ProjectEditor
    @Binding var creating: Bool

    var body: some View {
        VStack(spacing: 0) {
            List(selection: selection) {
                ForEach(model.subjects(kind: "project")) { subject in
                    VStack(alignment: .leading, spacing: 1) {
                        Text(subject.name).lineLimit(1)
                        if subject.headCheckpoint == nil {
                            Text("no checkpoint yet").font(.caption2).foregroundStyle(.secondary)
                        } else if subject.parentSubject != nil {
                            Text("a version").font(.caption2).foregroundStyle(.secondary)
                        }
                    }
                    .tag(subject.id)
                }
            }
            Divider()
            HStack {
                Button {
                    creating = true
                } label: {
                    Label("New project…", systemImage: "plus")
                }
                .buttonStyle(.borderless)
                Spacer()
            }
            .padding(8)
        }
    }

    private var selection: Binding<SubjectID?> {
        Binding<SubjectID?>(
            get: { editor.subjectID },
            set: { model.focusedSubject = $0 }
        )
    }
}
