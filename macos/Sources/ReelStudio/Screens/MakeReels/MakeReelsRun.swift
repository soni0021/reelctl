import Foundation
import Observation
import ReelCore

/// Where the Make reels screen is, from the form to the reels.
enum MakeReelsPhase: Int, CaseIterable, Comparable {
    case setup, decompile, plan, understand, reels

    static func < (lhs: MakeReelsPhase, rhs: MakeReelsPhase) -> Bool { lhs.rawValue < rhs.rawValue }

    var title: String {
        switch self {
        case .setup: return "Set up"
        case .decompile: return "Decompile"
        case .plan: return "Plan and approve"
        case .understand: return "Understand"
        case .reels: return "Reels"
        }
    }
}

/// One make-reels run: the form, the request as it grows (`dna`, then
/// `understanding`), and the stage the service last answered. One shared
/// instance, so the run survives leaving the screen; its tasks are not tied to
/// the view.
@MainActor
@Observable
final class MakeReelsRun {
    static let shared = MakeReelsRun()

    var draft = MakeReelsDraft()
    /// The request sent to `pipelines.make_reels`, with what earlier stages added.
    private(set) var request: MakeReelsRequest?
    private(set) var stage: MakeReelsStage?
    private(set) var phase: MakeReelsPhase = .setup
    private(set) var decompileJob: JobID?
    private(set) var understandJob: JobID?
    /// The plan waiting for a yes in the paid-call sheet.
    var pendingPlan: UnderstandPlan?
    /// The last plan shown, and what the user said.
    private(set) var lastPlan: UnderstandPlan?
    private(set) var approvedUsd: Double?
    private(set) var declined = false
    private(set) var busy = false
    private(set) var message: String?

    /// A stage is being driven or a plan waits for an answer.
    var isRunning: Bool { busy || pendingPlan != nil }

    /// Forget the run, keep the form.
    func reset() {
        guard !busy else { return }
        request = nil
        stage = nil
        phase = .setup
        decompileJob = nil
        understandJob = nil
        pendingPlan = nil
        lastPlan = nil
        approvedUsd = nil
        declined = false
        message = nil
    }

    func start(model: AppModel) {
        guard !busy else { return }
        Task { await self.begin(model: model) }
    }

    /// After a declined or expired plan: ask the service for a new one.
    func askAgain(model: AppModel) {
        guard !busy, request != nil else { return }
        declined = false
        message = nil
        Task { await self.drive(model: model) }
    }

    /// The user said yes to exactly `plan` in the paid-call sheet.
    func approve(_ plan: UnderstandPlan, model: AppModel) {
        pendingPlan = nil
        guard !busy else { return }
        Task { await self.runApproved(plan, model: model) }
    }

    func decline() {
        pendingPlan = nil
        declined = true
        message = "Nothing was sent. Ask again for a new plan when you are ready."
    }

    // MARK: The stages

    private func begin(model: AppModel) async {
        guard !busy else { return }
        reset()
        let problems = draft.problems
        guard problems.isEmpty, let video = draft.video else {
            message = problems.joined(separator: " ")
            return
        }
        busy = true
        var outliers: [InputRef] = []
        for choice in draft.outliers.prefix(3) {
            switch choice {
            case .asset(let id):
                outliers.append(.asset(id))
            case .subject(let id):
                guard let file = await model.headFile(of: id, role: "out") else {
                    busy = false
                    message = "The reel \(model.subject(id)?.name ?? String(id)) has no edit DNA yet."
                    return
                }
                outliers.append(.hash(file.hash))
            }
        }
        busy = false
        var request = MakeReelsRequest(
            input: .asset(video),
            outliers: outliers,
            targetURL: draft.targetURL.trimmingCharacters(in: .whitespaces)
        )
        if let asset = model.assets.first(where: { $0.id == video }) { request.name = asset.name }
        if let dna = draft.existingDna {
            request.dna = .asset(dna)
        } else {
            let whisper = ToolSetting.whisperModel.value
            if !whisper.isEmpty { request.whisperModel = whisper }
        }
        let modelText = draft.model.trimmingCharacters(in: .whitespaces)
        if !modelText.isEmpty { request.model = modelText }
        if let cost = Double(draft.maxCostText.trimmingCharacters(in: .whitespaces)) { request.maxCostUsd = cost }
        request.clips = draft.clips ?? []
        if draft.limitReels { request.reels = draft.reels }
        request.allowFlagged = draft.allowFlagged
        request.follow = draft.follow
        let weights = ToolSetting.rvmWeights.value
        if draft.follow == "presenter" && !weights.isEmpty { request.weights = weights }
        if let card = Int(draft.cardMsText.trimmingCharacters(in: .whitespaces)) { request.cardMs = card }
        let size = draft.size.trimmingCharacters(in: .whitespaces)
        if !size.isEmpty { request.size = size }
        request.style = draft.style.json
        self.request = request
        phase = .decompile
        await drive(model: model)
    }

    /// Call `pipelines.make_reels` until it needs the user (a plan) or gives the reels.
    private func drive(model: AppModel) async {
        busy = true
        defer { busy = false }
        while let current = request {
            guard let answer = await model.perform({ try await $0.makeReels(current) }) else {
                message = "The service refused the request; see the banner."
                return
            }
            var next = current
            next.subject = answer.subject
            request = next
            stage = answer
            switch answer.stage {
            case "decompile":
                phase = .decompile
                guard let job = answer.jobs?.first else {
                    message = "The service gave no decompile job."
                    return
                }
                decompileJob = job
                let done = await model.waitFor(job)
                guard let done, done.status == .succeeded else {
                    message = "Decompile did not finish (\(model.job(job)?.status.rawValue ?? "stopped")). Fix it and start again."
                    return
                }
                next.dna = .fromJob(job, role: "out")
                request = next
            case "plan":
                phase = .plan
                guard let plan = answer.plan else {
                    message = "The service gave no plan."
                    return
                }
                lastPlan = plan
                approvedUsd = nil
                pendingPlan = plan
                return
            case "reels":
                phase = .reels
                return
            default:
                message = "The service answered an unknown stage, \(answer.stage)."
                return
            }
        }
    }

    /// `understand.run` for the approved plan, then the reels.
    private func runApproved(_ plan: UnderstandPlan, model: AppModel) async {
        guard let key = KeyStore.read() else {
            message = "No API key is set. Add it in Settings, then ask again."
            declined = true
            return
        }
        busy = true
        let job = await model.perform { studio in
            try await studio.runUnderstand(plan: plan, approvedMaxUsd: plan.costUpperBoundUsd, apiKey: key)
        }
        guard let job else {
            busy = false
            declined = true
            message = "The call was refused (an expired plan needs a new one). Nothing was spent."
            return
        }
        approvedUsd = plan.costUpperBoundUsd
        understandJob = job
        phase = .understand
        let done = await model.waitFor(job)
        busy = false
        guard let done, done.status == .succeeded, var next = request else {
            message = "Understand did not finish (\(model.job(job)?.status.rawValue ?? "stopped"))."
            return
        }
        next.understanding = .fromJob(job, role: "out")
        request = next
        await drive(model: model)
    }
}
