import AppKit
import ReelCore
import SwiftUI

/// Datasets and Eval (docs/specs/mac-app.md screen 7). `dataset build` is not
/// a service job yet (its inputs are folders), so it is shown as a command for
/// the user to run; `eval` runs as a job and its metrics are shown as a table.
struct DatasetsScreen: View {
    @State private var form = EvalForm()
    @State private var session = EvalSession()

    init() {}

    var body: some View {
        VStack(spacing: 0) {
            DatasetBuildNotice()
                .padding(12)
            Divider()
            HStack(spacing: 0) {
                EvalFormView(form: $form, session: session)
                    .frame(width: 420)
                Divider()
                EvalReportView(session: session)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .navigationTitle("Datasets")
        .onChange(of: form.truthOrigin) { _, _ in
            form.truthAsset = nil
        }
    }
}

/// Why `dataset build` is not a button here, and the command to run instead.
struct DatasetBuildNotice: View {
    static let command = "reelctl dataset build --dna <dir|edit_dna.json>... [--understanding <understanding.json>...] --out <dir> [--val-percent 10] [--salt reelctl-dataset-v1]"
    static let example = "reelctl dataset build --dna reels/ --understanding talk.understanding.json --out dataset/"

    @State private var copied = false

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 8) {
                Text(verbatim: "dataset build is not a service job yet: its inputs are folders of edit_dna.json files, which the job catalogue cannot take. Run it in Terminal. It is offline (no network, no model) and writes train.jsonl, val.jsonl and manifest.json into the output folder. Import a .jsonl it wrote below to evaluate answers against it.")
                    .font(.callout)
                    .fixedSize(horizontal: false, vertical: true)
                commandLine(Self.command)
                commandLine(Self.example)
                HStack {
                    Button(copyTitle) {
                        let board = NSPasteboard.general
                        board.clearContents()
                        board.setString(Self.example, forType: .string)
                        copied = true
                    }
                    Text(verbatim: "The app does not run it; nothing here is executed.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } label: {
            Label("Build a dataset", systemImage: "tablecells")
        }
    }

    private var copyTitle: String { copied ? "Copied" : "Copy the example" }

    private func commandLine(_ text: String) -> some View {
        Text(verbatim: text)
            .font(.callout.monospaced())
            .textSelection(.enabled)
            .padding(6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.secondary.opacity(0.10), in: RoundedRectangle(cornerRadius: 5))
    }
}
