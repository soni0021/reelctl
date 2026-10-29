import ReelCore
import SwiftUI
import UniformTypeIdentifiers

private let evalJSONLType: UTType = UTType(filenameExtension: "jsonl") ?? .data

/// The eval form: subject, truth, answers, the DNA when the truth needs it.
struct EvalFormView: View {
    @Environment(AppModel.self) private var model
    @Binding var form: EvalForm
    let session: EvalSession

    var body: some View {
        Form {
            SwiftUI.Section("Eval") {
                SubjectPicker(kind: "eval", selection: $form.subject)
                Text(verbatim: "Scores a model's answers against measured truth. It reads files only and never calls a model.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            SwiftUI.Section("Truth") {
                Picker("Truth", selection: $form.truthOrigin) {
                    ForEach(EvalTruthOrigin.allCases) { origin in
                        Text(origin.title).tag(origin)
                    }
                }
                .pickerStyle(.segmented)
                switch form.truthOrigin {
                case .understandingRun:
                    Picker("Understanding", selection: $form.truthSubject) {
                        Text("None").tag(SubjectID?.none)
                        ForEach(model.subjects(kind: "understanding")) { subject in
                            Text(subject.name).tag(SubjectID?.some(subject.id))
                        }
                    }
                case .understandingFile:
                    AssetPicker(title: "understanding.json", kind: "understanding", types: [.json], selection: $form.truthAsset)
                case .datasetFile:
                    AssetPicker(title: "Dataset .jsonl", kind: "jsonl", types: [evalJSONLType, .json], selection: $form.truthAsset)
                }
            }

            SwiftUI.Section("Answers") {
                EvalAnswersPicker(selection: $form.answers)
                Text(verbatim: answersHint)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            if form.truthOrigin.needsDNA {
                SwiftUI.Section("DNA of the same video") {
                    UnderstandDNAPicker(selection: $form.dna)
                    Text(verbatim: "Needed for an understanding.json truth: the clip rules use its measured pauses, and the durations must match.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } else {
                SwiftUI.Section("Tolerances") {
                    Stepper("Cut tolerance \(form.cutToleranceMs) ms", value: $form.cutToleranceMs, in: 0...2000, step: 10)
                    Stepper("Event tolerance \(form.eventToleranceMs) ms", value: $form.eventToleranceMs, in: 0...5000, step: 10)
                    Text(verbatim: "One .jsonl file is scored whole, so the split does not apply (the report's split is file).")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            SwiftUI.Section {
                HStack {
                    Button("Run eval") {
                        let current = form
                        Task { await session.submit(form: current, model: model) }
                    }
                    .disabled(session.busy || !model.isConnected || form.subject == nil || form.answers == nil)
                    if session.busy {
                        ProgressView().controlSize(.small)
                    }
                }
                if let job = session.job {
                    JobProgressView(job: job)
                }
                if let note = session.note {
                    Text(note)
                        .font(.callout)
                        .foregroundStyle(.red)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .formStyle(.grouped)
    }

    private var answersHint: String {
        switch form.truthOrigin {
        case .understandingRun, .understandingFile:
            return "A model reply: the file understand --replay reads, or a bare reply. Only reel_candidates is read."
        case .datasetFile:
            return "JSON lines of {\"id\": record id, \"answer\": ...}, one per record."
        }
    }
}

/// Answers are imported as `jsonl` (a .jsonl file) or `other` (anything else).
struct EvalAnswersPicker: View {
    @Environment(AppModel.self) private var model
    @Binding var selection: AssetID?

    var body: some View {
        HStack {
            Picker("Answers", selection: $selection) {
                Text("None").tag(AssetID?.none)
                ForEach(model.assets.filter { $0.kind == "jsonl" || $0.kind == "other" }) { asset in
                    Text("\(asset.name) (\(asset.kind))").tag(AssetID?.some(asset.id))
                }
            }
            Button("Import…") {
                guard let url = chooseFile([evalJSONLType, .json, .data], title: "Answers") else { return }
                let kind = url.pathExtension.lowercased() == "jsonl" ? "jsonl" : "other"
                Task {
                    if let asset = await model.importFile(url, kind: kind) { selection = asset.id }
                }
            }
        }
    }
}

/// The report, flattened into name → value.
struct EvalReportView: View {
    let session: EvalSession
    @State private var filter = ""

    var body: some View {
        if session.rows.isEmpty {
            ContentUnavailableView(
                "No eval report",
                systemImage: "tablecells",
                description: Text("Pick a truth file and answers, run the eval, and its metrics appear here.")
            )
        } else {
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    TextField("Filter metrics", text: $filter)
                        .textFieldStyle(.roundedBorder)
                        .frame(maxWidth: 280)
                    Spacer()
                    Text("\(shown.count) of \(session.rows.count)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Table(shown) {
                    TableColumn("Metric") { (row: EvalMetricRow) in
                        Text(row.id).font(.body.monospaced()).textSelection(.enabled)
                    }
                    TableColumn("Value") { (row: EvalMetricRow) in
                        Text(row.value)
                            .font(.body.monospacedDigit())
                            .foregroundStyle(row.isNull ? Color.secondary : Color.primary)
                            .textSelection(.enabled)
                    }
                }
                if let path = session.reportPath {
                    Text(verbatim: path)
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Text(verbatim: "A null score could not be computed (nothing to score). Compare two reports by their values; none of them is a pass mark.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding(12)
        }
    }

    private var shown: [EvalMetricRow] {
        let needle = filter.trimmingCharacters(in: .whitespaces).lowercased()
        guard !needle.isEmpty else { return session.rows }
        return session.rows.filter { $0.id.lowercased().contains(needle) }
    }
}
