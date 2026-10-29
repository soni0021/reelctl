import ReelCore
import SwiftUI

/// The live queue (spec screen 10): grouped by class, filtered by status,
/// `after` chains as indented children, and an inspector for the selected job.
struct JobsScreen: View {
    @Environment(AppModel.self) private var model
    @State private var filter: JobsFilter = .all
    @State private var selected: JobID?

    var body: some View {
        let visible = model.jobs.filter { filter.matches($0.status) }
        Group {
            if visible.isEmpty {
                ContentUnavailableView(
                    model.jobs.isEmpty ? "No jobs yet" : "No \(filter.title.lowercased()) jobs",
                    systemImage: "list.bullet.rectangle",
                    description: Text("Jobs appear here as soon as a screen queues one.")
                )
            } else {
                List(selection: $selected) {
                    ForEach(JobClass.allCases, id: \.self) { jobClass in
                        let rows = JobTree.rows(visible.filter { $0.jobClass == jobClass })
                        if !rows.isEmpty {
                            SwiftUI.Section {
                                ForEach(rows) { row in
                                    JobRowView(job: row.job, depth: row.depth)
                                        .tag(row.job.id)
                                }
                            } header: {
                                JobClassHeader(jobClass: jobClass, jobs: visible.filter { $0.jobClass == jobClass })
                            }
                        }
                    }
                }
            }
        }
        .navigationTitle("Jobs")
        .toolbar {
            ToolbarItem {
                Picker("Status", selection: $filter) {
                    ForEach(JobsFilter.allCases) { filter in
                        Text(filter.title).tag(filter)
                    }
                }
                .pickerStyle(.menu)
                .help("Show jobs with this status")
            }
            ToolbarItem {
                Button {
                    Task { await model.refresh() }
                } label: {
                    Label("Reload", systemImage: "arrow.clockwise")
                }
                .help("Reload the job list")
            }
        }
        .inspector(isPresented: Binding<Bool>(
            get: { selected != nil },
            set: { shown in if !shown { selected = nil } }
        )) {
            if let selected {
                JobInspector(jobID: selected) { newJob in self.selected = newJob }
                    .inspectorColumnWidth(min: 320, ideal: 400, max: 640)
            }
        }
    }
}

/// The status filter of the Jobs screen.
enum JobsFilter: String, CaseIterable, Identifiable, Hashable {
    case all, active, queued, blocked, running, succeeded, failed, cancelled, interrupted

    var id: String { rawValue }

    var title: String {
        switch self {
        case .all: return "All"
        case .active: return "Unfinished"
        default: return rawValue.capitalized
        }
    }

    func matches(_ status: JobStatus) -> Bool {
        switch self {
        case .all: return true
        case .active: return !status.finished
        default: return status.rawValue == rawValue
        }
    }
}

/// One row of the flattened `after` tree.
struct JobTreeRow: Identifiable {
    let job: Job
    let depth: Int
    var id: JobID { job.id }
}

/// Flattens jobs into a tree: a job is drawn under the first of its `after`
/// jobs that is in the same list; the rest are roots, newest first.
enum JobTree {
    static func rows(_ jobs: [Job]) -> [JobTreeRow] {
        let ids = Set(jobs.map(\.id))
        var children: [JobID: [Job]] = [:]
        var roots: [Job] = []
        for job in jobs {
            if let parent = job.after.first(where: { $0 != job.id && ids.contains($0) }) {
                children[parent, default: []].append(job)
            } else {
                roots.append(job)
            }
        }
        var out: [JobTreeRow] = []
        var seen = Set<JobID>()
        func visit(_ job: Job, depth: Int) {
            guard !seen.contains(job.id) else { return }
            seen.insert(job.id)
            out.append(JobTreeRow(job: job, depth: depth))
            for child in (children[job.id] ?? []).sorted(by: { $0.id < $1.id }) {
                visit(child, depth: min(depth + 1, 6))
            }
        }
        for root in roots.sorted(by: { $0.id > $1.id }) {
            visit(root, depth: 0)
        }
        // Anything left (a cycle) is shown flat rather than dropped.
        for job in jobs where !seen.contains(job.id) {
            visit(job, depth: 0)
        }
        return out
    }
}

