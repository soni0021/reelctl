import Foundation

/// The typed API of `reelctl serve` (docs/specs/studio-service.md, section 5).
/// Every screen goes through this; nothing builds a `reelctl` command line.
public final class Studio: Sendable {
    public let client: RPCClient

    public init(client: RPCClient) {
        self.client = client
    }

    private static func kindParams(_ kind: String?) -> JSONValue {
        guard let kind else { return .object([:]) }
        return .object(["kind": .string(kind)])
    }

    // MARK: Workspace and tools

    public func info() async throws -> WorkspaceInfo {
        try await client.call("workspace.info", as: WorkspaceInfo.self)
    }

    public func setLimits(heavy: Int? = nil, light: Int? = nil, network: Int? = nil) async throws -> Limits {
        var params: [String: JSONValue] = [:]
        if let heavy { params["heavy"] = .int(Int64(heavy)) }
        if let light { params["light"] = .int(Int64(light)) }
        if let network { params["network"] = .int(Int64(network)) }
        return try await client.call("workspace.set_limits", .object(params), as: Limits.self)
    }

    /// Tool name (`ffmpeg`, `ffprobe`, `ffmpeg_libraries`, `manimgx`, `smelter`,
    /// `onnx_runtime`, `whisper_model`, `transnet_weights`, `rvm_weights`) to what was found.
    public func doctor() async throws -> [String: DoctorItem] {
        try await client.call("doctor.check", as: [String: DoctorItem].self)
    }

    public func shutdown() async throws {
        _ = try await client.call("shutdown")
    }

    // MARK: Assets and subjects

    /// Import a file by reference (`copy: false`) or into the store.
    public func importAsset(_ url: URL, kind: String, name: String? = nil, copy: Bool = false) async throws -> Asset {
        var params: [String: JSONValue] = ["path": .string(url.path), "kind": .string(kind), "copy": .bool(copy)]
        if let name { params["name"] = .string(name) }
        return try await client.call("assets.import", .object(params), as: Asset.self)
    }

    public func assets(kind: String? = nil) async throws -> [Asset] {
        try await client.call("assets.list", Self.kindParams(kind), as: [Asset].self)
    }

    public func createSubject(kind: String, name: String) async throws -> Subject {
        try await client.call("subjects.create", ["kind": .string(kind), "name": .string(name)], as: Subject.self)
    }

    public func subjects(kind: String? = nil) async throws -> [Subject] {
        try await client.call("subjects.list", Self.kindParams(kind), as: [Subject].self)
    }

    /// A new subject starting from `checkpoint`.
    public func branch(from checkpoint: CheckpointID, name: String) async throws -> (Subject, Checkpoint) {
        struct Branched: Decodable { let subject: Subject; let checkpoint: Checkpoint }
        let result = try await client.call("subjects.branch", ["checkpoint": .int(checkpoint), "name": .string(name)], as: Branched.self)
        return (result.subject, result.checkpoint)
    }

    // MARK: Jobs

    /// Queue a catalogue command (`decompile`, `timeline.cut-silence`, `render`, ...)
    /// with typed `args` (the flag names with `_`) and its inputs by role.
    public func submit(
        subject: SubjectID,
        command: String,
        args: [String: JSONValue] = [:],
        inputs: [String: InputRef],
        after: [JobID] = [],
        priority: Int = 0,
        force: Bool = false
    ) async throws -> SubmitResult {
        let params: JSONValue = [
            "subject": .int(subject),
            "command": .string(command),
            "args": .object(args),
            "inputs": .object(inputs.mapValues(\.json)),
            "after": .array(after.map { .int($0) }),
            "priority": .int(Int64(priority)),
            "force": .bool(force),
        ]
        return try await client.call("jobs.submit", params, as: SubmitResult.self)
    }

    /// The job, with its latest events and its checkpoint.
    public func job(_ id: JobID) async throws -> Job {
        try await client.call("jobs.get", ["job": .int(id)], as: Job.self)
    }

