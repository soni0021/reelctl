import Foundation

// The files the screens read from the store: edit_dna.json (docs/edit-dna.md),
// understanding.json, and diff's report. Only the fields the app shows are
// typed; everything is optional where the schema allows `null`.

public struct EditDNA: Codable, Sendable, Hashable {
    public struct FrameSize: Codable, Sendable, Hashable {
        public let width: Int
        public let height: Int
    }

    public struct Cut: Codable, Sendable, Hashable, Identifiable {
        public let frameIndex: Int64
        public let timeMs: Int64
        public let intervalMs: Int64
        public let sceneScore: Double
        public var id: Int64 { frameIndex }

        enum CodingKeys: String, CodingKey {
            case frameIndex = "frame_index"
            case timeMs = "time_ms"
            case intervalMs = "interval_ms"
            case sceneScore = "scene_score"
        }
    }

    public struct VisualEvent: Codable, Sendable, Hashable, Identifiable {
        public let timeMs: Int64
        public let frameIndex: Int64
        /// `punch_in`, `zoom_out` or `overlay_change`.
        public let kind: String
        public let scale: Double?
        public let centerX: Double?
        public let centerY: Double?
        public let area: Double?
        public var id: String { "\(frameIndex)-\(kind)" }

        enum CodingKeys: String, CodingKey {
            case kind, scale, area
            case timeMs = "time_ms"
            case frameIndex = "frame_index"
            case centerX = "center_x"
            case centerY = "center_y"
        }
    }

    public struct SilenceGap: Codable, Sendable, Hashable, Identifiable {
        public let startMs: Int64
        public let endMs: Int64
        public let durationMs: Int64
        public var id: Int64 { startMs }
        /// Longer than 180 ms: what `diff` flags as dead air.
        public var isDeadAir: Bool { durationMs > 180 }

        enum CodingKeys: String, CodingKey {
            case startMs = "start_ms"
            case endMs = "end_ms"
            case durationMs = "duration_ms"
        }
    }

    public struct Word: Codable, Sendable, Hashable, Identifiable {
        public let text: String
        public let startMs: Int64
        public let endMs: Int64
        public var id: String { "\(startMs)-\(text)" }

        enum CodingKeys: String, CodingKey {
            case text
            case startMs = "start_ms"
            case endMs = "end_ms"
        }
    }

    public struct ShotHistogram: Codable, Sendable, Hashable, Identifiable {
        public let shotIndex: Int
        public let startMs: Int64
        public let endMs: Int64
        public let meanR: Double
        public let meanG: Double
        public let meanB: Double
        public var id: Int { shotIndex }

        enum CodingKeys: String, CodingKey {
            case shotIndex = "shot_index"
            case startMs = "start_ms"
            case endMs = "end_ms"
            case meanR = "mean_r"
            case meanG = "mean_g"
            case meanB = "mean_b"
        }
    }

    public struct PayoffWindow: Codable, Sendable, Hashable {
        public let markerMs: Int64
        public let windowStartMs: Int64
        public let windowEndMs: Int64
        public let hasCue: Bool
        public let cueMs: Int64?

        enum CodingKeys: String, CodingKey {
            case markerMs = "marker_ms"
            case windowStartMs = "window_start_ms"
            case windowEndMs = "window_end_ms"
            case hasCue = "has_cue"
            case cueMs = "cue_ms"
        }
    }

    public struct ZoomCurve: Codable, Sendable, Hashable {
        public let startMs: Int64
        public let endMs: Int64
        public let fromScale: Double
        public let toScale: Double
        public let bezier: [Double]?

        enum CodingKeys: String, CodingKey {
            case bezier
            case startMs = "start_ms"
            case endMs = "end_ms"
            case fromScale = "from_scale"
            case toScale = "to_scale"
        }
    }

    public struct ZoomCurves: Codable, Sendable, Hashable {
        public let status: String
        public let reason: String
        public let curves: [ZoomCurve]
    }

    public let schemaVersion: Int
    public let durationMs: Int64
    public let fps: Double
    public let frameSize: FrameSize
    public let frameCount: Int64
    public let cuts: [Cut]
    public let visualEvents: [VisualEvent]
    public let silenceGaps: [SilenceGap]
    public let silenceThresholdDb: Double
    public let words: [Word]
    public let wordsPerMinute: Double
    public let integratedLufs: Double
    public let shotHistograms: [ShotHistogram]
    public let payoffWindow: PayoffWindow
    public let signalMs: Int64
    public let asrMs: Int64
    public let zoomCurves: ZoomCurves?
    public let cutDetector: String
    public let cutDetectorNote: String
    public let asrBackend: String
    public let asrNote: String
    public let decodeNote: String

