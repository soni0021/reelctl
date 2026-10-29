import Foundation
import Observation
import ReelCore

/// Where the eval's truth comes from.
enum EvalTruthOrigin: String, CaseIterable, Identifiable, Hashable {
    /// The head `out` of an `understanding` subject.
    case understandingRun
    /// An imported `understanding.json` (asset kind `understanding`).
    case understandingFile
    /// One `.jsonl` file that `dataset build` wrote (asset kind `jsonl`).
    case datasetFile

    var id: String { rawValue }

    var title: String {
        switch self {
        case .understandingRun: return "Understand run"
        case .understandingFile: return "understanding.json"
        case .datasetFile: return "Dataset .jsonl"
        }
    }

    /// An `understanding.json` truth needs the DNA of the same video.
    var needsDNA: Bool { self != .datasetFile }
}

/// The eval form.
struct EvalForm: Equatable {
    var subject: SubjectID?
    var truthOrigin: EvalTruthOrigin = .understandingRun
    var truthSubject: SubjectID?
    var truthAsset: AssetID?
    var answers: AssetID?
    var dna = UnderstandDNASelection()
    var cutToleranceMs = 100
    var eventToleranceMs = 250
}

/// One line of the metrics table: a dotted key and its value as text.
struct EvalMetricRow: Identifiable, Hashable {
    let id: String
    let value: String
    let isNull: Bool
}

enum EvalMetrics {
    /// Every leaf of `value`, objects flattened into dotted keys and arrays into
    /// `key[i]`, keys sorted.
    static func flatten(_ value: JSONValue) -> [EvalMetricRow] {
        var rows: [EvalMetricRow] = []
        walk(value, key: "", into: &rows)
        return rows
    }

    private static func walk(_ value: JSONValue, key: String, into rows: inout [EvalMetricRow]) {
        let name = key.isEmpty ? "(report)" : key
        switch value {
        case .object(let object):
            if object.isEmpty {
                rows.append(EvalMetricRow(id: name, value: "{}", isNull: false))
                return
            }
            for child in object.keys.sorted() {
                walk(object[child] ?? .null, key: key.isEmpty ? child : key + "." + child, into: &rows)
            }
        case .array(let items):
            if items.isEmpty {
                rows.append(EvalMetricRow(id: name, value: "[]", isNull: false))
                return
            }
            for (index, item) in items.enumerated() {
                walk(item, key: "\(key)[\(index)]", into: &rows)
            }
        case .null:
            rows.append(EvalMetricRow(id: name, value: "null", isNull: true))
        case .bool(let flag):
            rows.append(EvalMetricRow(id: name, value: flag ? "true" : "false", isNull: false))
        case .int(let number):
            rows.append(EvalMetricRow(id: name, value: String(number), isNull: false))
        case .double(let number):
            rows.append(EvalMetricRow(id: name, value: format(number), isNull: false))
        case .string(let text):
            rows.append(EvalMetricRow(id: name, value: text, isNull: false))
        }
    }

    private static func format(_ number: Double) -> String {
        guard number.isFinite else { return String(number) }
        if number == number.rounded(), abs(number) < 1e15 { return String(Int64(number)) }
        var text = String(format: "%.4f", number)
        while text.hasSuffix("0") { text.removeLast() }
        if text.hasSuffix(".") { text.removeLast() }
        return text
    }
}

/// Submit `eval`, wait for it, and read its stdout report (output role `report`).
@MainActor
@Observable
final class EvalSession {
    private(set) var job: JobID?
    var busy = false
    private(set) var rows: [EvalMetricRow] = []
    private(set) var reportPath: String?
    var note: String?

    func submit(form: EvalForm, model: AppModel) async {
        guard let subject = form.subject else {
            note = "Pick or create an eval subject first."
            return
        }
        guard let answers = form.answers else {
            note = "Pick the answers file."
            return
        }
        busy = true
        defer { busy = false }
        note = nil
        guard let truth = await truthRef(form, model: model) else { return }
        var inputs: [String: InputRef] = ["truth": truth, "answers": .asset(answers)]
        if form.truthOrigin.needsDNA {
            switch await UnderstandDNA.resolve(form.dna, model: model) {
            case .none:
                note = "An understanding.json truth needs the DNA of the same video."
                return
            case .missing(let reason):
                note = reason
                return
            case .found(let ref, _):
                inputs["dna"] = ref
            }
        }
        var args: [String: JSONValue] = [:]
        if form.truthOrigin == .datasetFile {
            args["cut_tolerance_ms"] = .int(Int64(form.cutToleranceMs))
            args["event_tolerance_ms"] = .int(Int64(form.eventToleranceMs))
        }
        let submitted: SubmitResult? = await model.perform { studio in
            try await studio.submit(subject: subject, command: "eval", args: args, inputs: inputs)
        }
        guard let submitted else { return }
        job = submitted.job
        rows = []
        reportPath = nil
        guard let finished = await model.waitFor(submitted.job) else { return }
        guard finished.status == .succeeded else {
            note = finished.error ?? "The eval finished as \(finished.status.rawValue)."
            return
        }
        guard let file = await model.output(of: submitted.job, role: "report") else {
            note = "The eval succeeded but has no report output."
            return
        }
        let url = file.url
        do {
            let report: JSONValue = try await Task.detached(priority: .userInitiated) {
                try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: url))
            }.value
            rows = EvalMetrics.flatten(report)
            reportPath = file.path
        } catch {
            note = "Could not read the report: \(error.localizedDescription)"
        }
    }

    private func truthRef(_ form: EvalForm, model: AppModel) async -> InputRef? {
        switch form.truthOrigin {
        case .understandingRun:
            guard let subject = form.truthSubject else {
                note = "Pick the understanding subject whose result is the truth."
                return nil
            }
            guard let file = await model.headFile(of: subject, role: "out") else {
                note = "That understanding subject has no understanding.json at its head yet."
                return nil
            }
            return .hash(file.hash)
        case .understandingFile, .datasetFile:
            guard let asset = form.truthAsset else {
                note = "Pick the truth file."
                return nil
            }
            return .asset(asset)
        }
    }
}