struct JobClassHeader: View {
    @Environment(AppModel.self) private var model
    let jobClass: JobClass
    let jobs: [Job]

    var body: some View {
        let running = jobs.filter { $0.status == .running }.count
        let waiting = jobs.filter { $0.status == .queued || $0.status == .blocked }.count
        HStack {
            Text(jobClass.rawValue.capitalized)
            Spacer()
            Text("\(running) running · \(waiting) waiting\(limitText)")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var limitText: String {
        guard let limits = model.info?.limits else { return "" }
        switch jobClass {
        case .heavy: return " · limit \(limits.heavy)"
        case .light: return " · limit \(limits.light)"
        case .network: return " · limit \(limits.network)"
        }
    }
}

/// One job in the queue list.
struct JobRowView: View {
    @Environment(AppModel.self) private var model
    let job: Job
    let depth: Int

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            if depth > 0 {
                Image(systemName: "arrow.turn.down.right")
                    .foregroundStyle(.secondary)
                    .padding(.leading, CGFloat(depth - 1) * 18)
            }
            StatusDot(status: job.status).padding(.top, 4)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text("#\(job.id)").font(.callout.monospacedDigit()).foregroundStyle(.secondary)
                    Text(job.command).font(.callout.monospaced())
                    Text(subjectName).font(.callout).foregroundStyle(.secondary).lineLimit(1)
                    if job.checkpoint?.cached == true { ServiceBadge(text: "cached") }
                    if job.force { ServiceBadge(text: "forced", color: .purple) }
                    if !job.after.isEmpty {
                        ServiceBadge(text: "after " + job.after.map { "#\($0)" }.joined(separator: ", "), color: .gray)
                    }
                    Spacer(minLength: 0)
                }
                progressLine
                HStack(spacing: 10) {
                    Text("queued \(ServiceFormat.clock(job.createdAt))")
                    if let started = job.startedAt { Text("started \(ServiceFormat.clock(started))") }
                    if let finished = job.finishedAt { Text("finished \(ServiceFormat.clock(finished))") }
                    if let took = ServiceFormat.duration(from: job.startedAt, to: job.finishedAt) { Text("took \(took)") }
                    if let code = job.exitCode { Text("exit \(code)") }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                if let error = job.error, !error.isEmpty {
                    Text(error).font(.caption).foregroundStyle(.red).lineLimit(2).help(error)
                }
            }
        }
        .padding(.vertical, 2)
    }

    private var subjectName: String {
        model.subject(job.subject)?.name ?? "subject \(job.subject)"
    }

    @ViewBuilder
    private var progressLine: some View {
        if job.status == .running {
            HStack(spacing: 8) {
                if let progress = model.progress[job.id] {
                    if let fraction = progress.fraction {
                        ProgressView(value: fraction).frame(maxWidth: 220)
                        Text(String(format: "%.0f%%", fraction * 100)).font(.caption.monospacedDigit())
                    } else {
                        ProgressView().controlSize(.small)
                    }
                    Text(progress.stage).font(.caption).foregroundStyle(.secondary)
                } else {
                    ProgressView().controlSize(.small)
                    Text(latestStage ?? "running").font(.caption).foregroundStyle(.secondary)
                }
            }
        } else if job.status == .blocked {
            Text("waiting for " + job.after.map { "#\($0)" }.joined(separator: ", "))
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    /// The latest `stage` event seen live.
    private var latestStage: String? {
        guard let events = model.recentEvents[job.id] else { return nil }
        return events.last(where: { $0["event"].string == "stage" })?["name"].string
    }
}
