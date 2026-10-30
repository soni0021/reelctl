import Foundation

/// A rendered reel found in the workspace: its subject, which clip of the
/// source it is, the checkpoint and stored file of its newest render, and the
/// diff report beside it when there is one.
public struct ReelRef: Equatable, Sendable, Identifiable {
    public let subject: SubjectID
    public let name: String
    /// From a subject name like `clip 3`; nil when the name has no number.
    public let clip: Int?
    public let renderCheckpoint: CheckpointID
    public let renderHash: String
    public let diffReportHash: String?

    public var id: SubjectID { subject }
}

/// The reels made from one source video. `source` is the asset id of the long
/// video, or nil when the reel's `timeline.from-clip` job is not known.
public struct SourceGroup: Equatable, Sendable, Identifiable {
    public let source: AssetID?
    public let reels: [ReelRef]

    public var id: Int64 { source ?? -1 }
}

/// What the Reels screen shows, read from what the service already lists:
/// subjects, their checkpoints and their jobs. Nothing is stored for it.
public enum ReelOutputs {
    /// A reel is a subject of kind `reel` with a `render` checkpoint that has an
    /// `out`. Its source is the asset its `timeline.from-clip` job read. Groups
    /// are newest render first, with the unknown source last; reels run in clip
    /// order (those without a clip number after, by subject).
    public static func group(
        subjects: [Subject],
        checkpoints: [SubjectID: [Checkpoint]],
        jobs: [SubjectID: [Job]]
    ) -> [SourceGroup] {
        var bySource: [AssetID?: [ReelRef]] = [:]
        for subject in subjects where subject.kind == "reel" {
            let list = checkpoints[subject.id] ?? []
            guard let render = list.filter({ $0.label == "render" && $0.outputs["out"] != nil }).max(by: { $0.id < $1.id }),
                  let hash = render.outputs["out"]
            else { continue }
            let report = list.filter { $0.label == "diff" }.max(by: { $0.id < $1.id })?.outputs["report"]
            let source = (jobs[subject.id] ?? [])
                .first { $0.command == "timeline.from-clip" }
                .flatMap { $0.inputs["input"]?["asset"].int }
            bySource[source, default: []].append(ReelRef(
                subject: subject.id,
                name: subject.name,
                clip: clipNumber(subject.name),
                renderCheckpoint: render.id,
                renderHash: hash,
                diffReportHash: report
            ))
        }
        let groups = bySource.map { source, reels in
            SourceGroup(source: source, reels: reels.sorted {
                ($0.clip ?? Int.max, $0.subject) < ($1.clip ?? Int.max, $1.subject)
            })
        }
        func newest(_ group: SourceGroup) -> CheckpointID { group.reels.map(\.renderCheckpoint).max() ?? 0 }
        return groups.sorted {
            if ($0.source == nil) != ($1.source == nil) { return $0.source != nil }
            return newest($0) > newest($1)
        }
    }

    /// The number in a subject name such as `clip 3`, the first run of digits.
    public static func clipNumber(_ name: String) -> Int? {
        let digits = name.split(whereSeparator: { !$0.isNumber }).first
        return digits.flatMap { Int($0) }
    }
}