    enum CodingKeys: String, CodingKey {
        case fps, cuts, words
        case schemaVersion = "schema_version"
        case durationMs = "duration_ms"
        case frameSize = "frame_size"
        case frameCount = "frame_count"
        case visualEvents = "visual_events"
        case silenceGaps = "silence_gaps"
        case silenceThresholdDb = "silence_threshold_db"
        case wordsPerMinute = "words_per_minute"
        case integratedLufs = "integrated_lufs"
        case shotHistograms = "shot_histograms"
        case payoffWindow = "payoff_window"
        case signalMs = "signal_ms"
        case asrMs = "asr_ms"
        case zoomCurves = "zoom_curves"
        case cutDetector = "cut_detector"
        case cutDetectorNote = "cut_detector_note"
        case asrBackend = "asr_backend"
        case asrNote = "asr_note"
        case decodeNote = "decode_note"
    }

    public static func load(_ url: URL) throws -> EditDNA {
        try JSONDecoder().decode(EditDNA.self, from: Data(contentsOf: url))
    }

    /// The frame shown at `ms`.
    public func frame(atMs ms: Int64) -> Int64 {
        Int64((Double(ms) * fps / 1000).rounded(.down))
    }

    /// Whether whisper measured the words (needed by understand and captions).
    public var hasWhisperWords: Bool { asrBackend == "whisper_cpp" && !words.isEmpty }
}

public struct Understanding: Codable, Sendable, Hashable {
    public struct SegmentRef: Codable, Sendable, Hashable {
        public let id: Int
        public let text: String
    }

    public struct Clip: Codable, Sendable, Hashable, Identifiable {
        public let rank: Int
        public let title: String
        public let startMs: Int64
        public let endMs: Int64
        public let lengthMs: Int64
        public let tightLengthMs: Int64
        public let hook: SegmentRef
        public let payoff: SegmentRef
        public let last: SegmentRef
        public let payoffAtMs: Int64?
        public let openLoop: String
        public let visualSuggestion: String
        public let why: String
        public let adjustments: [String]
        public let flags: [String]
        public var id: Int { rank }

        enum CodingKeys: String, CodingKey {
            case rank, title, hook, payoff, last, why, adjustments, flags
            case startMs = "start_ms"
            case endMs = "end_ms"
            case lengthMs = "length_ms"
            case tightLengthMs = "tight_length_ms"
            case payoffAtMs = "payoff_at_ms"
            case openLoop = "open_loop"
            case visualSuggestion = "visual_suggestion"
        }
    }

    public struct Rejected: Codable, Sendable, Hashable {
        public let rank: Int
        public let title: String
        public let reason: String
    }

    public struct Model: Codable, Sendable, Hashable {
        public let id: String
        public let provider: String
        public let costUsd: Double?
        public let costUpperBoundUsd: Double

        enum CodingKeys: String, CodingKey {
            case id, provider
            case costUsd = "cost_usd"
            case costUpperBoundUsd = "cost_upper_bound_usd"
        }
    }

    public struct Summary: Codable, Sendable, Hashable {
        public let topic: String
        public let language: String
    }

    public let schemaVersion: Int
    public let model: Model
    public let summary: Summary
    public let clips: [Clip]
    public let rejected: [Rejected]

    enum CodingKeys: String, CodingKey {
        case model, summary, clips, rejected
        case schemaVersion = "schema_version"
    }

    public static func load(_ url: URL) throws -> Understanding {
        try JSONDecoder().decode(Understanding.self, from: Data(contentsOf: url))
    }
}

/// `reelctl diff`'s JSON report.
public struct DiffReport: Codable, Sendable, Hashable {
    /// `dead_air`, `static_window` or `missing_payoff_cue`, with its times.
    public struct Flag: Codable, Sendable, Hashable, Identifiable {
        public let kind: String
        public let startMs: Int64?
        public let endMs: Int64?
        public let durationMs: Int64?
        public let windowStartMs: Int64?
        public let windowEndMs: Int64?
        public var id: String { "\(kind)-\(startMs ?? windowStartMs ?? 0)" }
        /// Where to seek the draft.
        public var atMs: Int64 { startMs ?? windowStartMs ?? 0 }

        enum CodingKeys: String, CodingKey {
            case kind
            case startMs = "start_ms"
            case endMs = "end_ms"
            case durationMs = "duration_ms"
            case windowStartMs = "window_start_ms"
            case windowEndMs = "window_end_ms"
        }
    }

