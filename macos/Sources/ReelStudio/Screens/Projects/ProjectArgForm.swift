import AppKit
import ReelCore
import SwiftUI

/// A refused field: the argument key or input role at fault (nil for the
/// whole form), and why.
struct ProjectFormProblem: Error, Equatable, Sendable {
    /// The command the problem belongs to.
    let command: String
    let key: String?
    let message: String

    /// The key a service or CLI message names: `cmd key: ...`, `"key"` or
    /// `--flag`. Longer keys are tried first so `min_gap_ms` wins over `gap`.
    static func field(in message: String, command: String, keys: [String]) -> String? {
        for key in keys.sorted(by: { $0.count > $1.count }) {
            let flag = "--" + key.replacingOccurrences(of: "_", with: "-")
            if message.contains("\(command) \(key):") || message.contains("\"\(key)\"") || message.contains(flag + " ") || message.hasSuffix(flag) {
                return key
            }
        }
        return nil
    }
}

/// The text and switches of a form, by argument key.
struct ProjectFormValues: Equatable {
    var text: [String: String] = [:]
    var flags: [String: Bool] = [:]

    init() {}

    init(specs: [ProjectArgSpec]) {
        for spec in specs where !spec.initial.isEmpty {
            text[spec.key] = spec.initial
        }
    }

    /// The typed `args` for `jobs.submit`, or the first field at fault. Empty
    /// optional fields are left out, so the command's default applies.
    func arguments(for specs: [ProjectArgSpec], command: String) -> Result<[String: JSONValue], ProjectFormProblem> {
        var args: [String: JSONValue] = [:]
        for spec in specs {
            let raw = (text[spec.key] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            func problem(_ message: String) -> ProjectFormProblem {
                ProjectFormProblem(command: command, key: spec.key, message: "\(spec.label): \(message)")
            }
            if case .bool = spec.kind {
                if flags[spec.key] == true { args[spec.key] = .bool(true) }
                continue
            }
            if raw.isEmpty {
                if spec.required { return .failure(problem("required.")) }
                continue
            }
            switch spec.kind {
            case .bool:
                break
            case .int:
                guard let value = Int64(raw) else { return .failure(problem("a whole number.")) }
                args[spec.key] = .int(value)
            case .float:
                guard let value = Double(raw), value.isFinite else { return .failure(problem("a number.")) }
                args[spec.key] = .double(value)
            case .str:
                if raw.hasPrefix("-") { return .failure(problem("may not start with -.")) }
                args[spec.key] = .string(raw)
            case .path:
                let path = (raw as NSString).expandingTildeInPath
                guard path.hasPrefix("/") else { return .failure(problem("an absolute path.")) }
                args[spec.key] = .string(path)
            case .choice(let choices):
                guard choices.contains(raw) else { return .failure(problem("one of \(choices.joined(separator: ", ")).")) }
                args[spec.key] = .string(raw)
            case .intList:
                var list: [JSONValue] = []
                for part in raw.split(separator: ",") {
                    let item = part.trimmingCharacters(in: .whitespaces)
                    guard let number = Int64(item), number > 0 else { return .failure(problem("positive whole numbers, comma-separated.")) }
                    list.append(.int(number))
                }
                args[spec.key] = .array(list)
            }
        }
        return .success(args)
    }
}

/// One argument's control: a text field for text and paths, a field with a
/// stepper for numbers, a switch for flags, a picker for choices.
struct ProjectArgField: View {
    let spec: ProjectArgSpec
    @Binding var values: ProjectFormValues
    let highlighted: Bool

    var body: some View {
        control
            .help(spec.help.isEmpty ? spec.flag : "\(spec.flag): \(spec.help)")
            .padding(.vertical, 1)
            .padding(.horizontal, 3)
            .overlay {
                RoundedRectangle(cornerRadius: 5)
                    .stroke(Color.red, lineWidth: highlighted ? 2 : 0)
            }
    }

    private var title: String { spec.required ? spec.label + " *" : spec.label }

    private var text: Binding<String> {
        Binding<String>(
            get: { values.text[spec.key] ?? "" },
            set: { values.text[spec.key] = $0 }
        )
    }

    private var flag: Binding<Bool> {
        Binding<Bool>(
            get: { values.flags[spec.key] ?? false },
            set: { values.flags[spec.key] = $0 }
        )
    }

    @ViewBuilder
    private var control: some View {
        switch spec.kind {
        case .bool:
            Toggle(title, isOn: flag)
        case .choice(let choices):
            Picker(title, selection: text) {
                if !spec.required {
                    Text("Default").tag("")
                }
                ForEach(choices, id: \.self) { choice in
                    Text(choice).tag(choice)
                }
            }
        case .int, .float:
            HStack(spacing: 4) {
                TextField(title, text: text, prompt: Text(spec.hint))
                Stepper(title, onIncrement: { bump(1) }, onDecrement: { bump(-1) })
                    .labelsHidden()
            }
        case .str where spec.key.hasSuffix("color"):
            HStack(spacing: 4) {
                TextField(title, text: text, prompt: Text(spec.hint))
                ColorPicker(title, selection: colour, supportsOpacity: false)
                    .labelsHidden()
            }
        case .str, .path, .intList:
            TextField(title, text: text, prompt: Text(spec.hint))
        }
    }

    private func bump(_ direction: Double) {
        let current = values.text[spec.key] ?? ""
        let base = Double(current) ?? Double(spec.hint) ?? Double(spec.initial) ?? 0
        let next = base + direction * spec.step
        if case .int = spec.kind {
            values.text[spec.key] = String(Int64(next.rounded()))
        } else {
            values.text[spec.key] = String(format: "%g", next)
        }
    }

    /// The field's `#rrggbb` as a colour, and back.
    private var colour: Binding<Color> {
        Binding<Color>(
            get: { ProjectHexColour.colour(values.text[spec.key] ?? "") ?? ProjectHexColour.colour(spec.hint) ?? Color.white },
            set: { values.text[spec.key] = ProjectHexColour.hex($0) }
        )
    }
}

enum ProjectHexColour {
    static func colour(_ text: String) -> Color? {
        var hex = text.trimmingCharacters(in: .whitespaces)
        if hex.hasPrefix("#") { hex.removeFirst() }
        guard hex.count >= 6, let value = UInt32(hex.prefix(6), radix: 16) else { return nil }
        let red = Double((value >> 16) & 0xff) / 255
        let green = Double((value >> 8) & 0xff) / 255
        let blue = Double(value & 0xff) / 255
        return Color(.sRGB, red: red, green: green, blue: blue, opacity: 1)
    }

    static func hex(_ colour: Color) -> String {
        guard let rgb = NSColor(colour).usingColorSpace(.sRGB) else { return "#ffffff" }
        let red = Int((rgb.redComponent * 255).rounded())
        let green = Int((rgb.greenComponent * 255).rounded())
        let blue = Int((rgb.blueComponent * 255).rounded())
        return String(format: "#%02x%02x%02x", max(0, min(255, red)), max(0, min(255, green)), max(0, min(255, blue)))
    }
}

/// The arguments of one command as a list of fields.
struct ProjectArgList: View {
    let specs: [ProjectArgSpec]
    @Binding var values: ProjectFormValues
    let problem: ProjectFormProblem?

    var body: some View {
        ForEach(specs) { spec in
            ProjectArgField(spec: spec, values: $values, highlighted: problem?.key == spec.key)
        }
    }
}
