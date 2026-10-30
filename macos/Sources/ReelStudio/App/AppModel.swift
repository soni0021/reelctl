import Foundation
import Observation
import ReelCore

/// The sidebar sections (docs/specs/mac-app.md, "Structure").
enum AppSection: String, CaseIterable, Identifiable, Hashable {
    case library, makeReels, reels, projects, compare, look, presenter, understand, datasets, overlays
    case jobs, history, storage, settings

    var id: String { rawValue }

    var title: String {
        switch self {
        case .library: return "Library"
        case .makeReels: return "Make reels"
        case .reels: return "Reels"
        case .projects: return "Projects"
        case .compare: return "Compare"
        case .look: return "Look"
        case .presenter: return "Presenter"
        case .understand: return "Understand"
        case .datasets: return "Datasets"
        case .overlays: return "Overlays"
        case .jobs: return "Jobs"
        case .history: return "History"
        case .storage: return "Storage"
        case .settings: return "Settings"
        }
    }

    var symbol: String {
        switch self {
        case .library: return "film.stack"
        case .makeReels: return "wand.and.stars"
        case .reels: return "film"
        case .projects: return "timeline.selection"
        case .compare: return "square.split.2x1"
        case .look: return "camera.filters"
        case .presenter: return "person.crop.rectangle"
        case .understand: return "text.magnifyingglass"
        case .datasets: return "tablecells"
        case .overlays: return "chart.bar.xaxis"
        case .jobs: return "list.bullet.rectangle"
        case .history: return "clock.arrow.circlepath"
        case .storage: return "internaldrive"
        case .settings: return "gearshape"
        }
    }

    static let work: [AppSection] = [.library, .makeReels, .reels, .projects, .compare, .look, .presenter, .understand, .datasets, .overlays]
    static let service: [AppSection] = [.jobs, .history, .storage, .settings]
}

/// The tool paths Settings sets, passed to `reelctl serve` as environment
/// variables. Kept in UserDefaults (paths only; never the key).
enum ToolSetting: String, CaseIterable, Identifiable {
    case manimgx = "MANIMGX_BIN"
    case smelter = "SMELTER_BIN"
    case onnxRuntime = "ORT_DYLIB_PATH"
    case whisperModel = "REELCTL_WHISPER_MODEL"
    case transnetWeights = "REELCTL_TRANSNET_WEIGHTS"
    case rvmWeights = "REELCTL_RVM_WEIGHTS"

    var id: String { rawValue }

    /// The `doctor.check` entry it shows as.
    var doctorKey: String {
        switch self {
        case .manimgx: return "manimgx"
        case .smelter: return "smelter"
        case .onnxRuntime: return "onnx_runtime"
        case .whisperModel: return "whisper_model"
        case .transnetWeights: return "transnet_weights"
        case .rvmWeights: return "rvm_weights"
        }
    }

    var title: String {
        switch self {
        case .manimgx: return "manimgx"
        case .smelter: return "Smelter"
        case .onnxRuntime: return "ONNX Runtime library"
        case .whisperModel: return "Whisper model"
        case .transnetWeights: return "TransNetV2 weights"
        case .rvmWeights: return "RobustVideoMatting weights"
        }
    }

    /// Where the model is usually kept, filled in once when the setting has never
    /// been touched and the file is there. Only the whisper model: the others
    /// change what `reelctl` does or need a runtime library, so they stay explicit.
    private var usualPath: String? {
        switch self {
        case .whisperModel:
            return NSString(string: "~/models/whisper/ggml-apex-hinglish-q8_0.bin").expandingTildeInPath
        default:
            return nil
        }
    }

    /// An empty string means "set to nothing" (Clear keeps it so); a setting that
    /// was never touched takes its usual path when that file exists.
    var value: String {
        get {
            let key = "tool." + rawValue
            if let stored = UserDefaults.standard.string(forKey: key) { return stored }
            if let path = usualPath, FileManager.default.fileExists(atPath: path) {
                UserDefaults.standard.set(path, forKey: key)
                return path
            }
            return ""
        }
        nonmutating set { UserDefaults.standard.set(newValue, forKey: "tool." + rawValue) }
    }