    public func jobs(subject: SubjectID? = nil, status: JobStatus? = nil, limit: Int = 200) async throws -> [Job] {
        var params: [String: JSONValue] = ["limit": .int(Int64(limit))]
        if let subject { params["subject"] = .int(subject) }
        if let status { params["status"] = .string(status.rawValue) }
        return try await client.call("jobs.list", .object(params), as: [Job].self)
    }

    @discardableResult
    public func cancel(_ id: JobID) async throws -> JobStatus {
        struct Cancelled: Decodable { let status: JobStatus }
        return try await client.call("jobs.cancel", ["job": .int(id)], as: Cancelled.self).status
    }

    /// A new job with the same request (not for a model call).
    public func retry(_ id: JobID) async throws -> JobID {
        struct Retried: Decodable { let job: JobID }
        return try await client.call("jobs.retry", ["job": .int(id)], as: Retried.self).job
    }

    /// The job as a `reelctl` command line, inputs as their stored paths. Never
    /// holds a key.
    public func commandLine(_ id: JobID) async throws -> String {
        struct Line: Decodable { let argv: [String] }
        let line = try await client.call("jobs.argv", ["job": .int(id)], as: Line.self)
        return (["reelctl"] + line.argv).map(ShellQuote.quote).joined(separator: " ")
    }

    // MARK: Events

    /// Start pushing notifications on this connection. With `job`, that job's
    /// stored events after `sinceSeq` come first. Events arrive on the client's
    /// reader thread through `handler`.
    public func subscribe(job: JobID? = nil, sinceSeq: Int64 = 0, _ handler: @escaping @Sendable (StudioEvent) -> Void) async throws {
        client.onNotification { notification in
            if let event = StudioEvent(notification) { handler(event) }
        }
        var params: [String: JSONValue] = ["since_seq": .int(sinceSeq)]
        if let job { params["job"] = .int(job) }
        _ = try await client.call("events.subscribe", .object(params))
    }

    // MARK: Checkpoints and objects

    public func checkpoints(subject: SubjectID) async throws -> [Checkpoint] {
        try await client.call("checkpoints.list", ["subject": .int(subject)], as: [Checkpoint].self)
    }

    /// The checkpoint, with each output's stored path.
    public func checkpoint(_ id: CheckpointID) async throws -> Checkpoint {
        try await client.call("checkpoints.get", ["checkpoint": .int(id)], as: Checkpoint.self)
    }

    public func revert(subject: SubjectID, to checkpoint: CheckpointID) async throws -> Checkpoint {
        try await client.call("checkpoints.revert", ["subject": .int(subject), "checkpoint": .int(checkpoint)], as: Checkpoint.self)
    }

    public func diff(_ a: CheckpointID, _ b: CheckpointID) async throws -> CheckpointDiff {
        try await client.call("checkpoints.diff", ["a": .int(a), "b": .int(b)], as: CheckpointDiff.self)
    }

    /// The read-only path of a stored object.
    public func objectPath(_ hash: String) async throws -> URL {
        struct Path: Decodable { let path: String }
        return URL(fileURLWithPath: try await client.call("objects.path", ["hash": .string(hash)], as: Path.self).path)
    }

    public func stats() async throws -> StoreStats {
        try await client.call("store.stats", as: StoreStats.self)
    }

    /// Garbage collection: a dry run unless `dryRun` is false.
    public func collectGarbage(dryRun: Bool = true) async throws -> GarbageReport {
        try await client.call("store.gc", ["dry_run": .bool(dryRun)], as: GarbageReport.self)
    }

    // MARK: The paid call

    /// The plan of an `understand` call: its dry run, no key, no spend.
    public func planUnderstand(
        subject: SubjectID,
        input: InputRef,
        dna: InputRef,
        model: String? = nil,
        everyS: Int? = nil,
        options: [String: JSONValue] = [:]
    ) async throws -> UnderstandPlan {
        var params = options
        params["subject"] = .int(subject)
        params["input"] = input.json
        params["dna"] = dna.json
        if let model { params["model"] = .string(model) }
        if let everyS { params["every_s"] = .int(Int64(everyS)) }
        return try await client.call("understand.plan", .object(params), as: UnderstandPlan.self)
    }

