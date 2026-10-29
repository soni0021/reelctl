import Foundation
import ReelCore
import SwiftUI

/// Chart overlays on a project (docs/specs/mac-app.md, screen 8): a chart-spec
/// form with a live JSON preview, `overlay.add-chart`, and `overlay.remove`.
struct OverlaysScreen: View {
    @Environment(AppModel.self) private var model
    @State private var project: SubjectID? = nil
    @State private var draft = OverlayChartDraft()
    @State private var head: StoredFile? = nil
    @State private var timelineMs: Int64?
    @State private var overlays: [OverlayEntry] = []
    @State private var hasCaptions = false
    @State private var loadNote: String? = nil
    @State private var manimgx: DoctorItem? = nil
    @State private var doctorChecked = false
    @State private var skipInspect = false
    @State private var timeoutText = ""
    @State private var activeJob: JobID? = nil
    @State private var working = false

    init() {}

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            HStack(alignment: .top, spacing: 0) {
                OverlayChartForm(draft: $draft, timelineMs: timelineMs)
                    .frame(minWidth: 380, idealWidth: 440, maxWidth: 520)
                Divider()
                ScrollView {
                    VStack(alignment: .leading, spacing: 18) {
                        OverlayChartPreview(draft: draft)
                        addControls
                        if let activeJob {
                            JobProgressView(job: activeJob)
                        }
                        if let loadNote {
                            Text(loadNote).foregroundStyle(.secondary)
                        }
                        if head != nil {
                            OverlayListView(
                                overlays: overlays,
                                hasCaptions: hasCaptions,
                                disabled: working,
                                remove: { overlay in Task { await remove(overlayID: overlay.id) } },
                                removeCaptions: { Task { await remove(overlayID: nil) } }
                            )
                        }
                    }
                    .padding(16)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .navigationTitle("Overlays")
        .task { await checkDoctor() }
        .task(id: OverlaysReloadKey(project: project, tick: model.checkpointTick)) { await reload() }
        .onAppear {
            if project == nil, let focused = model.focusedSubject, model.subject(focused)?.kind == "project" {
                project = focused
            }
        }
    }

    private var header: some View {
        HStack(spacing: 16) {
            SubjectPicker(kind: "project", selection: $project)
                .frame(maxWidth: 360)
            Spacer()
            manimgxStatus
        }
        .padding(12)
    }

    @ViewBuilder
    private var manimgxStatus: some View {
        if let manimgx, manimgx.found {
            Label("manimgx \(manimgx.version ?? "")", systemImage: "checkmark.circle")
                .foregroundStyle(.green)
                .help(manimgx.path ?? "")
        } else if doctorChecked {
            HStack(spacing: 8) {
                Label("manimgx not found", systemImage: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                Button("Settings…") { model.selection = .settings }
            }
            .help("Install it with `uv tool install --python 3.14 manimgx`, or choose it in Settings (MANIMGX_BIN).")
        } else {
            ProgressView().controlSize(.small)
        }
    }

    private var addControls: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 12) {
                Toggle("Skip manimgx inspect", isOn: $skipInspect)
                TextField("Timeout (s)", text: $timeoutText, prompt: Text("timeout s, default 300"))
                    .frame(width: 170)
            }
            HStack {
                Button {
                    Task { await addChart() }
                } label: {
                    Label("Add chart", systemImage: "plus.rectangle.on.rectangle")
                }
                .buttonStyle(.borderedProminent)
                .disabled(!canAdd)
                if working { ProgressView().controlSize(.small) }
                Spacer()
            }
            if project == nil {
                Text("Choose a project first.").font(.caption).foregroundStyle(.secondary)
            } else if doctorChecked && manimgx?.found != true {
                Text("Adding a chart needs manimgx (Settings → Doctor).").font(.caption).foregroundStyle(.orange)
            }
            Text("Add overlays last: cut-silence, punch-in and attach-bridge refuse a project with overlays.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var canAdd: Bool {
        project != nil && head != nil && !working && draft.problems.isEmpty && manimgx?.found == true && timeoutProblem == nil
    }

    private var timeoutProblem: String? {
        let text = timeoutText.trimmingCharacters(in: .whitespaces)
        if text.isEmpty { return nil }
        guard let value = Int(text), value >= 1, value <= 3600 else { return "The timeout must be 1 to 3600 s." }
        return nil
    }

    // MARK: Actions

    private func checkDoctor() async {
        let items = await model.perform { try await $0.doctor() }
        manimgx = items?["manimgx"]
        doctorChecked = true
    }

    private func reload() async {
        guard let project else {
            head = nil
            overlays = []
            hasCaptions = false
            timelineMs = nil
            loadNote = nil
            return
        }
        guard let file = await model.headFile(of: project, role: "out") else {
            head = nil
            overlays = []
            hasCaptions = false
            timelineMs = nil
            loadNote = "This project has no timeline yet. Make one in Projects."
            return
        }
        head = file
        do {
            let data = try Data(contentsOf: file.url)
            let json = try JSONDecoder().decode(JSONValue.self, from: data)
            overlays = OverlayReader.entries(json)
            hasCaptions = OverlayReader.hasCaptions(json)
            let timeline = ProjectTimeline(json: json)
            timelineMs = timeline.rate > 0 ? Int64((Double(timeline.totalFrames) * 1000 / timeline.rate).rounded()) : nil
            loadNote = nil
        } catch {
            overlays = []
            hasCaptions = false
            timelineMs = nil
            loadNote = "The project's timeline could not be read: \(error.localizedDescription)"
        }
    }

    private func addChart() async {
        guard let project, let head, !working else { return }
        working = true
        defer { working = false }
        await checkDoctor()
        guard manimgx?.found == true else {
            model.banner = "manimgx was not found. Install it or choose it in Settings, then try again."
            return
        }
        let problems = draft.problems
        guard problems.isEmpty else {
            model.banner = problems.joined(separator: " ")
            return
        }
        let url: URL
        do {
            url = try OverlayChartFile.write(draft)
        } catch {
            model.banner = "The chart spec could not be written: \(error.localizedDescription)"
            return
        }
        guard let asset = await model.importFile(url, kind: "chart_spec") else { return }
        var args: [String: JSONValue] = [:]
        if skipInspect { args["skip_inspect"] = .bool(true) }
        if let timeout = Int(timeoutText.trimmingCharacters(in: .whitespaces)), timeout > 0 {
            args["timeout_s"] = .int(Int64(timeout))
        }
        let finalArgs = args
        let projectHash = head.hash
        let assetID = asset.id
        let submitted = await model.perform { studio in
            try await studio.submit(
                subject: project,
                command: "overlay.add-chart",
                args: finalArgs,
                inputs: ["project": .hash(projectHash), "spec": .asset(assetID)]
            )
        }
        guard let submitted else { return }
        activeJob = submitted.job
        _ = await model.waitFor(submitted.job)
        await reload()
    }

    /// `overlay.remove` with `id`, or with `captions` when `overlayID` is nil.
    private func remove(overlayID: String?) async {
        guard let project, let head, !working else { return }
        working = true
        defer { working = false }
        var args: [String: JSONValue] = [:]
        if let overlayID {
            args["id"] = .string(overlayID)
        } else {
            args["captions"] = .bool(true)
        }
        let finalArgs = args
        let projectHash = head.hash
        let submitted = await model.perform { studio in
            try await studio.submit(
                subject: project,
                command: "overlay.remove",
                args: finalArgs,
                inputs: ["project": .hash(projectHash)]
            )
        }
        guard let submitted else { return }
        activeJob = submitted.job
        _ = await model.waitFor(submitted.job)
        await reload()
    }
}

/// Reload when the project changes or any checkpoint is made.
private struct OverlaysReloadKey: Hashable {
    let project: SubjectID?
    let tick: Int
}