    public struct Metrics: Codable, Sendable, Hashable {
        public let name: String
        public let durationMs: Int64
        public let cutCount: Int
        public let meanShotMs: Int64
        public let longestShotMs: Int64
        public let deadAirCount: Int
        public let deadAirPerMinute: Double
        public let silentShare: Double
        public let wordsPerMinute: Double
        /// Where the words came from (`whisper_cpp`, or an estimate).
        public let wordsSource: String?
        public let integratedLufs: Double

        enum CodingKeys: String, CodingKey {
            case name
            case wordsSource = "words_source"
            case durationMs = "duration_ms"
            case cutCount = "cut_count"
            case meanShotMs = "mean_shot_ms"
            case longestShotMs = "longest_shot_ms"
            case deadAirCount = "dead_air_count"
            case deadAirPerMinute = "dead_air_per_minute"
            case silentShare = "silent_share"
            case wordsPerMinute = "words_per_minute"
            case integratedLufs = "integrated_lufs"
        }
    }

    public struct Window: Codable, Sendable, Hashable, Identifiable {
        public let startMs: Int64
        public let endMs: Int64
        public let draftCuts: Int
        public let outlierCuts: [Int?]
        public var id: Int64 { startMs }

        enum CodingKeys: String, CodingKey {
            case startMs = "start_ms"
            case endMs = "end_ms"
            case draftCuts = "draft_cuts"
            case outlierCuts = "outlier_cuts"
        }
    }

    /// `rhythm_gap` or `pause_density`.
    public struct RelativeFlag: Codable, Sendable, Hashable {
        public let kind: String
        public let windowStartMs: Int64?
        public let windowEndMs: Int64?
        public let draftPerMinute: Double?
        public let outlierPerMinute: Double?
        public let outliersWithCut: Int?
        public let outliersCounted: Int?

        enum CodingKeys: String, CodingKey {
            case kind
            case outliersWithCut = "outliers_with_cut"
            case outliersCounted = "outliers_counted"
            case windowStartMs = "window_start_ms"
            case windowEndMs = "window_end_ms"
            case draftPerMinute = "draft_per_minute"
            case outlierPerMinute = "outlier_per_minute"
        }
    }

    public struct Comparison: Codable, Sendable, Hashable {
        public let draft: Metrics
        public let outliers: [Metrics]
        public let windows: [Window]
        public let flags: [RelativeFlag]
    }

    public let flags: [Flag]
    public let comparison: Comparison?

    public static func load(_ url: URL) throws -> DiffReport {
        try JSONDecoder().decode(DiffReport.self, from: Data(contentsOf: url))
    }
}

/// The `.otio` subset reelctl writes, read loosely: the clips and gaps on the
/// first video track, and `metadata.reelctl` (punch-ins, the bridge, captions,
/// overlays, the reframe).
public struct ProjectTimeline: Sendable, Hashable {
    public struct Item: Sendable, Hashable, Identifiable {
        public let id: Int
        /// `Clip` or `Gap`.
        public let kind: String
        public let name: String
        /// Where it starts on the timeline, and its length, in frames.
        public let startFrame: Int64
        public let frames: Int64
        /// Where its media starts, in frames of the source.
        public let sourceStartFrame: Int64
        public let mediaPath: String?
    }

    public let rate: Double
    public let items: [Item]
    public let reelctl: JSONValue
    public let raw: JSONValue

    public var totalFrames: Int64 { items.reduce(0) { $0 + $1.frames } }

    public init(json: JSONValue) {
        raw = json
        reelctl = json["metadata"]["reelctl"]
        let tracks = json["tracks"]["children"].array ?? []
        let video = tracks.first { $0["kind"].string == "Video" } ?? tracks.first ?? .null
        var items: [Item] = []
        var at: Int64 = 0
        var rate = 30.0
        for (index, child) in (video["children"].array ?? []).enumerated() {
            let range = child["source_range"]
            let duration = range["duration"]
            if let r = duration["rate"].double, r > 0 { rate = r }
            let frames = Int64((duration["value"].double ?? 0).rounded())
            let start = Int64((range["start_time"]["value"].double ?? 0).rounded())
            let schema = child["OTIO_SCHEMA"].string ?? ""
            let kind = schema.hasPrefix("Gap") ? "Gap" : "Clip"
            let media = child["media_references"]["DEFAULT_MEDIA"]["target_url"].string ?? child["media_reference"]["target_url"].string
            items.append(Item(id: index, kind: kind, name: child["name"].string ?? kind, startFrame: at, frames: frames, sourceStartFrame: start, mediaPath: media))
            at += frames
        }
        self.items = items
        self.rate = rate
    }

    public static func load(_ url: URL) throws -> ProjectTimeline {
        ProjectTimeline(json: try JSONDecoder().decode(JSONValue.self, from: Data(contentsOf: url)))
    }
}
