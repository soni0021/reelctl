import ReelCore
import SwiftUI
import UniformTypeIdentifiers

// The Understand form's values, and the DNA picker that the Datasets screen
// reuses: an imported edit_dna.json, or the head `out` of a reel subject.

/// Where a DNA comes from.
enum UnderstandDNAOrigin: String, CaseIterable, Identifiable, Hashable {
    case asset
    case reel

    var id: String { rawValue }

    var title: String {
        switch self {
        case .asset: return "Imported file"
        case .reel: return "Reel analysis"
        }
    }
}

struct UnderstandDNASelection: Equatable, Hashable {
    var origin: UnderstandDNAOrigin = .asset
    var asset: AssetID?
    var reel: SubjectID?
}

/// What a DNA selection points at.
enum UnderstandDNAResolution: Sendable {
    /// Nothing is chosen.
    case none
    /// Chosen, but there is no file (the reel has no decompile output yet).
    case missing(String)
    case found(ref: InputRef, url: URL)
}

enum UnderstandDNA {
    /// The input reference and the readable path of the chosen DNA.
    @MainActor
    static func resolve(_ selection: UnderstandDNASelection, model: AppModel) async -> UnderstandDNAResolution {
        switch selection.origin {
        case .asset:
            guard let id = selection.asset else { return .none }
            guard let asset = model.assets.first(where: { $0.id == id }) else {
                return .missing("The imported DNA is no longer in the catalogue.")
            }
            return .found(ref: .asset(asset.id), url: asset.url)
        case .reel:
            guard let reel = selection.reel else { return .none }
            guard let file = await model.headFile(of: reel, role: "out") else {
                return .missing("This reel has no edit_dna.json yet: run decompile on it first.")
            }
            return .found(ref: .hash(file.hash), url: file.url)
        }
    }
}

/// Pick a DNA: an imported `edit_dna` asset, or a reel subject's head.
struct UnderstandDNAPicker: View {
    @Environment(AppModel.self) private var model
    @Binding var selection: UnderstandDNASelection

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Picker("DNA from", selection: $selection.origin) {
                ForEach(UnderstandDNAOrigin.allCases) { origin in
                    Text(origin.title).tag(origin)
                }
            }
            .pickerStyle(.segmented)
            switch selection.origin {
            case .asset:
                AssetPicker(title: "edit_dna.json", kind: "edit_dna", types: [.json], selection: $selection.asset)
            case .reel:
                Picker("Reel", selection: $selection.reel) {
                    Text("None").tag(SubjectID?.none)
                    ForEach(model.subjects(kind: "reel")) { subject in
                        Text(subject.name).tag(SubjectID?.some(subject.id))
                    }
                }
            }
        }
    }
}

/// Everything the user sets before a plan. Any change makes the plan stale.
struct UnderstandForm: Equatable {
    static let defaultModel = "qwen/qwen3.5-9b"

    var subject: SubjectID?
    var video: AssetID?
    var dna = UnderstandDNASelection()
    var model = UnderstandForm.defaultModel
    var everyS = 5
    /// `auto`, `1`, `2` or `3`.
    var sheet = "auto"
    /// `none` (the model's default), `low`, `medium` or `high`.
    var effort = "none"
    /// The dry run refuses a bound over this (`max_cost_usd`, exit 4).
    var capUsd = 0.50

    /// The model id sent, or nil for the service's default.
    var modelID: String? {
        let trimmed = model.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    /// The model the sheet names.
    var shownModel: String { modelID ?? UnderstandForm.defaultModel }

    /// The plan's extra params (the `understand` argument names).
    var planOptions: [String: JSONValue] {
        var options: [String: JSONValue] = [
            "sheet": .string(sheet),
            "max_cost_usd": .double(capUsd),
        ]
        if effort != "none" { options["effort"] = .string(effort) }
        return options
    }
}

/// The state of the chosen DNA's check for whisper words.
enum UnderstandDNACheck: Equatable {
    case idle
    case loading
    case ready(words: Int, durationMs: Int64)
    case noWords(backend: String, words: Int)
    case missing(String)
    case failed(String)

    var isReady: Bool {
        if case .ready = self { return true }
        return false
    }
}

/// The DNA check, as a line under the picker.
struct UnderstandDNAStatus: View {
    @Environment(AppModel.self) private var model
    let check: UnderstandDNACheck

    var body: some View {
        switch check {
        case .idle:
            Text("Pick the DNA of this video, made by decompile with a whisper model.")
                .font(.callout).foregroundStyle(.secondary)
        case .loading:
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("Reading the DNA…").font(.callout).foregroundStyle(.secondary)
            }
        case .ready(let words, let durationMs):
            Label("\(words) whisper words over \(formatMs(durationMs))", systemImage: "checkmark.circle")
                .foregroundStyle(.green)
        case .noWords(let backend, let words):
            VStack(alignment: .leading, spacing: 6) {
                Label("No whisper words in this DNA", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
                Text(noWordsText(backend: backend, words: words))
                    .font(.callout)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Open Settings") { model.selection = .settings }
            }
        case .missing(let reason):
            Label(reason, systemImage: "questionmark.circle").foregroundStyle(.orange)
        case .failed(let reason):
            Label("Could not read the DNA: \(reason)", systemImage: "xmark.octagon").foregroundStyle(.red)
        }
    }

    private func noWordsText(backend: String, words: Int) -> String {
        let found = words == 0 ? "it has no words" : "its \(words) words are estimates"
        return "Its asr_backend is \(backend), so \(found). Understand needs word timing from whisper: "
            + "set a whisper model in Settings, run decompile on the video again, and pick the new DNA."
    }
}
