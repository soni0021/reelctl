import AVFoundation
import Foundation
import Observation
import ReelCore

/// One picture of a QA run (`qa.inspect-boundaries`' `views` tree).
struct ProjectQAImage: Identifiable, Hashable, Sendable {
    let id: String
    let name: String
    let url: URL
}

/// The latest render of the project.
struct ProjectRender: Equatable, Sendable {
    let checkpoint: CheckpointID
    let url: URL
    /// Made from an earlier state of the project than the one shown.
    let stale: Bool
}

/// A tree object's manifest in the store.
private struct ProjectTreeManifest: Decodable {
    struct Entry: Decodable {
        let name: String
        let hash: String
        let kind: String
    }

    let kind: String
    let entries: [Entry]
}

/// The open project: its history, the `.otio` it is at, the latest render and
/// QA run, the selected range, and the job running on it. Every change is a
/// job or a checkpoint call through `AppModel.perform`.
///
/// A checkpoint's state: a revert (`revert to #N`) or branch (`branch of #N`)
/// is the state of N; a `render` or QA run leaves the project as its parent
/// had it; any other checkpoint with an `out` holds the `.otio`.
@MainActor
@Observable
final class ProjectEditor {
    var subjectID: SubjectID?
    /// Newest first; the first is the head (every append moves the head).
    private(set) var checkpoints: [Checkpoint] = []
    /// The checkpoint whose `out` is the `.otio` shown (resolved through copies).
    private(set) var current: Checkpoint?
    private(set) var timeline: ProjectTimeline?
    private(set) var lanes = ProjectLanes.empty
    private(set) var timelineError: String?
    private(set) var render: ProjectRender?
    private(set) var gallery: [ProjectQAImage] = []
    private(set) var galleryCheckpoint: CheckpointID?
    /// The job shown in the progress row (the latest one started here).
    private(set) var job: JobID?
    var selection: ProjectFrameRange?
    /// The field to highlight in the open form.
    var problem: ProjectFormProblem?
    /// The DNA chosen last, offered again in the next form.
    var lastDNA: ProjectSourceChoice = .none

    let player = AVPlayer()
    @ObservationIgnored private var known: [CheckpointID: Checkpoint] = [:]
    @ObservationIgnored private var loadedOtio: String?
    @ObservationIgnored private var loadedRender: URL?
    @ObservationIgnored private var loadedGallery: String?

    init() {}

    var head: Checkpoint? { checkpoints.first }

    /// The project `.otio` hash every operation takes as `project`.
    var projectHash: String? { current?.outputs["out"] }

    /// Undo goes to the parent of the edit shown, when it is in this subject.
    var undoTarget: CheckpointID? {
        guard let current, current.subject == subjectID else { return nil }
        return current.parent
    }

    var selectionMs: (start: Int64, end: Int64)? {
        guard let selection, selection.frames > 0 else { return nil }
        return (lanes.ms(selection.start), lanes.ms(selection.end))
    }

    // MARK: Loading

    func open(_ subject: SubjectID?, _ model: AppModel) async {
        guard subject != subjectID else {
            await reload(model)
            return
        }
        subjectID = subject
        checkpoints = []
        current = nil
        timeline = nil
        lanes = .empty
        timelineError = nil
        render = nil
        gallery = []
        galleryCheckpoint = nil
        selection = nil
        problem = nil
        job = nil
        loadedOtio = nil
        loadedRender = nil
        loadedGallery = nil
        player.replaceCurrentItem(with: nil)
        await reload(model)
    }

    func reload(_ model: AppModel) async {
        guard let subject = subjectID else { return }
        guard let list = await model.perform({ try await $0.checkpoints(subject: subject) }) else { return }
        guard subject == subjectID else { return }
        for checkpoint in list { known[checkpoint.id] = checkpoint }
        checkpoints = list.sorted { $0.id > $1.id }
        guard let head = checkpoints.first else {
            current = nil
            timeline = nil
            lanes = .empty
            return
        }
        current = await editCheckpoint(for: head, model)
        await loadTimeline(model)
        await loadRender(model)
        await loadGallery(model)
    }

