import Foundation
import ReelCore

// The part of the job catalogue (crates/studio/src/commands.rs) the editor
// submits: each command's typed arguments and its content inputs besides
// `project`. Keys are the catalogue's (snake_case); `jobs.submit` refuses any
// other key, so this list must follow commands.rs.

/// `ArgKind` in commands.rs.
enum ProjectArgKind: Hashable, Sendable {
    case str
    case int
    case float
    /// A flag, sent only when on.
    case bool
    /// An absolute path (a model, a font, a style file).
    case path
    case choice([String])
    /// Positive whole numbers, comma-separated in the field.
    case intList
}

/// One argument of a command.
struct ProjectArgSpec: Identifiable, Hashable, Sendable {
    let key: String
    let kind: ProjectArgKind
    let required: Bool
    /// The CLI default or an example, shown as the placeholder.
    let hint: String
    /// Prefilled when the form opens (for required arguments with a sensible value).
    let initial: String
    /// Stepper step for numbers.
    let step: Double
    let help: String

    var id: String { key }

    var label: String {
        let words = key.replacingOccurrences(of: "_", with: " ")
        return words.prefix(1).uppercased() + words.dropFirst()
    }

    var flag: String { "--" + key.replacingOccurrences(of: "_", with: "-") }

    init(
        _ key: String,
        _ kind: ProjectArgKind,
        required: Bool = false,
        hint: String = "",
        initial: String = "",
        step: Double = 1,
        help: String = ""
    ) {
        self.key = key
        self.kind = kind
        self.required = required
        self.hint = hint
        self.initial = initial
        self.step = step
        self.help = help
    }
}

/// What a content input (other than `project`) may come from.
enum ProjectSourceKind: Hashable, Sendable {
    /// An `edit_dna` asset or a reel subject's head `out`.
    case dna
    /// An `understanding` asset or an understanding subject's head `out`.
    case understanding

    var assetKind: String {
        switch self {
        case .dna: return "edit_dna"
        case .understanding: return "understanding"
        }
    }

    var subjectKind: String {
        switch self {
        case .dna: return "reel"
        case .understanding: return "understanding"
        }
    }
}

/// A content input slot of a form.
struct ProjectInputSlot: Identifiable, Hashable, Sendable {
    /// The role sent in `inputs` (`dna`, `outlier`, `outlier.2`, ...).
    let role: String
    let title: String
    let source: ProjectSourceKind
    let required: Bool

    var id: String { role }
}

enum ProjectCatalogue {
    /// `CAPTION_STYLE` in commands.rs, in its order.
    static let captionStyle: [ProjectArgSpec] = [
        ProjectArgSpec("preset", .choice(["bold", "clean", "boxed", "karaoke", "minimal"]), help: "The base style; the fields below override it."),
        ProjectArgSpec("style", .path, hint: "/path/to/style.json", help: "A style JSON file (the flag names with underscores)."),
        ProjectArgSpec("font", .path, hint: "/Library/Fonts/Arial Bold.ttf", help: ".ttf or .otf."),
        ProjectArgSpec("font_size", .float, hint: "2–12", step: 0.5, help: "Percent of the output height."),
        ProjectArgSpec("color", .str, hint: "#ffffff"),
        ProjectArgSpec("highlight_color", .str, hint: "#ffd400 or none"),
        ProjectArgSpec("outline_color", .str, hint: "#000000"),
        ProjectArgSpec("outline_width", .int, hint: "0–12", help: "Pixels at 1080 wide."),
        ProjectArgSpec("shadow", .int, hint: "0–12"),
        ProjectArgSpec("box_color", .str, hint: "#000000"),
        ProjectArgSpec("box_opacity", .float, hint: "0–1", step: 0.05),
        ProjectArgSpec("position", .choice(["top", "middle", "bottom"])),
        ProjectArgSpec("margin", .float, hint: "0–40", step: 1, help: "Percent of the output height."),
        ProjectArgSpec("words_per_group", .int, hint: "1–5"),
        ProjectArgSpec("max_chars", .int, hint: "8–40", help: "Then the group wraps to two lines."),
        ProjectArgSpec("case", .choice(["as-is", "upper", "lower"])),
        ProjectArgSpec("min_gap_ms", .int, hint: "100–2000", step: 50, help: "A pause this long starts a new group."),
    ]

    static let initArgs: [ProjectArgSpec] = []

    static let fromClipArgs: [ProjectArgSpec] = [
        ProjectArgSpec("clip", .int, required: true, hint: "1", initial: "1", help: "The clip's place in understanding.json's clips, from 1."),
        ProjectArgSpec("allow_flagged", .bool, help: "Use a clip that breaks a clip rule."),
    ]
}

/// The editing operations on a project (spec: Projects, the editor).
enum ProjectOperation: String, CaseIterable, Identifiable, Sendable {
    case cutSilence = "timeline.cut-silence"
    case punchIn = "transform.punch-in"
    case autoPunchIn = "transform.auto-punch-in"
    case reframe = "transform.reframe"
    case attachBridge = "funnel.attach-bridge"
    case captions = "overlay.captions"
    case removeOverlay = "overlay.remove"
    case render = "render"
    case inspect = "qa.inspect-boundaries"

    var id: String { rawValue }
    var command: String { rawValue }