    static var environment: [String: String] {
        Dictionary(uniqueKeysWithValues: allCases.map { ($0.rawValue, $0.value) })
    }
}

/// What the app knows: the connection, the workspace, subjects, jobs and their
/// live events. One instance, in the environment. Updated on the main actor.
@MainActor
@Observable
final class AppModel {
    enum Connection: Equatable {
        case disconnected
        case connecting
        case connected
        case failed(String)
    }

    var connection: Connection = .disconnected
    var info: WorkspaceInfo?
    var subjects: [Subject] = []
    var assets: [Asset] = []
    /// Newest first.
    var jobs: [Job] = []
    /// The latest progress of each running job.
    var progress: [JobID: JobProgress] = [:]
    /// The latest few non-progress events of each job (log lines, warnings, stages).
    var recentEvents: [JobID: [JSONValue]] = [:]
    /// Bumped on every checkpoint, so screens showing history reload.
    var checkpointTick = 0
    var selection: AppSection? = .library
    /// A subject chosen on one screen and shown on another (History, Projects).
    var focusedSubject: SubjectID?
    /// An error to show in the window's banner.
    var banner: String?
    var showPalette = false
    var showOnboarding = !UserDefaults.standard.bool(forKey: "onboarded")

    private(set) var studio: Studio?
    /// The connection that receives notifications (kept apart from calls).
    private var events: Studio?

    var workspace: URL {
        get {
            if let path = UserDefaults.standard.string(forKey: "workspace") { return URL(fileURLWithPath: path) }
            let home = FileManager.default.homeDirectoryForCurrentUser
            return home.appendingPathComponent("Movies/ReelStudio")
        }
        set { UserDefaults.standard.set(newValue.path, forKey: "workspace") }
    }

    var isConnected: Bool { connection == .connected }

    /// Connect to (or start) the workspace's service, load the lists and subscribe.
    func connect() async {
        connection = .connecting
        do {
            let reelctl = Daemon.bundledReelctl()
            let calls = try await Daemon.connect(workspace: workspace, reelctl: reelctl, tools: ToolSetting.environment)
            let pushes = try await Daemon.connect(workspace: workspace, reelctl: reelctl, tools: ToolSetting.environment)
            studio = Studio(client: calls)
            events = Studio(client: pushes)
            try await events?.subscribe { [weak self] event in
                Task { @MainActor [weak self] in self?.apply(event) }
            }
            connection = .connected
            await refresh()
        } catch {
            connection = .failed(error.localizedDescription)
        }
    }

    /// Stop the service when no job is queued or running (spec: the app sends
    /// `shutdown` on quit only then).
    func quit() async {
        guard let studio else { return }
        let busy = jobs.contains { !$0.status.finished }
        if !busy { try? await studio.shutdown() }
        studio.client.close()
        events?.client.close()
    }

    /// Restart the service with new tool paths, when nothing runs.
    func restartService() async -> Bool {
        guard !jobs.contains(where: { !$0.status.finished }) else {
            banner = "Jobs are running; the new tool paths apply after they finish and the service restarts."
            return false
        }
        try? await studio?.shutdown()
        studio?.client.close()
        events?.client.close()
        studio = nil
        events = nil
        try? await Task.sleep(nanoseconds: 500_000_000)
        await connect()
        return isConnected
    }

    func refresh() async {
        guard let studio else { return }
        do {
            async let info = studio.info()
            async let subjects = studio.subjects()
            async let assets = studio.assets()
            async let jobs = studio.jobs(limit: 500)
            self.info = try await info
            self.subjects = try await subjects
            self.assets = try await assets
            self.jobs = try await jobs
        } catch {
            report(error)
        }
    }

