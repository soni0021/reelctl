import Foundation
import XCTest
@testable import ReelCore

#if canImport(Darwin)
import Darwin
#endif

/// A one-connection JSON-RPC server on a Unix socket, answering by a script:
/// method → reply (a result, or an error), and optionally pushing a
/// notification before the reply.
final class FakeServer: @unchecked Sendable {
    let path: String
    private let fd: Int32
    private let replies: [String: JSONValue]
    private let push: [String: JSONValue]
    private(set) var received: [JSONValue] = []
    private let lock = NSLock()

    init(replies: [String: JSONValue], push: [String: JSONValue] = [:]) throws {
        self.replies = replies
        self.push = push
        path = NSTemporaryDirectory() + "rc-\(UUID().uuidString.prefix(8)).sock"
        unlink(path)
        fd = socket(AF_UNIX, Int32(SOCK_STREAM), 0)
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let bytes = Array(path.utf8)
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            buffer.copyBytes(from: bytes)
            buffer[bytes.count] = 0
        }
        address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        XCTAssertEqual(bound, 0)
        listen(fd, 1)
        let thread = Thread { [self] in serve() }
        thread.start()
    }

    var requests: [JSONValue] {
        lock.lock()
        defer { lock.unlock() }
        return received
    }

    private func serve() {
        let client = accept(fd, nil, nil)
        guard client >= 0 else { return }
        var buffer = Data()
        var chunk = [UInt8](repeating: 0, count: 4096)
        while true {
            let count = chunk.withUnsafeMutableBytes { read(client, $0.baseAddress, $0.count) }
            if count <= 0 { break }
            buffer.append(contentsOf: chunk[0..<count])
            while let newline = buffer.firstIndex(of: 0x0A) {
                let line = buffer[buffer.startIndex..<newline]
                buffer.removeSubrange(buffer.startIndex...newline)
                guard let request = try? JSONDecoder().decode(JSONValue.self, from: Data(line)) else { continue }
                lock.lock()
                received.append(request)
                lock.unlock()
                let method = request["method"].string ?? ""
                if let note = push[method] {
                    let notification: JSONValue = ["jsonrpc": "2.0", "method": note["method"], "params": note["params"]]
                    write(client, notification.text + "\n")
                }
                let reply = replies[method] ?? ["error": ["code": -32601, "message": "no method"]]
                var message: [String: JSONValue] = ["jsonrpc": "2.0", "id": request["id"]]
                if reply["error"].isNull {
                    message["result"] = reply["result"]
                } else {
                    message["error"] = reply["error"]
                }
                write(client, JSONValue.object(message).text + "\n")
            }
        }
        close(client)
    }

    private func write(_ client: Int32, _ text: String) {
        let bytes = Array(text.utf8)
        _ = bytes.withUnsafeBytes { Darwin.write(client, $0.baseAddress, $0.count) }
    }

    deinit {
        close(fd)
        unlink(path)
    }
}

final class ReelCoreTests: XCTestCase {
    func testACallGetsItsResultAndANotificationArrivesWhileItWaits() async throws {
        let server = try FakeServer(
            replies: [
                "workspace.info": ["result": ["path": "/w", "version": "0.1.0", "schema_version": 1, "limits": ["heavy": 1, "light": 4, "network": 1], "pid": 42]],
                "events.subscribe": ["result": ["subscribed": true]],
            ],
            push: ["events.subscribe": ["method": "job.status", "params": ["job": 7, "status": "running"]]]
        )
        let client = try RPCClient(socketPath: server.path)
        let studio = Studio(client: client)
        let info = try await studio.info()
        XCTAssertEqual(info.schemaVersion, 1)
        XCTAssertEqual(info.limits, Limits(heavy: 1, light: 4, network: 1))

        let seen = expectation(description: "notification")
        try await studio.subscribe { event in
            if event == .jobStatus(job: 7, status: .running) { seen.fulfill() }
        }
        await fulfillment(of: [seen], timeout: 5)
        XCTAssertEqual(server.requests.first?["jsonrpc"], "2.0")
    }

    func testAnErrorCarriesItsCodeAndExitCode() async throws {
        let server = try FakeServer(replies: [
            "understand.run": ["error": ["code": -32000, "message": "over the bound", "data": ["exit_code": 4]]],
        ])
        let client = try RPCClient(socketPath: server.path)
        do {
            _ = try await client.call("understand.run", ["plan": 1])
            XCTFail("expected an error")
        } catch let error as RPCError {
            XCTAssertEqual(error.code, RPCError.refused)
            XCTAssertEqual(error.exitCode, 4)
            XCTAssertEqual(ErrorMeaning(error), .overBudget)
        }
    }

