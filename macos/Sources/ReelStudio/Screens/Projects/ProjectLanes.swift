import Foundation
import ReelCore

// What the track view draws, read loosely from a project `.otio`
// (docs/timeline.md, "metadata.reelctl"): the main video track's clips, gaps
// and card, punch-ins (clip metadata, in each clip's own frames), caption
// groups and the reframe (timeline metadata), and chart overlays (the track
// marked `metadata.reelctl.overlays`). Only what is present is shown.

/// A span of timeline frames, `start` inclusive, `end` exclusive.
struct ProjectFrameRange: Equatable, Hashable, Sendable {
    var start: Int64
    var end: Int64

    var frames: Int64 { max(0, end - start) }
}

enum ProjectSpanStyle: Hashable, Sendable {
    case clip, gap, card, punch, caption, chart, reframe
}

struct ProjectLaneSpan: Identifiable, Hashable, Sendable {
    let id: String
    let range: ProjectFrameRange
    let title: String
    let detail: String
    let style: ProjectSpanStyle
}

struct ProjectLane: Identifiable, Hashable, Sendable {
    let id: String
    let title: String
    let spans: [ProjectLaneSpan]
    /// Single-frame marks (reframe keyframes on the timeline).
    let ticks: [Int64]
    let height: Double
}

struct ProjectLanes: Sendable {
    var rate: Double = 30
    var totalFrames: Int64 = 0
    var lanes: [ProjectLane] = []
    /// One line per recorded edit (DNA, clip pick, cuts, bridge, reframe, captions).
    var notes: [String] = []
    /// `metadata.reelctl.dna.path`.
    var dnaPath: String?
    var overlayIDs: [String] = []
    var hasCaptions = false

    static let empty = ProjectLanes()

    init() {}

