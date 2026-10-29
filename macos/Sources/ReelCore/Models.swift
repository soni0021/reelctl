import Foundation

// The service's records (docs/specs/studio-service.md, section 5), as Swift
// types. Keys are snake_case on the wire, spelled out in each `CodingKeys`.

public typealias JobID = Int64
public typealias SubjectID = Int64
public typealias CheckpointID = Int64
public typealias AssetID = Int64

public enum JobStatus: String, Codable, Sendable, CaseIterable {
    case queued, blocked, running, succeeded, failed, cancelled, interrupted

    public var finished: Bool {
        switch self {
        case .succeeded, .failed, .cancelled, .interrupted: return true
        case .queued, .blocked, .running: return false
        }
    }
}

public enum JobClass: String, Codable, Sendable, CaseIterable {
    case heavy, light, network
}

/// Where a job input comes from.
public enum InputRef: Sendable, Hashable {
    /// A stored object.
    case hash(String)
    /// An output of an earlier job (which then runs first).
    case fromJob(JobID, role: String)
    /// An imported file, read where it is.
    case asset(AssetID)

    public var json: JSONValue {
        switch self {
        case .hash(let hash): return .string(hash)
        case .fromJob(let job, let role): return ["from": .int(job), "role": .string(role)]
        case .asset(let asset): return ["asset": .int(asset)]
        }
    }

    public init?(json: JSONValue) {
        if let hash = json.string {
            self = .hash(hash)
        } else if let job = json["from"].int, let role = json["role"].string {
            self = .fromJob(job, role: role)
        } else if let asset = json["asset"].int {
            self = .asset(asset)
        } else {
            return nil
        }
    }
}

public struct JobEvent: Codable, Sendable, Hashable {
    public let job: JobID
    public let seq: Int64
    public let ts: Int64
    public let event: JSONValue
}

public struct Job: Codable, Identifiable, Sendable, Hashable {
    public let id: JobID
    public let subject: SubjectID
    public let command: String
    public let args: JSONValue
    public let inputs: [String: JSONValue]
    public let jobClass: JobClass
    public let priority: Int64
    public var status: JobStatus
    public let after: [JobID]
    public let cacheKey: String
    public let exitCode: Int?
    public let result: JSONValue?
    public let error: String?
    public let envNames: [String]
    public let createdAt: Int64
    public let startedAt: Int64?
    public let finishedAt: Int64?
    public let force: Bool
    /// From `jobs.get` only: the latest events.
    public let events: [JobEvent]?
    /// From `jobs.get` only: the checkpoint the job made.
    public let checkpoint: Checkpoint?

    enum CodingKeys: String, CodingKey {
        case id, subject, command, args, inputs, priority, status, after, error, force, events, checkpoint
        case jobClass = "class"
        case cacheKey = "cache_key"
        case exitCode = "exit_code"
        case result
        case envNames = "env_names"
        case createdAt = "created_at"
        case startedAt = "started_at"
        case finishedAt = "finished_at"
    }

    /// The step name the service and make-reels use (`timeline.cut-silence` → `cut-silence`).
    public var shortCommand: String {
        command.split(separator: ".").last.map(String.init) ?? command
    }
}

public struct Subject: Codable, Identifiable, Sendable, Hashable {
    public let id: SubjectID
    public let kind: String
    public let name: String
    public let parentSubject: SubjectID?
    public let headCheckpoint: CheckpointID?
    public let createdAt: Int64

    enum CodingKeys: String, CodingKey {
        case id, kind, name
        case parentSubject = "parent_subject"
        case headCheckpoint = "head_checkpoint"
        case createdAt = "created_at"
    }

    public static let kinds = ["reel", "project", "look", "presenter", "understanding", "dataset", "eval", "make_reels"]
}

/// A stored output, from `checkpoints.get`.
public struct StoredFile: Codable, Sendable, Hashable {
    public let hash: String
    /// `file` or `tree` (a folder).
    public let kind: String?
    public let path: String

    public var url: URL { URL(fileURLWithPath: path) }
    public var isFolder: Bool { kind == "tree" }
}

public struct Checkpoint: Codable, Identifiable, Sendable, Hashable {
    public let id: CheckpointID
    public let subject: SubjectID
    public let parent: CheckpointID?
    public let job: JobID?
    public let label: String
    /// Output role to object hash.
    public let outputs: [String: String]
    public let cached: Bool
    public let note: String
    public let createdAt: Int64
    /// From `checkpoints.get` only.
    public let files: [String: StoredFile]?

    enum CodingKeys: String, CodingKey {
        case id, subject, parent, job, label, outputs, cached, note, files
        case createdAt = "created_at"
    }
}

public struct CheckpointDiff: Codable, Sendable, Hashable {
    public let same: [String]
    public let changed: [String]
    public let onlyInA: [String]
    public let onlyInB: [String]

    enum CodingKeys: String, CodingKey {
        case same, changed
        case onlyInA = "only_in_a"
        case onlyInB = "only_in_b"
    }
}

public struct Asset: Codable, Identifiable, Sendable, Hashable {
    public let id: AssetID
    public let kind: String
    public let name: String
    public let sourcePath: String
    public let size: UInt64
    public let mtime: Int64
    public let hash: String
    public let copied: Bool
    public let createdAt: Int64