    func testASubmitSendsTypedArgumentsAndInputRefs() async throws {
        let server = try FakeServer(replies: ["jobs.submit": ["result": ["job": 3, "status": "blocked", "cached": false]]])
        let studio = Studio(client: try RPCClient(socketPath: server.path))
        let result = try await studio.submit(
            subject: 1,
            command: "timeline.cut-silence",
            args: ["min_gap_ms": 180],
            inputs: ["project": .fromJob(2, role: "out"), "dna": .asset(5)]
        )
        XCTAssertEqual(result.status, .blocked)
        let params = server.requests.last?["params"] ?? .null
        XCTAssertEqual(params["args"]["min_gap_ms"], 180)
        XCTAssertEqual(params["inputs"]["project"], ["from": 2, "role": "out"])
        XCTAssertEqual(params["inputs"]["dna"], ["asset": 5])
    }

    func testAJobDecodesWithItsClassAndCheckpoint() throws {
        let json = """
        {"id":4,"subject":1,"command":"timeline.init","args":{"min_gap_ms":180},"inputs":{"input":{"asset":1}},
         "class":"light","priority":0,"status":"succeeded","after":[],"cache_key":"k","exit_code":0,
         "result":{"frames":"360"},"error":null,"env_names":[],"created_at":1,"started_at":2,"finished_at":3,
         "force":false,"events":[],"checkpoint":{"id":9,"subject":1,"parent":null,"job":4,"label":"timeline.init",
         "outputs":{"out":"ab"},"cached":true,"note":"","created_at":3}}
        """
        let job = try JSONDecoder().decode(Job.self, from: Data(json.utf8))
        XCTAssertEqual(job.jobClass, .light)
        XCTAssertEqual(job.shortCommand, "init")
        // Keys inside untyped values are kept as they are.
        XCTAssertEqual(job.args["min_gap_ms"], 180)
        XCTAssertEqual(job.checkpoint?.cached, true)
        XCTAssertEqual(job.checkpoint?.outputs["out"], "ab")
    }

    func testACubeReadsInFileOrderAndRefusesAShortTable() throws {
        var text = "TITLE \"t\"\nLUT_3D_SIZE 2\n"
        for b in 0..<2 { for g in 0..<2 { for r in 0..<2 { text += "\(r) \(g) \(b)\n" } } }
        let cube = try CubeLUT(text: text)
        XCTAssertEqual(cube.title, "t")
        XCTAssertEqual(cube.size, 2)
        XCTAssertEqual(Array(cube.rgba[4..<8]), [1, 0, 0, 1])
        XCTAssertEqual(cube.data.count, 8 * 4 * MemoryLayout<Float>.size)
        XCTAssertThrowsError(try CubeLUT(text: "LUT_3D_SIZE 2\n0 0 0\n"))
    }

    func testAPlanShowsTheAmountToApproveAndAShellWordIsQuoted() throws {
        let plan = try Wire.decode(UnderstandPlan.self, from: ["plan": 3, "cost_upper_bound_usd": 0.0124, "frames": 12, "images": 12, "expires_at": 0])
        XCTAssertEqual(plan.approvalText, "$0.0124")
        XCTAssertEqual(ShellQuote.quote("/tmp/a b.mp4"), "'/tmp/a b.mp4'")
        XCTAssertEqual(ShellQuote.quote("--min-gap-ms"), "--min-gap-ms")
        XCTAssertEqual(ShellQuote.quote("it's"), "'it'\\''s'")
    }

    func testAMakeReelsRequestSendsOnlyWhatIsSet() {
        var request = MakeReelsRequest(input: .asset(1), outliers: [.asset(2)], targetURL: "https://example.com")
        request.dna = .fromJob(5, role: "out")
        request.clips = [1, 3]
        let json = request.json
        XCTAssertEqual(json["dna"], ["from": 5, "role": "out"])
        XCTAssertEqual(json["clips"], [1, 3])
        XCTAssertTrue(json["understanding"].isNull)
        XCTAssertEqual(json["follow"], "presenter")
    }

