import ReelCore
import SwiftUI

/// Make reels (docs/specs/mac-app.md, screen 9): `pipelines.make_reels` in its
/// three stages. Decompile, then the plan through the paid-call sheet (every
/// time, for exactly the plan's bound, ADR-031), then one card per reel.
struct MakeReelsScreen: View {
    @Environment(AppModel.self) private var model

    init() {}

    /// Shared, so a run in progress is still here after visiting another screen.
    private var run: MakeReelsRun { MakeReelsRun.shared }

    var body: some View {
        let session = run
        HStack(alignment: .top, spacing: 0) {
            VStack(spacing: 0) {
                MakeReelsForm(run: session)
                Divider()
                actions(session)
                    .padding(12)
            }
            .frame(minWidth: 380, idealWidth: 430, maxWidth: 500)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    MakeReelsPhaseStrip(phase: session.phase, failed: session.message != nil && session.phase != .reels)
                    if session.phase == .setup {
                        emptyState(session)
                    }
                    stages(session)
                }
                .padding(16)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .navigationTitle("Make reels")
        .sheet(isPresented: Binding(
            get: { session.pendingPlan != nil },
            set: { shown in if !shown && session.pendingPlan != nil { session.decline() } }
        )) {
            if let plan = session.pendingPlan {
                PaidCallSheet(
                    plan: plan,
                    model: session.request?.model ?? session.draft.modelName,
                    approve: { session.approve(plan, model: model) },
                    cancel: { session.decline() }
                )
            }
        }
    }

    private func actions(_ session: MakeReelsRun) -> some View {
        let problems = session.draft.problems
        return VStack(alignment: .leading, spacing: 6) {
            HStack {
                Button {
                    session.start(model: model)
                } label: {
                    Label("Make reels", systemImage: "wand.and.stars")
                }
                .buttonStyle(.borderedProminent)
                .disabled(!problems.isEmpty || session.isRunning)
                if session.isRunning { ProgressView().controlSize(.small) }
                Spacer()
                if session.phase != .setup {
                    Button("Start over") { session.reset() }
                        .disabled(session.busy)
                }
            }
            if let first = problems.first, session.phase == .setup {
                Text(first).font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private func emptyState(_ session: MakeReelsRun) -> some View {
        VStack(spacing: 10) {
            Image(systemName: "film.stack").font(.system(size: 40)).foregroundStyle(.secondary)
            Text("Drop a long video here, or pick one on the left.").font(.title3)
            Text("Decompile measures its words, a model picks the clips (you approve the cost first), and each clip becomes a vertical reel: cut-silence, punch-ins, reframe, bridge card, captions, render and a diff against your outliers.")
                .multilineTextAlignment(.center)
                .foregroundStyle(.secondary)
                .frame(maxWidth: 520)
        }
        .frame(maxWidth: .infinity, minHeight: 240)
        .background(RoundedRectangle(cornerRadius: 12).strokeBorder(.quaternary, style: StrokeStyle(lineWidth: 2, dash: [6])))
        .dropImport(kind: "video") { asset in session.draft.video = asset.id }
    }

    @ViewBuilder
    private func stages(_ session: MakeReelsRun) -> some View {
        if let job = session.decompileJob {
            GroupBox("Decompile") {
                JobProgressView(job: job).frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        if let plan = session.lastPlan {
            GroupBox("Plan") {
                VStack(alignment: .leading, spacing: 6) {
                    Text("\(plan.frames) frames, \(plan.images) images, at most \(plan.approvalText) with \(session.request?.model ?? session.draft.modelName).")
                    if let approved = session.approvedUsd {
                        Label("Approved up to \(formatUsd(approved)).", systemImage: "checkmark.seal").foregroundStyle(.green)
                    } else if session.declined {
                        HStack {
                            Label("Not approved; nothing was sent.", systemImage: "hand.raised").foregroundStyle(.orange)
                            Button("Ask again") { session.askAgain(model: model) }
                                .disabled(session.busy)
                        }
                    } else if session.pendingPlan != nil {
                        Text("Waiting for your answer.").foregroundStyle(.secondary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        if let job = session.understandJob {
            GroupBox("Understand") {
                JobProgressView(job: job).frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        if let message = session.message {
            Label(message, systemImage: "info.circle")
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
        if let stage = session.stage, stage.stage == "reels" {
            reels(stage)
        }
    }

    @ViewBuilder
    private func reels(_ stage: MakeReelsStage) -> some View {
        let list = stage.reels ?? []
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(verbatim: list.isEmpty ? "No clip was chosen." : "\(list.count) reel\(list.count == 1 ? "" : "s")").font(.title3.bold())
                Spacer()
                Button("Open in Reels") { model.selection = .reels }
                    .buttonStyle(.borderless)
            }
            if let follow = stage.follow {
                Text(follow == "centre" ? "Framing: centre crop for at least one reel." : "Framing: follows the presenter.")
                    .font(.callout)
                    .foregroundStyle(follow == "centre" ? Color.orange : Color.secondary)
            }
            ForEach(stage.notes ?? [], id: \.self) { note in
                Label(note, systemImage: "note.text").font(.callout)
            }
        }
        if list.isEmpty {
            Text("Every clip may be flagged: try Allow flagged clips, or name clips by rank.")
                .foregroundStyle(.secondary)
        }
        ForEach(list, id: \.subject) { reel in
            MakeReelsReelCard(reel: reel)
        }
    }
}

/// Decompile → Plan and approve → Understand → Reels.
struct MakeReelsPhaseStrip: View {
    let phase: MakeReelsPhase
    let failed: Bool

    var body: some View {
        HStack(spacing: 10) {
            ForEach(MakeReelsPhase.allCases.filter { $0 != .setup }, id: \.self) { item in
                HStack(spacing: 4) {
                    Image(systemName: symbol(item)).foregroundStyle(color(item))
                    Text(item.title).font(.callout).foregroundStyle(item == phase ? Color.primary : Color.secondary)
                }
                if item != .reels {
                    Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
                }
            }
        }
    }

    private func symbol(_ item: MakeReelsPhase) -> String {
        if item < phase || (item == .reels && phase == .reels) { return "checkmark.circle.fill" }
        if item == phase { return failed ? "exclamationmark.circle.fill" : "circle.dotted" }
        return "circle"
    }

    private func color(_ item: MakeReelsPhase) -> Color {
        if item < phase || (item == .reels && phase == .reels) { return .green }
        if item == phase { return failed ? .orange : .blue }
        return .secondary
    }
}