    private func loadTimeline(_ model: AppModel) async {
        guard let hash = projectHash else {
            timeline = nil
            lanes = .empty
            timelineError = head == nil ? nil : "This checkpoint holds no project file."
            loadedOtio = nil
            return
        }
        guard hash != loadedOtio else { return }
        guard let url = await model.perform({ try await $0.objectPath(hash) }) else { return }
        do {
            let loaded = try ProjectTimeline.load(url)
            timeline = loaded
            lanes = ProjectLanes(loaded)
            timelineError = nil
            loadedOtio = hash
            if let selection, selection.end > lanes.totalFrames { self.selection = nil }
        } catch {
            timeline = nil
            lanes = .empty
            timelineError = "The project file could not be read: \(error.localizedDescription)"
        }
    }

    private func loadRender(_ model: AppModel) async {
        guard let found = await latest(label: "render", model), let hash = found.copy.outputs["out"] else {
            render = nil
            return
        }
        guard let url = await model.perform({ try await $0.objectPath(hash) }) else { return }
        var stale = false
        if let parent = found.copy.parent, let parentCheckpoint = await fetch(parent, model) {
            let made = await editCheckpoint(for: parentCheckpoint, model)
            stale = made?.outputs["out"] != projectHash
        }
        render = ProjectRender(checkpoint: found.listed.id, url: url, stale: stale)
        if url != loadedRender {
            loadedRender = url
            // Store objects have no extension: say what the file is.
            let asset = AVURLAsset(url: url, options: [AVURLAssetOverrideMIMETypeKey: "video/mp4"])
            player.replaceCurrentItem(with: AVPlayerItem(asset: asset))
        }
    }

    private func loadGallery(_ model: AppModel) async {
        guard let found = await latest(label: "qa.inspect-boundaries", model), let hash = found.copy.outputs["views"] else {
            gallery = []
            galleryCheckpoint = nil
            return
        }
        galleryCheckpoint = found.listed.id
        guard hash != loadedGallery else { return }
        let images: [ProjectQAImage]? = await model.perform { studio -> [ProjectQAImage] in
            var pictures: [ProjectQAImage] = []
            var pending: [(prefix: String, hash: String)] = [(prefix: "", hash: hash)]
            var visited = 0
            while let next = pending.popLast(), visited < 64 {
                visited += 1
                let manifestURL = try await studio.objectPath(next.hash)
                let manifest = try JSONDecoder().decode(ProjectTreeManifest.self, from: Data(contentsOf: manifestURL))
                for entry in manifest.entries {
                    let name = next.prefix + entry.name
                    if entry.kind == "tree" {
                        pending.append((prefix: name + "/", hash: entry.hash))
                    } else {
                        let lower = entry.name.lowercased()
                        if lower.hasSuffix(".png") || lower.hasSuffix(".jpg") || lower.hasSuffix(".jpeg") {
                            let url = try await studio.objectPath(entry.hash)
                            pictures.append(ProjectQAImage(id: entry.hash + name, name: name, url: url))
                        }
                    }
                }
            }
            return pictures.sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
        }
        if let images {
            gallery = images
            loadedGallery = hash
        }
    }

    // MARK: Resolving checkpoints

    private static func copiedFrom(_ label: String) -> CheckpointID? {
        for prefix in ["revert to #", "branch of #"] where label.hasPrefix(prefix) {
            return CheckpointID(label.dropFirst(prefix.count))
        }
        return nil
    }

    private func fetch(_ id: CheckpointID, _ model: AppModel) async -> Checkpoint? {
        if let checkpoint = known[id] { return checkpoint }
        guard let checkpoint = await model.perform({ try await $0.checkpoint(id) }) else { return nil }
        known[id] = checkpoint
        return checkpoint
    }

    /// The checkpoint a copy was made from (itself when it is not a copy).
    private func origin(of checkpoint: Checkpoint, _ model: AppModel) async -> Checkpoint {
        var at = checkpoint
        for _ in 0..<64 {
            guard let source = Self.copiedFrom(at.label), let next = await fetch(source, model) else { break }
            at = next
        }
        return at
    }