    init(_ timeline: ProjectTimeline) {
        rate = timeline.rate > 0 ? timeline.rate : 30
        let meta = timeline.reelctl
        let tracks = timeline.raw["tracks"]["children"].array ?? []
        let main = tracks.first(where: { $0["kind"].string == "Video" && !Self.isOverlayTrack($0) }) ?? tracks.first ?? .null

        // The main track: clips, gaps, the card, and the punch-ins on clips.
        var mainSpans: [ProjectLaneSpan] = []
        var punches: [String: ProjectLaneSpan] = [:]
        var placed: [(position: Int64, sourceStart: Int64, frames: Int64)] = []
        var position: Int64 = 0
        for (index, child) in (main["children"].array ?? []).enumerated() {
            let frames = Self.frames(child["source_range"]["duration"])
            let sourceStart = Self.frames(child["source_range"]["start_time"])
            let range = ProjectFrameRange(start: position, end: position + frames)
            let schema = child["OTIO_SCHEMA"].string ?? ""
            let clipMeta = child["metadata"]["reelctl"]
            let media = Self.mediaReference(child)
            let name = child["name"].string ?? ""
            if schema.hasPrefix("Gap") {
                mainSpans.append(ProjectLaneSpan(id: "gap-\(index)", range: range, title: "gap", detail: "Gap, \(frames) frames", style: .gap))
            } else if !clipMeta["bridge"].isNull || (media["OTIO_SCHEMA"].string ?? "").hasPrefix("GeneratorReference") {
                let url = clipMeta["bridge"]["url"].string ?? media["parameters"]["text"].string ?? ""
                mainSpans.append(ProjectLaneSpan(id: "card-\(index)", range: range, title: "card \(url)", detail: "Bridge card: \(url), \(frames) frames", style: .card))
            } else {
                var detail = "\(name.isEmpty ? "Clip" : name): source frames \(sourceStart)–\(sourceStart + frames)"
                if let fade = clipMeta["fade_ms"].int { detail += ", \(fade) ms audio fade" }
                if let path = media["target_url"].string { detail += "\n\(path)" }
                let title = name.isEmpty ? "clip \(index + 1)" : name
                mainSpans.append(ProjectLaneSpan(id: "clip-\(index)", range: range, title: title, detail: detail, style: .clip))
                placed.append((position: position, sourceStart: sourceStart, frames: frames))
            }
            let punch = clipMeta["punch_in"]
            if let scale = punch["scale"].double, let start = punch["start"]["value"].double, let end = punch["end"]["value"].double {
                // The same span in each clip's own frames: one entry per span.
                let span = ProjectFrameRange(start: position + Int64(start.rounded()), end: position + Int64(end.rounded()))
                let key = "\(span.start)-\(span.end)-\(scale)"
                if punches[key] == nil {
                    let ramp = punch["ramp_frames"].int.map { ", \($0)-frame ramps" } ?? ""
                    punches[key] = ProjectLaneSpan(id: "punch-\(key)", range: span, title: String(format: "×%.2f", scale), detail: String(format: "Punch-in ×%.3f", scale) + ramp, style: .punch)
                }
            }
            position += frames
        }
        totalFrames = position
        lanes.append(ProjectLane(id: "video", title: "Video", spans: mainSpans, ticks: [], height: 34))
        if !punches.isEmpty {
            let spans = punches.values.sorted { $0.range.start < $1.range.start }
            lanes.append(ProjectLane(id: "punch", title: "Punch-ins", spans: spans, ticks: [], height: 20))
        }

        // Captions: groups in timeline frames.
        let captions = meta["captions"]
        if let groups = captions["groups"].array {
            hasCaptions = true
            var spans: [ProjectLaneSpan] = []
            for (index, group) in groups.enumerated() {
                guard let start = group["start"].int, let end = group["end"].int else { continue }
                let words = (group["words"].array ?? []).compactMap { $0["text"].string }.joined(separator: " ")
                spans.append(ProjectLaneSpan(id: "caption-\(index)", range: ProjectFrameRange(start: start, end: end), title: words, detail: "Caption: \(words)", style: .caption))
            }
            lanes.append(ProjectLane(id: "captions", title: "Captions", spans: spans, ticks: [], height: 20))
            var line = "Captions: \(groups.count) groups"
            if let preset = captions["style"]["preset"].string { line += ", preset \(preset)" }
            if let dropped = captions["dropped"].int, dropped > 0 { line += ", \(dropped) words dropped" }
            notes.append(line)
        }

        // Chart overlays: their own track, in timeline frames.
        var charts: [ProjectLaneSpan] = []
        for track in tracks where Self.isOverlayTrack(track) {
            var at: Int64 = 0
            for (index, child) in (track["children"].array ?? []).enumerated() {
                let frames = Self.frames(child["source_range"]["duration"])
                let overlay = child["metadata"]["reelctl"]["overlay"]
                if !overlay.isNull {
                    let id = overlay["id"].string ?? "overlay \(index)"
                    let kind = overlay["spec"]["kind"].string ?? "chart"
                    let title = overlay["spec"]["title"].string ?? ""
                    overlayIDs.append(id)
                    charts.append(ProjectLaneSpan(id: "chart-\(id)-\(at)", range: ProjectFrameRange(start: at, end: at + frames), title: "\(kind): \(title)", detail: "Overlay \(id): \(kind) \"\(title)\"", style: .chart))
                }
                at += frames
            }
        }
        if !charts.isEmpty {
            lanes.append(ProjectLane(id: "charts", title: "Overlays", spans: charts, ticks: [], height: 20))
        }

        // The reframe: a window over the whole timeline, keyframes placed
        // through the clips (they are in source frames).
        let reframe = meta["reframe"]
        if !reframe.isNull {
            let aspect = reframe["aspect"].string ?? "?"
            let follow = reframe["follow"].string ?? "?"
            let keyframes = (reframe["keyframes"].array ?? []).compactMap { $0["source_frame"].int }
            var ticks: [Int64] = []
            for frame in keyframes {
                for piece in placed where frame >= piece.sourceStart && frame < piece.sourceStart + piece.frames {
                    ticks.append(piece.position + frame - piece.sourceStart)
                }
            }
            let title = "\(aspect) · follow \(follow) · \(keyframes.count) keyframes"
            let span = ProjectLaneSpan(id: "reframe", range: ProjectFrameRange(start: 0, end: totalFrames), title: title, detail: reframe["note"].string ?? title, style: .reframe)
            lanes.append(ProjectLane(id: "reframe", title: "Reframe", spans: [span], ticks: ticks, height: 20))
            var line = "Reframe: \(title)"
            if let width = reframe["crop_width"].int, let height = reframe["crop_height"].int { line += ", crop \(width)×\(height)" }
            if let note = reframe["note"].string, !note.isEmpty { line += ". \(note)" }
            notes.append(line)
        }

        // The other records, as lines.
        if let path = meta["dna"]["path"].string {
            dnaPath = path
            notes.insert("DNA: \(URL(fileURLWithPath: path).lastPathComponent)", at: 0)
        }
        let clip = meta["clip"]
        if let index = clip["index"].int {
            var line = "Clip \(index)"
            if let start = clip["start_ms"].int, let end = clip["end_ms"].int { line += ": \(formatMs(start))–\(formatMs(end)) of the source" }
            notes.append(line)
        }
        if let cuts = meta["cuts"].array, !cuts.isEmpty {
            let fillers = cuts.filter { $0["reason"].string == "filler" }.count
            notes.append("Cut silence: \(cuts.count - fillers) silences and \(fillers) fillers removed")
        }
        let bridge = meta["bridge"]
        if let url = bridge["url"].string {
            var line = "Bridge: \(url)"
            if let card = bridge["card_ms"].int { line += ", card \(formatMs(card))" }
            if bridge["appended"].bool == true {
                line += ", appended"
            } else if let cut = bridge["cut_ms"].int {
                line += ", cut at \(formatMs(cut))"
                if bridge["in_pause"].bool == true { line += " (in a pause)" }
            }
            notes.append(line)
        }
    }

    func ms(_ frame: Int64) -> Int64 {
        Int64((Double(frame) * 1000 / rate).rounded())
    }

    func frame(ms: Int64) -> Int64 {
        Int64((Double(ms) * rate / 1000).rounded())
    }

    private static func isOverlayTrack(_ track: JSONValue) -> Bool {
        track["metadata"]["reelctl"]["overlays"].bool == true
    }

    /// A `RationalTime`'s value in frames (the project keeps one rate).
    private static func frames(_ time: JSONValue) -> Int64 {
        Int64((time["value"].double ?? 0).rounded())
    }

    private static func mediaReference(_ clip: JSONValue) -> JSONValue {
        let references = clip["media_references"]
        let key = clip["active_media_reference_key"].string ?? "DEFAULT_MEDIA"
        if !references[key].isNull { return references[key] }
        if !references["DEFAULT_MEDIA"].isNull { return references["DEFAULT_MEDIA"] }
        return clip["media_reference"]
    }
}
