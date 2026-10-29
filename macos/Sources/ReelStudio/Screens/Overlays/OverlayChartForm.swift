import AppKit
import ReelCore
import SwiftUI

/// The chart-spec form: every `ChartSpec` field, with the limits beside them.
struct OverlayChartForm: View {
    @Binding var draft: OverlayChartDraft
    /// The project's edited length, when known, to warn about a span past it.
    let timelineMs: Int64?

    var body: some View {
        Form {
            SwiftUI.Section("Chart") {
                Picker("Kind", selection: $draft.kind) {
                    ForEach(OverlayChartKind.allCases) { kind in
                        Text(kind.title).tag(kind)
                    }
                }
                TextField("Title", text: $draft.title, prompt: Text("1 to 60 characters"))
                TextField("Unit", text: $draft.unit, prompt: Text("optional, up to 8, e.g. %"))
            }
            SwiftUI.Section("Series (\(draft.kind.pointsText))") {
                ForEach($draft.points) { $point in
                    HStack(spacing: 8) {
                        TextField("Label", text: $point.label, prompt: Text("Label"))
                            .labelsHidden()
                        TextField("Value", value: $point.value, format: .number, prompt: Text("Value"))
                            .labelsHidden()
                            .frame(width: 120)
                            .multilineTextAlignment(.trailing)
                        Button {
                            draft.removePoint(point.id)
                        } label: {
                            Image(systemName: "minus.circle")
                        }
                        .buttonStyle(.borderless)
                        .disabled(draft.points.count <= draft.kind.minPoints)
                        .help("Remove this point")
                    }
                }
                Button("Add point") { draft.addPoint() }
                    .disabled(draft.points.count >= draft.kind.maxPoints)
            }
            SwiftUI.Section("Span on the edited timeline") {
                TextField("Start (ms)", value: $draft.startMs, format: .number)
                TextField("End (ms)", value: $draft.endMs, format: .number)
                LabeledContent("Length") {
                    Text(formatMs(Int64(draft.endMs - draft.startMs))).monospacedDigit()
                }
                if let timelineMs {
                    LabeledContent("Timeline") {
                        Text(formatMs(timelineMs)).monospacedDigit()
                    }
                    if Int64(draft.endMs) > timelineMs {
                        Label("The span ends after the timeline; the service will refuse it.", systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                            .font(.callout)
                    }
                }
                Text("500 to 30000 ms, rounded to the nearest frame. It may not overlap another overlay.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            SwiftUI.Section("Placement") {
                Picker("Position", selection: $draft.position) {
                    ForEach(OverlayChartPosition.allCases) { position in
                        Text(position.title).tag(position)
                    }
                }
                .pickerStyle(.segmented)
                Stepper(value: $draft.widthPct, in: 20...100, step: 5) {
                    LabeledContent("Width") {
                        Text("\(draft.widthPct)% of the frame").monospacedDigit()
                    }
                }
            }
        }
        .formStyle(.grouped)
        .onChange(of: draft.kind) { _, kind in
            draft.fit(to: kind)
        }
    }
}

/// The spec as it will be written, and what is wrong with it.
struct OverlayChartPreview: View {
    let draft: OverlayChartDraft

    var body: some View {
        let problems = draft.problems
        VStack(alignment: .leading, spacing: 8) {
            Text("chart_spec.json").font(.headline)
            ScrollView {
                Text(draft.prettyText)
                    .font(.caption.monospaced())
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(8)
            }
            .frame(minHeight: 140, maxHeight: 260)
            .background(Color(nsColor: .textBackgroundColor), in: RoundedRectangle(cornerRadius: 6))
            if problems.isEmpty {
                Label("The spec is valid.", systemImage: "checkmark.circle").foregroundStyle(.green).font(.callout)
            } else {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(problems, id: \.self) { problem in
                        Label(problem, systemImage: "xmark.circle").foregroundStyle(.red).font(.callout)
                    }
                }
            }
        }
    }
}