    /// The edit checkpoint that holds the project as it is at `checkpoint`.
    private func editCheckpoint(for checkpoint: Checkpoint, _ model: AppModel) async -> Checkpoint? {
        var at: Checkpoint? = checkpoint
        for _ in 0..<256 {
            guard let step = at else { return nil }
            let source = await origin(of: step, model)
            if source.outputs["out"] != nil && !ProjectOperation.nonProjectLabels.contains(source.label) {
                return source
            }
            guard let parent = source.parent else { return nil }
            at = await fetch(parent, model)
        }
        return nil
    }

    /// The newest checkpoint of this subject whose origin has `label`.
    private func latest(label: String, _ model: AppModel) async -> (listed: Checkpoint, copy: Checkpoint)? {
        for checkpoint in checkpoints {
            let source = await origin(of: checkpoint, model)
            if source.label == label { return (checkpoint, source) }
        }
        return nil
    }

    // MARK: Actions

    /// Submit `command` on this project with the head `.otio` as `project`.
    /// Returns the problem when the service refused it; the job is then
    /// watched in the background and the head reloaded when it succeeds.
    func submit(command: String, args: [String: JSONValue], inputs: [String: InputRef], keys: [String], _ model: AppModel) async -> ProjectFormProblem? {
        guard let subject = subjectID else {
            return ProjectFormProblem(command: command, key: nil, message: "Choose a project first.")
        }
        guard let hash = projectHash else {
            return ProjectFormProblem(command: command, key: nil, message: "The project has no .otio yet; wait for its first job.")
        }
        var all = inputs
        all["project"] = .hash(hash)
        return await start(subject: subject, command: command, args: args, inputs: all, keys: keys, model)
    }

    /// Submit any job on `subject` (also the project's first job).
    func start(subject: SubjectID, command: String, args: [String: JSONValue], inputs: [String: InputRef], keys: [String], _ model: AppModel) async -> ProjectFormProblem? {
        problem = nil
        var refusal: Error?
        let submitted = await model.perform { studio -> SubmitResult in
            do {
                return try await studio.submit(subject: subject, command: command, args: args, inputs: inputs)
            } catch {
                refusal = error
                throw error
            }
        }
        guard let submitted else {
            let message = refusal?.localizedDescription ?? "The job was not queued."
            let found = ProjectFormProblem(command: command, key: ProjectFormProblem.field(in: message, command: command, keys: keys), message: message)
            if let refusal, ErrorMeaning(refusal) == .badArgument { problem = found }
            return found
        }
        if subjectID != subject {
            await open(subject, model)
        }
        job = submitted.job
        await model.reloadJob(submitted.job)
        Task { await self.watch(submitted.job, subject: subject, command: command, keys: keys, model) }
        return nil
    }

    private func watch(_ id: JobID, subject: SubjectID, command: String, keys: [String], _ model: AppModel) async {
        guard let finished = await model.waitFor(id) else { return }
        guard subject == subjectID else { return }
        switch finished.status {
        case .succeeded:
            if command == ProjectOperation.punchIn.command { selection = nil }
            await reload(model)
        case .failed:
            let message = finished.error ?? "exit \(finished.exitCode.map { String($0) } ?? "?")"
            if finished.exitCode == 2 {
                problem = ProjectFormProblem(command: command, key: ProjectFormProblem.field(in: message, command: command, keys: keys), message: message)
                model.banner = "Check the highlighted field: \(message)"
            } else {
                model.banner = "\(command) failed: \(message)"
            }
        case .queued, .blocked, .running, .cancelled, .interrupted:
            break
        }
    }

    func undo(_ model: AppModel) async {
        guard let subject = subjectID, let target = undoTarget else { return }
        if await model.perform({ try await $0.revert(subject: subject, to: target) }) != nil {
            await reload(model)
        }
    }

    func revert(to checkpoint: CheckpointID, _ model: AppModel) async {
        guard let subject = subjectID else { return }
        if await model.perform({ try await $0.revert(subject: subject, to: checkpoint) }) != nil {
            await reload(model)
        }
    }

    /// "Try another version": a new project subject from the head.
    func branch(name: String, _ model: AppModel) async {
        guard let head else { return }
        guard let made = await model.perform({ try await $0.branch(from: head.id, name: name) }) else { return }
        await model.refreshSubjects()
        model.focusedSubject = made.0.id
        await open(made.0.id, model)
    }
}
