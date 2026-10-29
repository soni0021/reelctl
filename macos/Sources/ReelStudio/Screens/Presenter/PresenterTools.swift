import Foundation
import ReelCore

/// A tool `compositor.isolate-presenter` needs that `doctor.check` did not find.
struct PresenterMissingTool: Identifiable, Hashable {
    let key: String
    let title: String
    let detail: String

    var id: String { key }
}

/// What the screen checks before it submits: the RVM weights and the ONNX
/// Runtime library (and the ffmpeg tools), from `doctor.check`.
enum PresenterTools {
    /// The weights path to send: the Settings path when it is set, else the one
    /// the service found. Only an absolute path (the catalogue's `Path` kind).
    static func weights(doctor: [String: DoctorItem], setting: String) -> String? {
        let set = setting.trimmingCharacters(in: .whitespacesAndNewlines)
        if !set.isEmpty {
            return set.hasPrefix("/") ? set : nil
        }
        guard let item = doctor["rvm_weights"], item.found, let path = item.path, path.hasPrefix("/") else {
            return nil
        }
        return path
    }

    static func missing(doctor: [String: DoctorItem], setting: String) -> [PresenterMissingTool] {
        var missing: [PresenterMissingTool] = []
        let set = setting.trimmingCharacters(in: .whitespacesAndNewlines)
        let rvm = doctor["rvm_weights"]
        if rvm?.found != true {
            missing.append(PresenterMissingTool(
                key: "rvm_weights",
                title: "RobustVideoMatting weights",
                detail: "Choose rvm_mobilenetv3_fp32.onnx (or _fp16) in Settings (\(rvm?.variable ?? "REELCTL_RVM_WEIGHTS")). The file is never downloaded for you."
            ))
        } else if !set.isEmpty && !set.hasPrefix("/") {
            missing.append(PresenterMissingTool(
                key: "rvm_weights",
                title: "RobustVideoMatting weights",
                detail: "The path in Settings is not absolute: \(set)"
            ))
        } else if weights(doctor: doctor, setting: setting) == nil {
            missing.append(PresenterMissingTool(
                key: "rvm_weights",
                title: "RobustVideoMatting weights",
                detail: "The service found weights but gave no absolute path; choose the file in Settings."
            ))
        }
        let onnx = doctor["onnx_runtime"]
        if onnx?.found != true {
            missing.append(PresenterMissingTool(
                key: "onnx_runtime",
                title: "ONNX Runtime library",
                detail: "Choose libonnxruntime.dylib (1.17 or newer, e.g. from `brew install onnxruntime`) in Settings (\(onnx?.variable ?? "ORT_DYLIB_PATH"), an absolute path)."
            ))
        }
        for tool in ["ffmpeg", "ffprobe"] {
            if let item = doctor[tool], !item.found {
                missing.append(PresenterMissingTool(
                    key: tool,
                    title: tool,
                    detail: "Install it (for example `brew install ffmpeg`) so the service finds it on PATH."
                ))
            }
        }
        return missing
    }
}
