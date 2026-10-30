import ReelCore
import SwiftUI

/// A subject's checkpoints (spec screen 11): revert, branch, compare two, and
/// reveal the stored outputs.
struct HistoryScreen: View {
    @Environment(AppModel.self) private var model
    @State private var subjectID: SubjectID?
    @State private var checkpoints: [Checkpoint] = []
    @State private var selection = Set<CheckpointID>()
    @State private var loading = false
    @State private var naming = false
    @State private var branchName = ""
    @State private var comparison: HistoryComparison?
    @State private var confirmRevert = false
    @State private var player = FramePlayer()
    @State private var showPlayer = false
    @State private var playingTitle = ""

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                subjectPicker
                Spacer()
                if loading { ProgressView().controlSize(.small) }
                Text(selectionHint).font(.caption).foregroundStyle(.secondary)
            }
            .padding(10)
            Divider()
            if subjectID == nil {
                ContentUnavailableView("Pick a subject", systemImage: "clock.arrow.circlepath",
                                       description: Text("Every job on a subject makes a checkpoint."))
            } else if checkpoints.isEmpty && !loading {
                ContentUnavailableView("No checkpoints yet", systemImage: "clock.arrow.circlepath")
            } else {
                table
            }
        }
        .navigationTitle("History")
        .toolbar { toolbar }
        .sheet(isPresented: $showPlayer) {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    Text(playingTitle).font(.headline)
                    Spacer()
                    Button("Done") { showPlayer = false }
                }
                PlayerView(player: player.player)
                    .frame(minWidth: 360, minHeight: 640)
            }
            .padding(16)
            .onDisappear { player.load(nil) }
        }
        .onAppear {
            if let focused = model.focusedSubject { subjectID = focused }
        }
        .onChange(of: model.focusedSubject) { _, focused in
            if let focused, focused != subjectID { subjectID = focused }
        }
        .onChange(of: subjectID) { _, subject in
            selection = []
            if let subject { model.focusedSubject = subject }
        }
        .task(id: "\(subjectID ?? -1)/\(model.checkpointTick)") {
            await reload()
        }
        .alert("Branch from checkpoint", isPresented: $naming) {
            TextField("Name of the new subject", text: $branchName)
            Button("Branch") { Task { await branch() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("A new subject starts from this checkpoint, so another edit can be tried without touching this one.")
        }
        .confirmationDialog("Revert to this checkpoint?", isPresented: $confirmRevert, titleVisibility: .visible) {
            Button("Revert") { Task { await revert() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("A new checkpoint with the same outputs is added and becomes the head. Nothing is deleted, so the revert can itself be reverted.")
        }
        .sheet(item: $comparison) { comparison in
            CheckpointCompareSheet(comparison: comparison)
        }
    }

    private var subject: Subject? { subjectID.flatMap { model.subject($0) } }

    /// Newest first.
    private var rows: [Checkpoint] { checkpoints.sorted { $0.id > $1.id } }

    private var chosen: [CheckpointID] { selection.sorted() }

    private var selectionHint: String {
        switch selection.count {
        case 0: return "Select a checkpoint, or two to compare (⌘-click)"
        case 1: return "1 selected"
        case 2: return "2 selected: Compare"
        default: return "\(selection.count) selected: choose one or two"
        }
    }

    private var subjectPicker: some View {
        Picker("Subject", selection: $subjectID) {
            Text("None").tag(SubjectID?.none)
            ForEach(Subject.kinds, id: \.self) { kind in
                let list = model.subjects(kind: kind)
                if !list.isEmpty {
                    SwiftUI.Section(ServiceFormat.kindTitle(kind)) {
                        ForEach(list) { subject in
                            Text(subject.name).tag(SubjectID?.some(subject.id))
                        }
                    }
                }
            }
        }
        .frame(maxWidth: 360)
    }

    private var table: some View {
        let head = subject?.headCheckpoint
        return Table(rows, selection: $selection) {
            TableColumn("") { (checkpoint: Checkpoint) in
                if checkpoint.id == head {
                    Image(systemName: "arrowtriangle.right.fill").foregroundStyle(.tint).help("Head")
                }
            }
            .width(18)
            TableColumn("#") { (checkpoint: Checkpoint) in
                Text("\(checkpoint.id)").monospacedDigit()
            }
            .width(min: 30, ideal: 40, max: 60)
            TableColumn("Label") { (checkpoint: Checkpoint) in
                HStack(spacing: 4) {
                    Text(checkpoint.label)
                    if checkpoint.id == head { ServiceBadge(text: "head", color: .green) }
                }
            }
            TableColumn("Job") { (checkpoint: Checkpoint) in
                Text(jobText(checkpoint)).foregroundStyle(.secondary)
            }
            TableColumn("Cached") { (checkpoint: Checkpoint) in
                if checkpoint.cached { ServiceBadge(text: "cached") }
            }
            .width(min: 50, ideal: 60, max: 80)
            TableColumn("Outputs") { (checkpoint: Checkpoint) in
                Text(checkpoint.outputs.keys.sorted().joined(separator: ", ")).font(.caption.monospaced())
            }
            TableColumn("Note") { (checkpoint: Checkpoint) in
                Text(checkpoint.note).foregroundStyle(.secondary).help(checkpoint.note)
            }
            TableColumn("Time") { (checkpoint: Checkpoint) in
                Text(ServiceFormat.time(checkpoint.createdAt)).foregroundStyle(.secondary)
            }
        }
        .contextMenu(forSelectionType: CheckpointID.self) { ids in
            if ids.count == 1, let id = ids.first {
                Button("Revert to #\(id)") { selection = [id]; confirmRevert = true }
                Button("Branch from #\(id)…") { selection = [id]; startBranch() }
                Button("Reveal outputs in Finder") { Task { await reveal(id) } }
                Button("Play video") { Task { await playReel(id) } }
                Button("Export video…") { Task { await exportReel(id) } }
            }
            if ids.count == 2 {
                Button("Compare") { selection = ids; Task { await compare() } }
            }
        } primaryAction: { ids in
            if ids.count == 1, let id = ids.first { Task { await reveal(id) } }
        }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup {
            Button {
                confirmRevert = true
            } label: {
                Label("Revert to", systemImage: "arrow.uturn.backward")
            }
            .disabled(chosen.count != 1 || subjectID == nil)
            .help("Make the selected checkpoint the head again")
            Button {
                startBranch()
            } label: {
                Label("Branch", systemImage: "arrow.triangle.branch")
            }
            .disabled(chosen.count != 1)
            .help("Start a new subject from the selected checkpoint")
            Button {
                Task { await compare() }
            } label: {
                Label("Compare", systemImage: "square.split.2x1")
            }
            .disabled(chosen.count != 2)
            .help("Compare the two selected checkpoints' outputs")
            Button {
                if let id = chosen.first { Task { await reveal(id) } }
            } label: {
                Label("Reveal", systemImage: "folder")
            }
            .disabled(chosen.count != 1)
            .help("Show the selected checkpoint's stored outputs in Finder")
            Button {
                if let id = chosen.first { Task { await playReel(id) } }
            } label: {
                Label("Play", systemImage: "play.rectangle")
            }
            .disabled(chosen.count != 1)
            .help("Play the video stored in the selected checkpoint")
            Button {
                if let id = chosen.first { Task { await exportReel(id) } }
            } label: {
                Label("Export", systemImage: "square.and.arrow.down")
            }
            .disabled(chosen.count != 1)
            .help("Save the selected checkpoint's video as an .mp4 file")
            Button {
                Task { await reload() }
            } label: {
                Label("Reload", systemImage: "arrow.clockwise")
            }
        }
    }

    private func jobText(_ checkpoint: Checkpoint) -> String {
        guard let id = checkpoint.job else { return "—" }
        if let job = model.job(id) { return "#\(id) \(job.shortCommand)" }
        return "#\(id)"
    }

    private func reload() async {
        guard let subjectID else {
            checkpoints = []
            return
        }
        loading = true
        let list: [Checkpoint]? = await model.perform { try await $0.checkpoints(subject: subjectID) }
        if let list, subjectID == self.subjectID {
            checkpoints = list
            selection = selection.filter { id in list.contains { $0.id == id } }
        }
        loading = false
    }

    private func revert() async {
        guard let subjectID, chosen.count == 1, let target = chosen.first else { return }
        let made: Checkpoint? = await model.perform { try await $0.revert(subject: subjectID, to: target) }
        if let made {
            await model.refreshSubjects()
            await reload()
            selection = [made.id]
        }
    }

    private func startBranch() {
        guard let id = chosen.first else { return }
        let base = subject?.name ?? "branch"
        branchName = "\(base) from #\(id)"
        naming = true
    }

    private func branch() async {
        guard chosen.count == 1, let id = chosen.first else { return }
        let trimmed = branchName.trimmingCharacters(in: .whitespacesAndNewlines)
        let name = trimmed.isEmpty ? "branch from #\(id)" : trimmed
        let made: (Subject, Checkpoint)? = await model.perform { try await $0.branch(from: id, name: name) }
        guard let made else { return }
        await model.refreshSubjects()
        model.focusedSubject = made.0.id
        subjectID = made.0.id
    }

    private func compare() async {
        guard chosen.count == 2 else { return }
        let a = chosen[0]
        let b = chosen[1]
        let diff: CheckpointDiff? = await model.perform { try await $0.diff(a, b) }
        if let diff { comparison = HistoryComparison(a: a, b: b, diff: diff) }
    }

    /// The first stored video of a checkpoint, if it has one.
    private func videoFile(_ id: CheckpointID) async -> (url: URL, label: String)? {
        let found: Checkpoint? = await model.perform { try await $0.checkpoint(id) }
        guard let files = found?.files else { return nil }
        for key in files.keys.sorted() {
            if let file = files[key], Playable.isVideo(file.url) { return (file.url, key) }
        }
        return nil
    }

    private func playReel(_ id: CheckpointID) async {
        guard let video = await videoFile(id) else {
            model.banner = "Checkpoint #\(id) has no video."
            return
        }
        let subject = subjectID.flatMap { model.subject($0)?.name } ?? "video"
        playingTitle = "\(subject) · checkpoint #\(id)"
        player.load(video.url)
        showPlayer = true
    }

    private func exportReel(_ id: CheckpointID) async {
        guard let video = await videoFile(id) else {
            model.banner = "Checkpoint #\(id) has no video."
            return
        }
        let subject = subjectID.flatMap { model.subject($0)?.name } ?? "video"
        exportVideo(video.url, suggestedName: "\(subject)-#\(id)", model: model)
    }

    private func reveal(_ id: CheckpointID) async {
        let found: Checkpoint? = await model.perform { try await $0.checkpoint(id) }
        guard let files = found?.files else { return }
        let urls = files.keys.sorted().compactMap { files[$0]?.url }
        if urls.isEmpty {
            model.banner = "Checkpoint #\(id) has no stored outputs."
        } else {
            serviceReveal(urls)
        }
    }
}

/// Two checkpoints and their `checkpoints.diff`.
struct HistoryComparison: Identifiable {
    let a: CheckpointID
    let b: CheckpointID
    let diff: CheckpointDiff
    var id: String { "\(a)-\(b)" }
}
