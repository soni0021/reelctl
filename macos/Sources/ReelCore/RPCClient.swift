import Foundation

#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// `close(2)`, named apart from `RPCClient.close()`.
private func closeDescriptor(_ fd: Int32) {
    #if canImport(Darwin)
    _ = Darwin.close(fd)
    #else
    _ = Glibc.close(fd)
    #endif
}

/// A JSON-RPC error from the service, or a transport failure.
public struct RPCError: Error, LocalizedError, Sendable, Equatable {
    public let code: Int
    public let message: String
    /// The child's exit code (2 to 4) when that is the cause: 2 a bad argument,
    /// 3 no API key, 4 the cost bound.
    public let exitCode: Int?

    public init(code: Int, message: String, exitCode: Int? = nil) {
        self.code = code
        self.message = message
        self.exitCode = exitCode
    }

    public var errorDescription: String? { message }

    public static let parse = -32700
    public static let invalidRequest = -32600
    public static let methodNotFound = -32601
    public static let invalidParams = -32602
    public static let refused = -32000
    public static let internalError = -32603
    /// Not from the service: the socket closed or could not be reached.
    public static let transport = -1

    static func transport(_ message: String) -> RPCError {
        RPCError(code: transport, message: message)
    }
}

/// A notification the service pushed (after `events.subscribe`).
public struct RPCNotification: Sendable, Equatable {
    public let method: String
    public let params: JSONValue
}

/// One connection to `reelctl serve`'s Unix socket: newline-delimited JSON-RPC
/// 2.0. Calls may overlap; replies are matched by id. Notifications go to
/// `onNotification`, on the reader thread.
public final class RPCClient: @unchecked Sendable {
    private let fd: Int32
    private let lock = NSLock()
    private var nextID: Int64 = 0
    private var pending: [Int64: CheckedContinuation<JSONValue, Error>] = [:]
    private var closed = false
    private var handler: (@Sendable (RPCNotification) -> Void)?
    private var reader: Thread?

    /// Connect to the socket at `path`. Throws when nothing listens there.
    public init(socketPath path: String) throws {
        let fd = socket(AF_UNIX, Int32(SOCK_STREAM), 0)
        guard fd >= 0 else { throw RPCError.transport("socket(): \(String(cString: strerror(errno)))") }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        let capacity = MemoryLayout.size(ofValue: address.sun_path)
        guard bytes.count < capacity else {
            closeDescriptor(fd)
            throw RPCError.transport("the socket path is longer than \(capacity - 1) bytes: \(path)")
        }
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            buffer.copyBytes(from: bytes)
            buffer[bytes.count] = 0
        }
        #if canImport(Darwin)
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        var one: Int32 = 1
        setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
        #endif
        let result = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard result == 0 else {
            let reason = String(cString: strerror(errno))
            closeDescriptor(fd)
            throw RPCError.transport("cannot connect to \(path): \(reason)")
        }
        self.fd = fd
        let thread = Thread { [weak self] in self?.readLoop() }
        thread.name = "ReelCore.RPCClient.reader"
        reader = thread
        thread.start()
    }

    deinit { shutdownSocket() }

    /// Where pushed notifications go. Set it before `events.subscribe`.
    public func onNotification(_ handler: @escaping @Sendable (RPCNotification) -> Void) {
        lock.lock()
        self.handler = handler
        lock.unlock()
    }

    public var isClosed: Bool {
        lock.lock()
        defer { lock.unlock() }
        return closed
    }

    /// Close the connection; calls waiting get a transport error.
    public func close() { shutdownSocket() }

    private func shutdownSocket() {
        lock.lock()
        let wasClosed = closed
        closed = true
        let waiting = pending
        pending = [:]
        lock.unlock()
        if !wasClosed {
            shutdown(fd, Int32(SHUT_RDWR))
            closeDescriptor(fd)
        }
        for continuation in waiting.values {
            continuation.resume(throwing: RPCError.transport("the connection to reelctl serve closed"))
        }
    }

    /// Send `method` with `params` (an object) and wait for its result.
    public func call(_ method: String, _ params: JSONValue = .object([:])) async throws -> JSONValue {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<JSONValue, Error>) in
            lock.lock()
            if closed {
                lock.unlock()
                continuation.resume(throwing: RPCError.transport("the connection to reelctl serve is closed"))
                return
            }
            nextID += 1
            let id = nextID
            pending[id] = continuation
            lock.unlock()
            let request: JSONValue = ["jsonrpc": "2.0", "id": .int(id), "method": .string(method), "params": params]
            do {
                try send(request.text + "\n")
            } catch {
                lock.lock()
                let waiting = pending.removeValue(forKey: id)
                lock.unlock()
                waiting?.resume(throwing: error)
            }
        }
    }

    /// Send a typed request and decode a typed result.
    public func call<Result: Decodable>(_ method: String, _ params: JSONValue = .object([:]), as type: Result.Type) async throws -> Result {
        let value = try await call(method, params)
        do {
            return try Wire.decode(type, from: value)
        } catch {
            throw RPCError(code: RPCError.internalError, message: "\(method): cannot read the reply: \(error)")
        }
    }

    private func send(_ text: String) throws {
        let bytes = Array(text.utf8)
        var written = 0
        while written < bytes.count {
            let count = bytes[written...].withUnsafeBytes { buffer in
                write(fd, buffer.baseAddress, buffer.count)
            }
            if count < 0 {
                if errno == EINTR { continue }
                throw RPCError.transport("write to reelctl serve: \(String(cString: strerror(errno)))")
            }
            written += count
        }
    }

    private func readLoop() {
        var buffer = Data()
        var chunk = [UInt8](repeating: 0, count: 65_536)
        while true {
            let count = chunk.withUnsafeMutableBytes { read(fd, $0.baseAddress, $0.count) }
            if count < 0 && errno == EINTR { continue }
            if count <= 0 { break }
            buffer.append(contentsOf: chunk[0..<count])
            while let newline = buffer.firstIndex(of: 0x0A) {
                let line = buffer[buffer.startIndex..<newline]
                buffer.removeSubrange(buffer.startIndex...newline)
                if !line.isEmpty { handle(Data(line)) }
            }
        }
        shutdownSocket()
    }

    private func handle(_ line: Data) {
        guard let message = try? JSONDecoder().decode(JSONValue.self, from: line) else { return }
        if let id = message["id"].int {
            lock.lock()
            let continuation = pending.removeValue(forKey: id)
            lock.unlock()
            guard let continuation else { return }
            let error = message["error"]
            if error.isNull {
                continuation.resume(returning: message["result"])
            } else {
                continuation.resume(throwing: RPCError(
                    code: Int(error["code"].int ?? Int64(RPCError.internalError)),
                    message: error["message"].string ?? "the service refused",
                    exitCode: error["data"]["exit_code"].int.map(Int.init)
                ))
            }
        } else if let method = message["method"].string {
            lock.lock()
            let handler = self.handler
            lock.unlock()
            handler?(RPCNotification(method: method, params: message["params"]))
        }
    }
}