    enum CodingKeys: String, CodingKey {
        case id, kind, name, size, mtime, hash, copied
        case sourcePath = "source_path"
        case createdAt = "created_at"
    }

    public var url: URL { URL(fileURLWithPath: sourcePath) }

    public static let kinds = ["video", "edit_dna", "understanding", "otio", "chart_spec", "cube", "jsonl", "other"]
}

public struct Limits: Codable, Sendable, Hashable {
    public var heavy: Int
    public var light: Int
    public var network: Int

    public init(heavy: Int, light: Int, network: Int) {
        self.heavy = heavy
        self.light = light
        self.network = network
    }
}

public struct WorkspaceInfo: Codable, Sendable, Hashable {
    public let path: String
    public let version: String
    public let schemaVersion: Int
    public let limits: Limits
    public let pid: Int

    enum CodingKeys: String, CodingKey {
        case path, version, limits, pid
        case schemaVersion = "schema_version"
    }
}

/// One line of `doctor.check`.
public struct DoctorItem: Codable, Sendable, Hashable {
    public let found: Bool
    public let path: String?
    public let version: String?
    /// The environment variable that names it, for files.
    public let variable: String?
}

public struct SubjectBytes: Codable, Sendable, Hashable {
    public let subject: SubjectID
    public let name: String
    public let kind: String
    public let bytes: UInt64
}

public struct StoreStats: Codable, Sendable, Hashable {
    public let objects: Int
    public let bytes: UInt64
    public let byKind: [String: UInt64]
    public let bySubject: [SubjectBytes]

    enum CodingKeys: String, CodingKey {
        case objects, bytes
        case byKind = "by_kind"
        case bySubject = "by_subject"
    }
}

public struct GarbageReport: Codable, Sendable, Hashable {
    public let dryRun: Bool
    public let objects: Int
    public let bytes: UInt64
    public let hashes: [String]

    enum CodingKeys: String, CodingKey {
        case objects, bytes, hashes
        case dryRun = "dry_run"
    }
}

public struct SubmitResult: Codable, Sendable, Hashable {
    public let job: JobID
    public let status: JobStatus
    public let cached: Bool?
}

public struct UnderstandPlan: Codable, Sendable, Hashable {
    public let plan: JobID
    public let costUpperBoundUsd: Double
    public let frames: Int
    public let images: Int
    public let expiresAt: Int64

    enum CodingKeys: String, CodingKey {
        case plan, frames, images
        case costUpperBoundUsd = "cost_upper_bound_usd"
        case expiresAt = "expires_at"
    }

    /// The amount to show and approve, exactly as the service will compare it.
    public var approvalText: String { String(format: "$%.4f", costUpperBoundUsd) }
}

/// A pushed notification, typed.
public enum StudioEvent: Sendable, Equatable {
    case jobStatus(job: JobID, status: JobStatus)
    /// A progress event (`{"event":"progress","stage":..,"done":..,"total":..}`).
    case jobProgress(job: JobID, seq: Int64, event: JSONValue)
    /// Any other event of a job: stage, warning, result, error, log.
    case jobEvent(job: JobID, seq: Int64, event: JSONValue)
    case checkpointCreated(subject: SubjectID, checkpoint: CheckpointID, job: JobID, cached: Bool)

    public init?(_ notification: RPCNotification) {
        let params = notification.params
        switch notification.method {
        case "job.status":
            guard let job = params["job"].int, let status = params["status"].string.flatMap(JobStatus.init(rawValue:)) else { return nil }
            self = .jobStatus(job: job, status: status)
        case "job.progress":
            guard let job = params["job"].int, let seq = params["seq"].int else { return nil }
            self = .jobProgress(job: job, seq: seq, event: params["event"])
        case "job.event":
            guard let job = params["job"].int, let seq = params["seq"].int else { return nil }
            self = .jobEvent(job: job, seq: seq, event: params["event"])
        case "checkpoint.created":
            guard let subject = params["subject"].int, let checkpoint = params["checkpoint"].int, let job = params["job"].int else { return nil }
            self = .checkpointCreated(subject: subject, checkpoint: checkpoint, job: job, cached: params["cached"].bool ?? false)
        default:
            return nil
        }
    }

    public var job: JobID {
        switch self {
        case .jobStatus(let job, _), .jobProgress(let job, _, _), .jobEvent(let job, _, _), .checkpointCreated(_, _, let job, _):
            return job
        }
    }
}

/// A progress event's numbers, for a bar.
public struct JobProgress: Sendable, Hashable {
    public let stage: String
    public let done: UInt64
    public let total: UInt64?

    public init?(event: JSONValue) {
        guard event["event"].string == "progress", let stage = event["stage"].string, let done = event["done"].int else { return nil }
        self.stage = stage
        self.done = UInt64(max(0, done))
        self.total = event["total"].int.map { UInt64(max(0, $0)) }
    }

    public var fraction: Double? {
        guard let total, total > 0 else { return nil }
        return min(1, Double(done) / Double(total))
    }
}