    func testAStoredVideoIsRecognisedAndNamedByItsBrand() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("playable-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        func stored(_ name: String, _ bytes: [UInt8]) throws -> URL {
            let url = dir.appendingPathComponent(name)
            try Data(bytes).write(to: url)
            return url
        }
        let mp4 = try stored("mp4", [0, 0, 0, 24] + Array("ftypisom".utf8) + [0, 0, 0, 0])
        let mov = try stored("mov", [0, 0, 0, 20] + Array("ftypqt  ".utf8) + [0, 0, 0, 0])
        let json = try stored("dna.json", Array("{\"clips\": []}".utf8))
        let empty = try stored("empty", [])
        XCTAssertTrue(Playable.isVideo(mp4))
        XCTAssertTrue(Playable.isVideo(mov))
        XCTAssertFalse(Playable.isVideo(json))
        XCTAssertFalse(Playable.isVideo(empty))
        XCTAssertEqual(Playable.fileExtension(for: mp4), "mp4")
        XCTAssertEqual(Playable.fileExtension(for: mov), "mov")
        XCTAssertEqual(Playable.fileExtension(for: json), "json")
    }

    // MARK: Reel outputs (the Reels screen's grouping)

    private func subject(_ id: Int, _ name: String, kind: String = "reel") throws -> Subject {
        try Wire.decode(Subject.self, from: ["id": .int(Int64(id)), "kind": .string(kind), "name": .string(name), "created_at": 0])
    }

    private func checkpoint(_ id: Int, _ subject: Int, _ label: String, _ outputs: [String: String]) throws -> Checkpoint {
        var out: [String: JSONValue] = [:]
        for (key, value) in outputs { out[key] = .string(value) }
        return try Wire.decode(Checkpoint.self, from: [
            "id": .int(Int64(id)), "subject": .int(Int64(subject)), "label": .string(label),
            "outputs": .object(out), "cached": false, "note": "", "created_at": 0,
        ])
    }

    private func fromClip(_ id: Int, _ subject: Int, asset: Int) throws -> Job {
        try Wire.decode(Job.self, from: [
            "id": .int(Int64(id)), "subject": .int(Int64(subject)), "command": "timeline.from-clip",
            "args": ["clip": 1], "inputs": ["input": ["asset": .int(Int64(asset))]],
            "class": "light", "priority": 0, "status": "succeeded", "after": [], "cache_key": "k",
            "env_names": [], "created_at": 0, "force": false,
        ])
    }

    func testReelsAreGroupedBySourceVideoInClipOrder() throws {
        let subjects = [try subject(10, "clip 3"), try subject(8, "clip 1"), try subject(9, "clip 2"), try subject(20, "other 1")]
        var checkpoints: [SubjectID: [Checkpoint]] = [:]
        var jobs: [SubjectID: [Job]] = [:]
        for (i, id) in [8, 9, 10].enumerated() {
            checkpoints[Int64(id)] = [try checkpoint(30 + i, id, "render", ["out": "h\(id)"]), try checkpoint(40 + i, id, "diff", ["report": "r\(id)"])]
            jobs[Int64(id)] = [try fromClip(100 + i, id, asset: 1)]
        }
        checkpoints[20] = [try checkpoint(50, 20, "render", ["out": "h20"])]
        jobs[20] = [try fromClip(120, 20, asset: 3)]
        let groups = ReelOutputs.group(subjects: subjects, checkpoints: checkpoints, jobs: jobs)
        XCTAssertEqual(groups.map(\.source), [3, 1], "the newest render's source comes first")
        let neet = try XCTUnwrap(groups.last)
        XCTAssertEqual(neet.reels.map(\.clip), [1, 2, 3])
        XCTAssertEqual(neet.reels.map(\.renderHash), ["h8", "h9", "h10"])
        XCTAssertEqual(neet.reels.first?.diffReportHash, "r8")
        XCTAssertEqual(groups.first?.reels.first?.diffReportHash, nil)
    }

    func testASubjectWithoutARenderOrASourceIsHandledNotDropped() throws {
        let subjects = [try subject(1, "clip 1"), try subject(2, "clip 2"), try subject(3, "an edit", kind: "project")]
        let checkpoints: [SubjectID: [Checkpoint]] = [
            1: [try checkpoint(5, 1, "timeline.from-clip", ["out": "p"])],
            2: [try checkpoint(6, 2, "render", ["out": "old"]), try checkpoint(9, 2, "render", ["out": "new"])],
            3: [try checkpoint(7, 3, "render", ["out": "x"])],
        ]
        let groups = ReelOutputs.group(subjects: subjects, checkpoints: checkpoints, jobs: [:])
        XCTAssertEqual(groups.count, 1, "no render, or not a reel subject: not a reel")
        XCTAssertNil(groups[0].source, "no from-clip job: unknown source")
        XCTAssertEqual(groups[0].reels.map(\.renderHash), ["new"], "the newest render replaces an older one")
        XCTAssertEqual(groups[0].reels[0].renderCheckpoint, 9)
    }

    func testTheUnknownSourceComesLastEvenWhenItsRenderIsNewer() throws {
        let subjects = [try subject(1, "clip 1"), try subject(2, "clip 1")]
        let checkpoints: [SubjectID: [Checkpoint]] = [
            1: [try checkpoint(5, 1, "render", ["out": "known"])],
            2: [try checkpoint(99, 2, "render", ["out": "unknown"])],
        ]
        let jobs: [SubjectID: [Job]] = [1: [try fromClip(7, 1, asset: 4)]]
        let groups = ReelOutputs.group(subjects: subjects, checkpoints: checkpoints, jobs: jobs)
        XCTAssertEqual(groups.map(\.source), [4, nil])
    }

    func testAClipNumberComesFromTheSubjectName() throws {
        XCTAssertEqual(ReelOutputs.clipNumber("clip 12"), 12)
        XCTAssertEqual(ReelOutputs.clipNumber("Reel clip 3 final"), 3)
        XCTAssertNil(ReelOutputs.clipNumber("my reel"))
    }
}
