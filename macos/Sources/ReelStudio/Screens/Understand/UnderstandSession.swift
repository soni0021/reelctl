import Foundation
import Observation
import ReelCore

/// The Understand screen's work: the DNA check, the plan, the approved run and
/// its result. The live call happens only in `run(approved:model:)`, which the
/// screen calls only from `PaidCallSheet`'s approve button (ADR-031).
@MainActor
@Observable
final class UnderstandSession {
    var dnaCheck: UnderstandDNACheck = .idle
    /// The DNA input, set only when it has whisper words.
    private(set) var dnaRef: InputRef?
    var planning = false
    /// The current plan, unused and for the form as it is now.
    private(set) var plan: UnderstandPlan?
    /// The model the plan was made for (what the sheet names).
    private(set) var plannedModel = UnderstandForm.defaultModel
    private(set) var runJob: JobID?
    var running = false
    private(set) var result: Understanding?
    /// Where the shown result came from.
    private(set) var resultSource = ""
    var note: String?

    private var dnaToken = 0
    private var planToken = 0
    private var resultToken = 0

    /// Whether `plan` can still be approved (15 minutes after it was made).
    static func isLive(_ plan: UnderstandPlan, at date: Date = Date()) -> Bool {
        date.timeIntervalSince1970 * 1000 < Double(plan.expiresAt)
    }

    /// Forget the plan: the form changed, or it was used.
    func invalidatePlan() {
        planToken += 1
        plan = nil
    }

    // MARK: DNA

    func checkDNA(_ selection: UnderstandDNASelection, model: AppModel) async {
        dnaToken += 1
        let token = dnaToken
        dnaRef = nil
        dnaCheck = .loading
        let resolution = await UnderstandDNA.resolve(selection, model: model)
        guard token == dnaToken else { return }
        switch resolution {
        case .none:
            dnaCheck = .idle
        case .missing(let reason):
            dnaCheck = .missing(reason)
        case .found(let ref, let url):
            do {
                let dna: EditDNA = try await Task.detached(priority: .userInitiated) {
                    try EditDNA.load(url)
                }.value
                guard token == dnaToken else { return }
                if dna.hasWhisperWords {
                    dnaRef = ref
                    dnaCheck = .ready(words: dna.words.count, durationMs: dna.durationMs)
                } else {
                    dnaCheck = .noWords(backend: dna.asrBackend, words: dna.words.count)
                }
            } catch {
                guard token == dnaToken else { return }
                dnaCheck = .failed(error.localizedDescription)
            }
        }
    }

    // MARK: Plan

    /// `understand.plan`: the dry run. No key, no spend.
    func makePlan(form: UnderstandForm, model: AppModel) async {
        guard let subject = form.subject else {
            model.banner = "Pick or create an understanding subject first."
            return
        }
        guard let video = form.video else {
            model.banner = "Pick the source video first."
            return
        }
        guard let dna = dnaRef, dnaCheck.isReady else {
            model.banner = "The DNA needs whisper words before a plan."
            return
        }
        invalidatePlan()
        let token = planToken
        planning = true
        defer { planning = false }
        let modelID = form.modelID
        let everyS = form.everyS
        let options = form.planOptions
        let made: UnderstandPlan? = await model.perform { studio in
            try await studio.planUnderstand(
                subject: subject,
                input: .asset(video),
                dna: dna,
                model: modelID,
                everyS: everyS,
                options: options
            )
        }
        guard let made, token == planToken else { return }
        plan = made
        plannedModel = form.shownModel
        note = nil
    }

    // MARK: The paid call

    /// `understand.run` for `approved`, which the user approved in the sheet just
    /// now. Refuses a plan that is not the current one or has expired.
    func run(approved: UnderstandPlan, model: AppModel) async {
        guard let current = plan, current == approved else {
            model.banner = "That plan is no longer current. Plan again and approve the new amount."
            return
        }
        guard Self.isLive(current) else {
            invalidatePlan()
            model.banner = "The plan expired (plans last 15 minutes). Plan again and approve the new amount."
            return
        }
        guard let key = KeyStore.read() else {
            model.banner = "No OpenRouter API key is set. Add it in Settings, then plan again."
            model.selection = .settings
            return
        }
        // A plan is used once, whatever the answer.
        invalidatePlan()
        running = true
        defer { running = false }
        note = nil
        let job: JobID? = await model.perform { studio in
            try await studio.runUnderstand(plan: current, approvedMaxUsd: current.costUpperBoundUsd, apiKey: key)
        }
        guard let job else { return }
        runJob = job
        let finished = await model.waitFor(job)
        guard let finished else { return }
        guard finished.status == .succeeded else {
            note = finished.error ?? "The call finished as \(finished.status.rawValue)."
            return
        }
        guard let file = await model.output(of: job, role: "out") else {
            note = "The job succeeded but has no understanding.json output."
            return
        }
        await loadResult(file.url, source: "job \(job)", model: model)
    }

    // MARK: Results

    /// The subject's latest understanding.json, if its head has one.
    func loadHead(subject: SubjectID?, model: AppModel) async {
        resultToken += 1
        result = nil
        resultSource = ""
        runJob = nil
        note = nil
        guard let subject else { return }
        let token = resultToken
        guard let file = await model.headFile(of: subject, role: "out"), token == resultToken else { return }
        await loadResult(file.url, source: "the subject's latest result", model: model)
    }

    private func loadResult(_ url: URL, source: String, model: AppModel) async {
        resultToken += 1
        let token = resultToken
        do {
            let loaded: Understanding = try await Task.detached(priority: .userInitiated) {
                try Understanding.load(url)
            }.value
            guard token == resultToken else { return }
            result = loaded
            resultSource = source
        } catch {
            guard token == resultToken else { return }
            note = "Could not read understanding.json: \(error.localizedDescription)"
        }
    }
}
