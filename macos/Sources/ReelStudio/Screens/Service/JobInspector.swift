import ReelCore
import SwiftUI

/// The selected job: its request, inputs, environment names (never values),
/// events, and Cancel, Retry and "Copy as CLI command".
struct JobInspector: View {
    @Environment(AppModel.self) private var model
    let jobID: JobID
    /// Called with the new job after a retry, so the list selects it.
    let select: (JobID) -> Void

    @State private var detail: Job?
    @State private var loading = false
    @State private var copied = false

    var body: some View {
        let job: Job? = detail ?? model.job(jobID)
        Group {
            if let job {
                content(job)
            } else if loading {
                ProgressView("Loading job #\(jobID)…").frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ContentUnavailableView("Job #\(jobID) not found", systemImage: "questionmark.circle")
            }
        }
        .task(id: jobID) {
            copied = false
            detail = nil
            await reload()
        }
        .onChange(of: model.job(jobID)?.status) { _, _ in
            Task { await reload() }
        }
    }

    private func reload() async {
        loading = true
        let fetched: Job? = await model.perform { try await $0.job(jobID) }
        if let fetched { detail = fetched }
        loading = false
    }

    /// The live status (from notifications) wins over the fetched copy.
    private func status(_ job: Job) -> JobStatus {
        model.job(jobID)?.status ?? job.status
    }

    @ViewBuilder
    private func content(_ job: Job) -> some View {
        let current = status(job)
        Form {
            SwiftUI.Section {
                HStack(spacing: 8) {
                    StatusDot(status: current)
                    Text("#\(job.id)").foregroundStyle(.secondary)
                    Text(job.command).font(.headline.monospaced())
                    if job.checkpoint?.cached == true { ServiceBadge(text: "cached") }
                }
                actions(job, status: current)
                if current == .running, let progress = model.progress[jobID] {
                    if let fraction = progress.fraction {
                        ProgressView(value: fraction) { Text(progress.stage) }
                    } else {
                        ProgressView { Text(progress.stage) }
                    }
                }
            }
            SwiftUI.Section("Job") {
                FieldRow(label: "Status", value: current.rawValue)
                FieldRow(label: "Subject", value: model.subject(job.subject).map { "\($0.name) (\($0.kind))" } ?? "#\(job.subject)")
                FieldRow(label: "Class", value: job.jobClass.rawValue)
                FieldRow(label: "Priority", value: "\(job.priority)")
                FieldRow(label: "Queued", value: ServiceFormat.time(job.createdAt))
                FieldRow(label: "Started", value: ServiceFormat.time(job.startedAt))
                FieldRow(label: "Finished", value: ServiceFormat.time(job.finishedAt))
                if let took = ServiceFormat.duration(from: job.startedAt, to: job.finishedAt) {
                    FieldRow(label: "Took", value: took)
                }
                FieldRow(label: "Exit code", value: job.exitCode.map { "\($0)" } ?? "—")
                if job.force { FieldRow(label: "Forced", value: "yes (cache skipped)") }
                if !job.after.isEmpty {
                    FieldRow(label: "After", value: job.after.map { "#\($0)" }.joined(separator: ", "))
                }
                FieldRow(label: "Cache key", value: job.cacheKey)
                if let error = job.error, !error.isEmpty {
                    Text(error).foregroundStyle(.red).textSelection(.enabled)
                }
            }
            SwiftUI.Section("Arguments") {
                Text(ServiceFormat.pretty(job.args))
                    .font(.caption.monospaced())
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            SwiftUI.Section("Inputs") {
                if job.inputs.isEmpty {
                    Text("None").foregroundStyle(.secondary)
                }
                ForEach(job.inputs.keys.sorted(), id: \.self) { role in
                    FieldRow(label: role, value: inputText(job.inputs[role] ?? .null))
                }
            }
            SwiftUI.Section("Environment (names only)") {
                if job.envNames.isEmpty {
                    Text("None").foregroundStyle(.secondary)
                } else {
                    Text(job.envNames.sorted().joined(separator: "\n"))
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                }
            }
            if let result = job.result, !result.isNull {
                SwiftUI.Section("Result") {
                    Text(ServiceFormat.pretty(result))
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            if let checkpoint = job.checkpoint {
                SwiftUI.Section("Checkpoint") {
                    FieldRow(label: "Checkpoint", value: "#\(checkpoint.id) \(checkpoint.label)")
                    FieldRow(label: "Outputs", value: checkpoint.outputs.keys.sorted().joined(separator: ", "))
                    if !checkpoint.note.isEmpty { FieldRow(label: "Note", value: checkpoint.note) }
                    Button("Show in History") {
                        model.focusedSubject = checkpoint.subject
                        model.selection = .history
                    }
                }
            }
            SwiftUI.Section("Live events") {
                let live = model.recentEvents[jobID] ?? []
                if live.isEmpty {
                    Text("None since the app connected").foregroundStyle(.secondary)
                }
                ForEach(Array(live.enumerated().reversed()), id: \.offset) { item in
                    eventRow(item.element)
                }
            }
            SwiftUI.Section("Stored events (latest)") {
                let stored = job.events ?? []
                if stored.isEmpty {
                    Text("None").foregroundStyle(.secondary)
                }
                ForEach(stored.reversed(), id: \.seq) { event in
                    HStack(alignment: .top) {
                        Text("\(event.seq)").font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                        eventRow(event.event)
                    }
                }
            }
        }
        .formStyle(.grouped)
    }

    private func eventRow(_ event: JSONValue) -> some View {
        Text(ServiceFormat.eventLine(event))
            .font(.caption.monospaced())
            .foregroundStyle(ServiceFormat.eventIsProblem(event) ? Color.orange : Color.primary)
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private func actions(_ job: Job, status: JobStatus) -> some View {
        HStack {
            if !status.finished {
                Button("Cancel", role: .destructive) {
                    Task {
                        await model.perform { try await $0.cancel(jobID) }
                        await model.reloadJob(jobID)
                        await reload()
                    }
                }
            }
            if status.finished {
                Button("Retry") {
                    Task {
                        if let newJob = await model.perform({ try await $0.retry(jobID) }) {
                            await model.reloadJob(newJob)
                            select(newJob)
                        }
                    }
                }
                .disabled(job.command.hasPrefix("understand"))
                .help(job.command.hasPrefix("understand")
                      ? "A model call is never retried; plan it again in Understand."
                      : "Queue a new job with the same request")
            }
            Button(copied ? "Copied" : "Copy as CLI command") {
                Task {
                    if let line = await model.perform({ try await $0.commandLine(jobID) }) {
                        serviceCopy(line)
                        copied = true
                    }
                }
            }
            .help("The job as a reelctl command line, with the stored input paths. It never holds a key.")
        }
    }

    private func inputText(_ value: JSONValue) -> String {
        guard let ref = InputRef(json: value) else { return value.text }
        switch ref {
        case .hash(let hash):
            return "object " + String(hash.prefix(16)) + (hash.count > 16 ? "…" : "")
        case .fromJob(let job, let role):
            return "output \(role) of job #\(job)"
        case .asset(let asset):
            if let found = model.assets.first(where: { $0.id == asset }) {
                return "\(found.name) (asset #\(asset))\n\(found.sourcePath)"
            }
            return "asset #\(asset)"
        }
    }
}