    /// The live call, after the user approved exactly `plan.costUpperBoundUsd`.
    public func runUnderstand(plan: UnderstandPlan, approvedMaxUsd: Double, apiKey: String) async throws -> JobID {
        struct Run: Decodable { let job: JobID }
        let params: JSONValue = [
            "plan": .int(plan.plan),
            "approved_max_usd": .double(approvedMaxUsd),
            "api_key": .string(apiKey),
        ]
        return try await client.call("understand.run", params, as: Run.self).job
    }

    // MARK: Make reels

    public func makeReels(_ request: MakeReelsRequest) async throws -> MakeReelsStage {
        try await client.call("pipelines.make_reels", request.json, as: MakeReelsStage.self)
    }
}

/// `pipelines.make_reels`'s params: as `reelctl make-reels`'s flags.
public struct MakeReelsRequest: Sendable, Hashable {
    public var subject: SubjectID?
    public var name: String?
    public var input: InputRef
    public var outliers: [InputRef]
    public var targetURL: String
    /// Skip decompile.
    public var dna: InputRef?
    /// Skip the plan and the call.
    public var understanding: InputRef?
    public var whisperModel: String?
    public var model: String?
    public var everyS: Int?
    public var maxCostUsd: Double?
    public var clips: [Int] = []
    public var reels: Int?
    public var allowFlagged = false
    /// `presenter` or `centre`.
    public var follow = "presenter"
    public var weights: String?
    public var cardMs: Int?
    public var size: String?
    /// `overlay captions` arguments.
    public var style: [String: JSONValue] = [:]

    public init(input: InputRef, outliers: [InputRef], targetURL: String) {
        self.input = input
        self.outliers = outliers
        self.targetURL = targetURL
    }

    public var json: JSONValue {
        var params: [String: JSONValue] = [
            "input": input.json,
            "outliers": .array(outliers.map(\.json)),
            "target_url": .string(targetURL),
            "allow_flagged": .bool(allowFlagged),
            "follow": .string(follow),
            "style": .object(style),
        ]
        if let subject { params["subject"] = .int(subject) }
        if let name { params["name"] = .string(name) }
        if let dna { params["dna"] = dna.json }
        if let understanding { params["understanding"] = understanding.json }
        if let whisperModel { params["whisper_model"] = .string(whisperModel) }
        if let model { params["model"] = .string(model) }
        if let everyS { params["every_s"] = .int(Int64(everyS)) }
        if let maxCostUsd { params["max_cost_usd"] = .double(maxCostUsd) }
        if !clips.isEmpty { params["clips"] = .array(clips.map { .int(Int64($0)) }) }
        if let reels { params["reels"] = .int(Int64(reels)) }
        if let weights { params["weights"] = .string(weights) }
        if let cardMs { params["card_ms"] = .int(Int64(cardMs)) }
        if let size { params["size"] = .string(size) }
        return .object(params)
    }
}

/// Where `pipelines.make_reels` got to.
public struct MakeReelsStage: Codable, Sendable, Hashable {
    /// `decompile`, `plan` or `reels`.
    public let stage: String
    public let subject: SubjectID
    /// `decompile`: the decompile job; call again with `dna: .fromJob(job, "out")`.
    public let jobs: [JobID]?
    /// `plan`: approve it with `runUnderstand`, then call again with
    /// `understanding: .fromJob(job, "out")`.
    public let plan: UnderstandPlan?
    /// `reels`: one per clip.
    public let reels: [ReelJobs]?
    public let follow: String?
    public let notes: [String]?
}

public struct ReelJobs: Codable, Sendable, Hashable {
    public let clip: Int
    public let subject: SubjectID
    public let jobs: [StepJob]
}

public struct StepJob: Codable, Sendable, Hashable {
    /// `from-clip`, `cut-silence`, `auto-punch-in`, `reframe`, `bridge`,
    /// `captions`, `render`, `diff`.
    public let step: String
    public let job: JobID
}

/// POSIX shell quoting for "Copy as CLI command".
public enum ShellQuote {
    public static func quote(_ word: String) -> String {
        let safe = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_./=:,+@%"))
        if !word.isEmpty, word.unicodeScalars.allSatisfy({ safe.contains($0) }) { return word }
        return "'" + word.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}
