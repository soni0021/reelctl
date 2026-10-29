import ReelCore
import SwiftUI

/// The Library's inspector: the selected marker's fields and the run's
/// measurements and notes from `edit_dna.json`.
struct AnalysisInspector: View {
    let asset: Asset?
    let dna: EditDNA?
    let marker: AnalysisMarker?
    let checkpoint: CheckpointID?

    var body: some View {
        Form {
            SwiftUI.Section("Selected") {
                if let marker {
                    Text(marker.title).font(.headline)
                    ForEach(marker.fields) { field in
                        FieldRow(label: field.label, value: field.value)
                    }
                } else {
                    Text("Click a marker on the lanes or a filmstrip picture.")
                        .foregroundStyle(.secondary)
                }
            }
            if let dna {
                runSection(dna)
                notesSection(dna)
            } else if let asset {
                SwiftUI.Section("File") {
                    FieldRow(label: "Name", value: asset.name)
                    FieldRow(label: "Size", value: formatBytes(asset.size))
                    Text(asset.sourcePath).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                }
            }
        }
        .formStyle(.grouped)
    }

    @ViewBuilder
    private func runSection(_ dna: EditDNA) -> some View {
        let n = AnalysisSupport.number
        SwiftUI.Section("Run") {
            FieldRow(label: "signal_ms", value: "\(dna.signalMs) ms")
            FieldRow(label: "asr_ms", value: "\(dna.asrMs) ms")
            FieldRow(label: "asr_backend", value: dna.asrBackend)
            FieldRow(label: "cut_detector", value: dna.cutDetector)
            FieldRow(label: "Frame size", value: "\(dna.frameSize.width) × \(dna.frameSize.height)")
            FieldRow(label: "fps", value: n(dna.fps, 3))
            FieldRow(label: "Frames", value: "\(dna.frameCount)")
            FieldRow(label: "Duration", value: formatMs(dna.durationMs))
            FieldRow(label: "Cuts", value: "\(dna.cuts.count)")
            FieldRow(label: "Words per minute", value: n(dna.wordsPerMinute, 1))
            FieldRow(label: "Integrated loudness", value: n(dna.integratedLufs, 1) + " LUFS")
            FieldRow(label: "Silence threshold", value: n(dna.silenceThresholdDb, 1) + " dB")
            FieldRow(label: "Payoff window", value: "\(formatMs(dna.payoffWindow.windowStartMs)) – \(formatMs(dna.payoffWindow.windowEndMs))")
            FieldRow(label: "Payoff cue", value: dna.payoffWindow.cueMs.map { formatMs($0) } ?? (dna.payoffWindow.hasCue ? "yes" : "none"))
            if let zoom = dna.zoomCurves {
                FieldRow(label: "Zoom curves", value: "\(zoom.curves.count) (\(zoom.status))")
            }
            FieldRow(label: "Schema", value: "\(dna.schemaVersion)")
            if let checkpoint {
                FieldRow(label: "Checkpoint", value: "\(checkpoint)")
            }
        }
    }

    @ViewBuilder
    private func notesSection(_ dna: EditDNA) -> some View {
        SwiftUI.Section("Notes") {
            note("asr_note", dna.asrNote)
            note("cut_detector_note", dna.cutDetectorNote)
            note("decode_note", dna.decodeNote)
            if let zoom = dna.zoomCurves, !zoom.reason.isEmpty {
                note("zoom_curves.reason", zoom.reason)
            }
        }
    }

    private func note(_ label: String, _ text: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(label).font(.caption.bold()).foregroundStyle(.secondary)
            Text(text.isEmpty ? "—" : text)
                .font(.callout)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