    func apply(_ event: StudioEvent) {
        switch event {
        case .jobStatus(let id, let status):
            if let index = jobs.firstIndex(where: { $0.id == id }) {
                jobs[index].status = status
                if status.finished {
                    progress.removeValue(forKey: id)
                    Task { await reloadJob(id) }
                }
            } else {
                Task { await reloadJob(id) }
            }
        case .jobProgress(let id, _, let value):
            if let value = JobProgress(event: value) { progress[id] = value }
        case .jobEvent(let id, _, let value):
            var list = recentEvents[id] ?? []
            list.append(value)
            if list.count > 40 { list.removeFirst(list.count - 40) }
            recentEvents[id] = list
        case .checkpointCreated:
            checkpointTick += 1
            Task { await refreshSubjects() }
        }
    }

    func reloadJob(_ id: JobID) async {
        guard let studio, let job = try? await studio.job(id) else { return }
        if let index = jobs.firstIndex(where: { $0.id == id }) {
            jobs[index] = job
        } else {
            jobs.insert(job, at: 0)
        }
    }

    func refreshSubjects() async {
        guard let studio else { return }
        if let subjects = try? await studio.subjects() { self.subjects = subjects }
        if let assets = try? await studio.assets() { self.assets = assets }
    }

    func job(_ id: JobID) -> Job? { jobs.first { $0.id == id } }

    func subject(_ id: SubjectID) -> Subject? { subjects.first { $0.id == id } }

    func subjects(kind: String) -> [Subject] { subjects.filter { $0.kind == kind } }

    /// Show an error. A key or cost error also opens the place to fix it.
    func report(_ error: Error) {
        switch ErrorMeaning(error) {
        case .needsKey:
            banner = "No API key: set it in Settings. (\(error.localizedDescription))"
            selection = .settings
        case .overBudget:
            banner = "Over the approved cost: \(error.localizedDescription)"
        case .badArgument:
            banner = "Check the highlighted field: \(error.localizedDescription)"
        case .other:
            banner = error.localizedDescription
        }
    }

    /// Run `body` with the studio, reporting any error. Returns nil on failure.
    @discardableResult
    func perform<T>(_ body: (Studio) async throws -> T) async -> T? {
        guard let studio else {
            banner = "Not connected to reelctl serve."
            return nil
        }
        do {
            return try await body(studio)
        } catch {
            report(error)
            return nil
        }
    }

    /// Import a file (by reference) or return the asset already imported.
    func importFile(_ url: URL, kind: String) async -> Asset? {
        let asset = await perform { try await $0.importAsset(url, kind: kind) }
        if asset != nil { await refreshSubjects() }
        return asset
    }

    /// Wait for a job to finish (through the live status), then reload it.
    func waitFor(_ id: JobID) async -> Job? {
        while true {
            if let job = job(id), job.status.finished {
                await reloadJob(id)
                return self.job(id)
            }
            if job(id) == nil { await reloadJob(id) }
            try? await Task.sleep(nanoseconds: 250_000_000)
            if Task.isCancelled { return nil }
        }
    }

    /// The stored file of a job's output role, once it succeeded.
    func output(of job: JobID, role: String) async -> StoredFile? {
        await perform { studio -> StoredFile? in
            let found = try await studio.job(job)
            guard let checkpoint = found.checkpoint else { return nil }
            return try await studio.checkpoint(checkpoint.id).files?[role]
        } ?? nil
    }

    /// The stored file of a subject's head checkpoint.
    /// The edit DNA of a subject: the `out` of its newest decompile checkpoint. The
    /// head is not always the DNA: a make-reels subject's head is its understanding.
    func dnaFile(of subject: SubjectID) async -> StoredFile? {
        await perform { studio -> StoredFile? in
            let decompiles = try await studio.checkpoints(subject: subject)
                .filter { $0.label == "decompile" && $0.outputs["out"] != nil }
            guard let newest = decompiles.max(by: { $0.id < $1.id }) else { return nil }
            return try await studio.checkpoint(newest.id).files?["out"]
        } ?? nil
    }

    func headFile(of subject: SubjectID, role: String) async -> StoredFile? {
        await perform { studio -> StoredFile? in
            guard let head = try await studio.subjects().first(where: { $0.id == subject })?.headCheckpoint else { return nil }
            return try await studio.checkpoint(head).files?[role]
        } ?? nil
    }
}
