import Foundation
import Security

/// Starts `reelctl serve` for a workspace when its socket does not answer, and
/// connects to it.
public enum Daemon {
    /// The socket of a workspace.
    public static func socketPath(workspace: URL) -> String {
        workspace.appendingPathComponent(".reelstudio/reelctl.sock").path
    }

    /// The `reelctl` inside the app bundle (`Contents/MacOS/reelctl`), else one on PATH.
    public static func bundledReelctl() -> URL? {
        let bundled = Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/reelctl")
        if FileManager.default.isExecutableFile(atPath: bundled.path) { return bundled }
        let path = ProcessInfo.processInfo.environment["PATH"] ?? "/usr/bin:/bin"
        for dir in (path + ":/opt/homebrew/bin").split(separator: ":") {
            let candidate = URL(fileURLWithPath: String(dir)).appendingPathComponent("reelctl")
            if FileManager.default.isExecutableFile(atPath: candidate.path) { return candidate }
        }
        return nil
    }

    /// The environment for the service: this process's, with `/opt/homebrew/bin`
    /// on PATH and the tool paths from Settings. The API key is never in it.
    public static func environment(tools: [String: String]) -> [String: String] {
        var environment = ProcessInfo.processInfo.environment
        environment.removeValue(forKey: "OPENROUTER_API_KEY")
        let path = environment["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin"
        if !path.split(separator: ":").contains("/opt/homebrew/bin") {
            environment["PATH"] = "/opt/homebrew/bin:" + path
        }
        for (name, value) in tools where !value.isEmpty {
            environment[name] = value
        }
        return environment
    }

    /// Connect to the workspace's service, starting it first when nothing answers.
    /// Waits up to `timeout` for the socket.
    public static func connect(
        workspace: URL,
        reelctl: URL?,
        tools: [String: String] = [:],
        timeout: TimeInterval = 10
    ) async throws -> RPCClient {
        let socket = socketPath(workspace: workspace)
        if let client = try? RPCClient(socketPath: socket) { return client }
        guard let reelctl else {
            throw RPCError.transport("reelctl was not found in the app or on PATH")
        }
        try FileManager.default.createDirectory(at: workspace, withIntermediateDirectories: true)
        let process = Process()
        process.executableURL = reelctl
        process.arguments = ["serve", "--workspace", workspace.path]
        process.environment = environment(tools: tools)
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        let log = FileManager.default.temporaryDirectory.appendingPathComponent("ReelStudio-serve.log")
        FileManager.default.createFile(atPath: log.path, contents: nil)
        process.standardError = try? FileHandle(forWritingTo: log)
        try process.run()
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if let client = try? RPCClient(socketPath: socket) { return client }
            if !process.isRunning {
                let said = (try? String(contentsOf: log, encoding: .utf8)) ?? ""
                throw RPCError.transport("reelctl serve stopped: \(said.trimmingCharacters(in: .whitespacesAndNewlines))")
            }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        throw RPCError.transport("reelctl serve did not open \(socket) within \(Int(timeout)) s")
    }
}

/// The OpenRouter key in the Keychain: a generic password, service
/// `ReelStudio`, account `openrouter`. The data-protection keychain first; the
/// login keychain when that is refused (an ad-hoc signed app may not get it).
public enum KeyStore {
    public static let service = "ReelStudio"
    public static let account = "openrouter"

    private static func query(dataProtection: Bool) -> [String: Any] {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        if dataProtection { query[kSecUseDataProtectionKeychain as String] = true }
        return query
    }

    public static func read() -> String? {
        for dataProtection in [true, false] {
            var query = query(dataProtection: dataProtection)
            query[kSecReturnData as String] = true
            query[kSecMatchLimit as String] = kSecMatchLimitOne
            var item: CFTypeRef?
            if SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
               let data = item as? Data,
               let key = String(data: data, encoding: .utf8),
               !key.isEmpty
            {
                return key
            }
        }
        return nil
    }

    public static var isSet: Bool { read() != nil }

