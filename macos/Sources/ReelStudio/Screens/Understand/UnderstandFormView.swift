import ReelCore
import SwiftUI
import UniformTypeIdentifiers

/// The left column: inputs, model settings, the plan and the run's progress.
struct UnderstandFormView: View {
    @Environment(AppModel.self) private var model
    @Binding var form: UnderstandForm
    let session: UnderstandSession
    let onPlan: () -> Void
    let onSpend: (UnderstandPlan) -> Void

    var body: some View {
        Form {
            SwiftUI.Section("Inputs") {
                SubjectPicker(kind: "understanding", selection: $form.subject)
                AssetPicker(title: "Video", kind: "video", types: UTType.videoTypes, selection: $form.video)
                UnderstandDNAPicker(selection: $form.dna)
                UnderstandDNAStatus(check: session.dnaCheck)
            }

            SwiftUI.Section("Model") {
                TextField("Model", text: $form.model, prompt: Text(UnderstandForm.defaultModel))
                Stepper("A frame every \(form.everyS) s", value: $form.everyS, in: 1...60)
                Picker("Sheets", selection: $form.sheet) {
                    Text("Auto").tag("auto")
                    Text("Single frames").tag("1")
                    Text("2 × 2").tag("2")
                    Text("3 × 3").tag("3")
                }
                Picker("Effort", selection: $form.effort) {
                    Text("Model default").tag("none")
                    Text("Low").tag("low")
                    Text("Medium").tag("medium")
                    Text("High").tag("high")
                }
                TextField("Refuse a bound over (USD)", value: $form.capUsd, format: .number)
            }

            SwiftUI.Section("Plan") {
                Text("Plan is a dry run: it counts the frames and prices the call. Nothing is sent to the model and no key is used.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                HStack {
                    Button("Plan", action: onPlan)
                        .disabled(!canPlan)
                    if session.planning {
                        ProgressView().controlSize(.small)
                        Text("Running the dry run…").font(.caption).foregroundStyle(.secondary)
                    }
                }
                if let plan = session.plan {
                    UnderstandPlanCard(
                        plan: plan,
                        modelName: session.plannedModel,
                        busy: session.running,
                        onSpend: onSpend
                    )
                }
            }

            if session.runJob != nil || session.note != nil {
                SwiftUI.Section("Call") {
                    if let job = session.runJob {
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
        }
        .formStyle(.grouped)
    }

    private var canPlan: Bool {
        form.subject != nil && form.video != nil && session.dnaCheck.isReady
            && !session.planning && !session.running && model.isConnected
    }
}

/// A plan: frames, images, the bound, the time left, and the spend button.
struct UnderstandPlanCard: View {
    @Environment(AppModel.self) private var model
    let plan: UnderstandPlan
    let modelName: String
    let busy: Bool
    let onSpend: (UnderstandPlan) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            countdownPart
            if !KeyStore.isSet {
                HStack {
                    Label("No API key is set.", systemImage: "key.slash").foregroundStyle(.red)
                    Button("Settings") { model.selection = .settings }
                }
            }
        }
    }

    private var countdownPart: some View {
        TimelineView(.periodic(from: Date(), by: 1)) { context in
            let remaining = secondsLeft(at: context.date)
            VStack(alignment: .leading, spacing: 8) {
                LabeledContent("Model", value: modelName)
                LabeledContent("Frames", value: "\(plan.frames)")
                LabeledContent("Images sent", value: "\(plan.images)")
                LabeledContent("Cost upper bound", value: plan.approvalText)
                LabeledContent("Expires in") {
                    if remaining > 0 {
                        Text(countdown(remaining)).monospacedDigit()
                            .foregroundStyle(remaining < 60 ? Color.orange : Color.primary)
                    } else {
                        Text("Expired").foregroundStyle(.red)
                    }
                }
                Text("The bound prices every input token at the highest eligible price; the real cost is usually lower.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if remaining > 0 {
                    Button("Spend up to \(plan.approvalText)…") { onSpend(plan) }
                        .buttonStyle(.borderedProminent)
                        .disabled(busy)
                } else {
                    Text("Plans last 15 minutes. Plan again to get a new amount to approve.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private func secondsLeft(at date: Date) -> Int {
        let left = Double(plan.expiresAt) / 1000 - date.timeIntervalSince1970
        return left > 0 ? Int(left.rounded(.down)) : 0
    }

    private func countdown(_ seconds: Int) -> String {
        String(format: "%d:%02d", seconds / 60, seconds % 60)
    }
}