    var title: String {
        switch self {
        case .cutSilence: return "Cut silence"
        case .punchIn: return "Punch in"
        case .autoPunchIn: return "Auto punch-in"
        case .reframe: return "Reframe"
        case .attachBridge: return "Attach bridge"
        case .captions: return "Captions"
        case .removeOverlay: return "Remove overlay"
        case .render: return "Render"
        case .inspect: return "Inspect joins"
        }
    }

    var symbol: String {
        switch self {
        case .cutSilence: return "scissors"
        case .punchIn: return "plus.magnifyingglass"
        case .autoPunchIn: return "wand.and.rays"
        case .reframe: return "crop"
        case .attachBridge: return "link"
        case .captions: return "captions.bubble"
        case .removeOverlay: return "minus.rectangle"
        case .render: return "film"
        case .inspect: return "eye"
        }
    }

    var summary: String {
        switch self {
        case .cutSilence: return "Removes silence gaps (and filler words with whisper words) found in the edit DNA."
        case .punchIn: return "A centred zoom over the span selected on the track (at least 12 frames)."
        case .autoPunchIn: return "Punch-ins wherever one picture holds longer than 2.5 s; the scale follows the outliers."
        case .reframe: return "A crop window of the aspect that follows the presenter (RobustVideoMatting) or stays centred."
        case .attachBridge: return "Cuts at a share of the timeline (or appends) and adds a card with the URL."
        case .captions: return "Burns whisper word captions in. Add captions last: earlier edits refuse a captioned project."
        case .removeOverlay: return "Takes a chart overlay off by its id, or the captions."
        case .render: return "H.264 and AAC; plays here when it finishes."
        case .inspect: return "One timeline view per join: two frames each side, the waveform and the word ticks."
        }
    }

    var args: [ProjectArgSpec] {
        switch self {
        case .cutSilence:
            return [
                ProjectArgSpec("min_gap_ms", .int, hint: "180", step: 10),
                ProjectArgSpec("fade_ms", .int, hint: "30", step: 5),
                ProjectArgSpec("fillers", .str, hint: "um,uh", help: "Whole words, comma-separated; needs whisper words."),
            ]
        case .punchIn:
            return [
                ProjectArgSpec("start_ms", .int, required: true, hint: "drag on the track", step: 100),
                ProjectArgSpec("end_ms", .int, required: true, hint: "drag on the track", step: 100),
                ProjectArgSpec("scale", .float, required: true, hint: "1.18", initial: "1.18", step: 0.01, help: "Above 1, at most 2."),
            ]
        case .autoPunchIn:
            return []
        case .reframe:
            return [
                ProjectArgSpec("aspect", .str, hint: "9:16"),
                ProjectArgSpec("follow", .choice(["presenter", "centre"]), required: true, initial: "centre"),
                ProjectArgSpec("weights", .path, hint: "/path/to/rvm_mobilenetv3_fp32.onnx", help: "Or REELCTL_RVM_WEIGHTS in Settings."),
                ProjectArgSpec("downsample_ratio", .float, hint: "0.25", step: 0.05),
            ]
        case .attachBridge:
            return [
                ProjectArgSpec("target_url", .str, required: true, hint: "example.com/course"),
                ProjectArgSpec("cutoff_pct", .float, hint: "0.72", step: 0.01, help: "Not with Append."),
                ProjectArgSpec("append", .bool, help: "Add the card after the last frame and cut nothing."),
                ProjectArgSpec("card_ms", .int, hint: "2500", step: 100, help: "At most 60000."),
            ]
        case .captions:
            return ProjectCatalogue.captionStyle
        case .removeOverlay:
            return [
                ProjectArgSpec("id", .str, hint: "overlay id", help: "The 16 hex digits add-chart printed."),
                ProjectArgSpec("captions", .bool, help: "Remove the captions instead."),
            ]
        case .render:
            return [
                ProjectArgSpec("size", .str, hint: "1080x1920", help: "Default: the source (or the reframe window)."),
                ProjectArgSpec("backend", .choice(["ffmpeg", "smelter"])),
                ProjectArgSpec("smelter", .path, hint: "/path/to/smelter"),
            ]
        case .inspect:
            return []
        }
    }

    var inputs: [ProjectInputSlot] {
        switch self {
        case .cutSilence, .captions:
            return [ProjectInputSlot(role: "dna", title: "Edit DNA", source: .dna, required: true)]
        case .autoPunchIn:
            return [
                ProjectInputSlot(role: "dna", title: "Edit DNA", source: .dna, required: true),
                ProjectInputSlot(role: "outlier", title: "Outlier 1", source: .dna, required: true),
                ProjectInputSlot(role: "outlier.2", title: "Outlier 2", source: .dna, required: false),
                ProjectInputSlot(role: "outlier.3", title: "Outlier 3", source: .dna, required: false),
            ]
        case .reframe, .attachBridge:
            return [ProjectInputSlot(role: "dna", title: "Edit DNA", source: .dna, required: false)]
        case .punchIn, .removeOverlay, .render, .inspect:
            return []
        }
    }

    /// Whether its output `out` is a new `.otio` (render's is the video; QA has none).
    var editsProject: Bool {
        switch self {
        case .render, .inspect: return false
        default: return true
        }
    }

    /// Checkpoint labels (the job's command) whose `out` is not a project.
    static let nonProjectLabels: Set<String> = [
        "render", "diff", "decompile", "look", "understand", "compositor.isolate-presenter", "eval", "qa.inspect-boundaries",
    ]
}