    /// Store `key`, replacing any earlier one. Returns which keychain took it.
    @discardableResult
    public static func write(_ key: String) throws -> String {
        delete()
        let data = Data(key.utf8)
        for dataProtection in [true, false] {
            var query = query(dataProtection: dataProtection)
            query[kSecValueData as String] = data
            query[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlocked
            let status = SecItemAdd(query as CFDictionary, nil)
            if status == errSecSuccess { return dataProtection ? "data protection keychain" : "login keychain" }
        }
        throw RPCError(code: RPCError.internalError, message: "the Keychain refused to store the key")
    }

    public static func delete() {
        for dataProtection in [true, false] {
            SecItemDelete(query(dataProtection: dataProtection) as CFDictionary)
        }
    }
}

/// A `.cube` 3D LUT, as Core Image's `CIColorCube` wants it: RGBA floats, red
/// changing fastest (the order the file lists them in).
public struct CubeLUT: Sendable, Hashable {
    public let title: String
    public let size: Int
    /// `size³ × 4` floats.
    public let rgba: [Float]

    public enum Failure: Error, Equatable {
        case noSize
        case wrongCount(expected: Int, found: Int)
        case badLine(String)
    }

    public init(text: String) throws {
        var title = ""
        var size = 0
        var values: [Float] = []
        for raw in text.split(whereSeparator: \.isNewline) {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.isEmpty || line.hasPrefix("#") { continue }
            if line.hasPrefix("TITLE") {
                title = line.dropFirst(5).trimmingCharacters(in: .whitespaces).trimmingCharacters(in: CharacterSet(charactersIn: "\""))
            } else if line.hasPrefix("LUT_3D_SIZE") {
                size = Int(line.split(separator: " ").last ?? "") ?? 0
            } else if line.hasPrefix("DOMAIN_") || line.hasPrefix("LUT_1D_SIZE") {
                continue
            } else {
                let parts = line.split(whereSeparator: { $0 == " " || $0 == "\t" }).compactMap { Float($0) }
                guard parts.count == 3 else { throw Failure.badLine(line) }
                values.append(contentsOf: [parts[0], parts[1], parts[2], 1])
            }
        }
        guard size > 1 else { throw Failure.noSize }
        let expected = size * size * size
        guard values.count == expected * 4 else { throw Failure.wrongCount(expected: expected, found: values.count / 4) }
        self.title = title
        self.size = size
        self.rgba = values
    }

    public init(url: URL) throws {
        try self.init(text: String(contentsOf: url, encoding: .utf8))
    }

    /// The bytes for `CIColorCubeWithColorSpace`'s `inputCubeData`.
    public var data: Data {
        rgba.withUnsafeBufferPointer { Data(buffer: $0) }
    }
}

/// What an exit code in an error means to the user (spec: 2 points at a field,
/// 3 opens the key setting, 4 opens the cost sheet).
public enum ErrorMeaning: Sendable, Equatable {
    case badArgument
    case needsKey
    case overBudget
    case other

    public init(_ error: Error) {
        guard let rpc = error as? RPCError else { self = .other; return }
        switch rpc.exitCode {
        case 2: self = .badArgument
        case 3: self = .needsKey
        case 4: self = .overBudget
        default: self = rpc.code == RPCError.invalidParams ? .badArgument : .other
        }
    }
}

/// Stored objects are named by hash, with no extension, and AVFoundation picks
/// a reader by extension. `Playable.url(for:)` returns a link to the file in the
/// temporary folder with the extension its first bytes call for (`.mov` for a
/// QuickTime file, else `.mp4`); the store itself is never written.
public enum Playable {
    public static func url(for file: URL) -> URL {
        guard file.pathExtension.isEmpty else { return file }
        let header = (try? FileHandle(forReadingFrom: file)).flatMap { handle -> Data? in
            defer { try? handle.close() }
            return try? handle.read(upToCount: 12)
        } ?? Data()
        let brand = header.count >= 12 ? String(decoding: header[8..<12], as: UTF8.self) : ""
        let ext = brand == "qt  " ? "mov" : "mp4"
        let folder = FileManager.default.temporaryDirectory.appendingPathComponent("ReelStudio-play", isDirectory: true)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let link = folder.appendingPathComponent(file.lastPathComponent).appendingPathExtension(ext)
        if (try? link.checkResourceIsReachable()) != true {
            try? FileManager.default.createSymbolicLink(at: link, withDestinationURL: file)
        }
        return link
    }
}
