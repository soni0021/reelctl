import Foundation
import ReelCore

/// Helpers the Library and Compare screens share: the `reel` subject of a
/// file, the latest checkpoint holding given outputs, and frame-exact times.
enum AnalysisSupport {
    /// The subject name Compare's `diff` runs go on, kept apart from the
    /// draft's own `reel` subject so a report never hides its edit DNA.
    static func compareSubjectName(_ draft: Asset) -> String { "compare · " + draft.name }

    static func isCompareSubject(_ subject: Subject) -> Bool { subject.name.hasPrefix("compare · ") }

    /// `whisper_model` from Settings, as an absolute path, when one is set.
    static func whisperArgs() -> [String: JSONValue] {
        let raw = ToolSetting.whisperModel.value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !raw.isEmpty else { return [:] }
        let path = (raw as NSString).expandingTildeInPath
        guard path.hasPrefix("/") else { return [:] }
        return ["whisper_model": .string(path)]
    }

    /// The millisecond inside frame `frame` (its middle), so a zero-tolerance
    /// seek shows exactly that frame even at 29.97 fps.
    static func frameMs(_ frame: Int64, fps: Double, fallback: Int64) -> Int64 {
        guard fps > 0, frame >= 0 else { return fallback }
        return Int64(((Double(frame) + 0.5) * 1000 / fps).rounded(.down))
    }

    /// The frame whose start is nearest `ms` (times in edit_dna.json are rounded).
    static func nearestFrame(_ ms: Int64, fps: Double) -> Int64 {
        guard fps > 0 else { return -1 }
        return Int64((Double(ms) * fps / 1000).rounded())
    }

    /// The newest `reel` subject with this name.
    @MainActor
    static func reelSubject(model: AppModel, name: String) -> Subject? {
        model.subjects(kind: "reel").filter { $0.name == name }.max { $0.id < $1.id }
    }

    /// The `reel` subject with this name, created when there is none.
    @MainActor
    static func ensureReelSubject(model: AppModel, name: String) async -> Subject? {
        if let found = reelSubject(model: model, name: name) { return found }
        let created: Subject? = await model.perform { try await $0.createSubject(kind: "reel", name: name) }
        if created != nil { await model.refreshSubjects() }
        return created
    }

    /// A queued or running job of `command` on `subject`.
    @MainActor
    static func activeJob(model: AppModel, subject: SubjectID, command: String) -> Job? {
        model.jobs.first { $0.subject == subject && $0.command == command && !$0.status.finished }
    }

    /// The subject's checkpoint holding every role in `roles` (the head when it
    /// does, else the newest), fetched with its stored paths.
    @MainActor
    static func latestCheckpoint(model: AppModel, subject: SubjectID, roles: [String]) async -> Checkpoint? {
        let head: CheckpointID? = model.subject(subject)?.headCheckpoint
        let found: Checkpoint?? = await model.perform { (studio: Studio) -> Checkpoint? in
            let list = try await studio.checkpoints(subject: subject)
            let matching = list.filter { checkpoint in roles.allSatisfy { checkpoint.outputs[$0] != nil } }
            let chosen = matching.first { $0.id == head } ?? matching.max { $0.id < $1.id }
            guard let chosen else { return nil }
            return try await studio.checkpoint(chosen.id)
        }
        return found ?? nil
    }

    static func number(_ value: Double, _ digits: Int = 2) -> String {
        String(format: "%.\(digits)f", value)
    }
}
