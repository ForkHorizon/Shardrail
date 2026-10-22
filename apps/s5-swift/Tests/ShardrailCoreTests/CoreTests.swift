import XCTest
@testable import ShardrailCore
import Darwin

final class CoreTests: XCTestCase {
    func testFramingHandlesSplitAndMultipleLinesAndRejectsOversize() throws {
        var reader = FrameDecoder(limit: 8)
        XCTAssertEqual(try reader.append(Data("{\"a\":".utf8)), [])
        XCTAssertEqual(try reader.append(Data("1}\n{}\n".utf8)).map { String(decoding: $0, as: UTF8.self) }, ["{\"a\":1}", "{}"])
        var limited = FrameDecoder(limit: 3)
        XCTAssertThrowsError(try limited.append(Data("1234".utf8)))
    }

    func testSharedHandshakeVector() throws {
        let vector = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("test-vectors/client-handshake-v1.json"))) as! [String: String]
        let message = try Handshake.transcript(fieldsHex: [vector["client_nonce"]!, vector["server_nonce"]!, vector["client_public_key"]!, vector["host_public_key"]!])
        XCTAssertEqual(message.hex, vector["transcript_sha256"])
        try Handshake.verify(signatureHex: vector["client_signature"]!, message: message, publicKeyHex: vector["client_public_key"]!)
        XCTAssertThrowsError(try Handshake.verify(signatureHex: vector["client_signature"]!, message: Data("tampered".utf8), publicKeyHex: vector["client_public_key"]!))
        XCTAssertThrowsError(try Handshake.transcript(fieldsHex: ["not-hex", "00", "00", "00"]))
    }

    func testSocketDisablesSigPipe() throws {
        var sockets: [Int32] = [0, 0]
        XCTAssertEqual(socketpair(AF_UNIX, SOCK_STREAM, 0, &sockets), 0)
        defer { Darwin.close(sockets[0]); Darwin.close(sockets[1]) }
        try DaemonConnection.configureNoSigPipe(fd: sockets[0])
        var enabled: Int32 = 0
        var size = socklen_t(MemoryLayout<Int32>.size)
        XCTAssertEqual(getsockopt(sockets[0], SOL_SOCKET, SO_NOSIGPIPE, &enabled, &size), 0)
        XCTAssertEqual(enabled, 1)
    }

    func testFailedConnectionClosesSocketDescriptor() {
        let openDescriptors = { (0..<Int(getdtablesize())).filter { fcntl(Int32($0), F_GETFD) != -1 } }
        let before = openDescriptors()
        XCTAssertThrowsError(try DaemonConnection(
            socketPath: "/tmp/\(String(repeating: "x", count: 256))",
            clientID: "client",
            privateKeySeed: Data(repeating: 0, count: 32),
            hostPublicKey: String(repeating: "00", count: 32)
        ))
        XCTAssertEqual(openDescriptors(), before)
    }

    func testPercentileEmptyAndBoundaryFractions() {
        XCTAssertNil(percentile([], 0.5))
        XCTAssertEqual(percentile([1, 2, 3], 0), 1)
        XCTAssertEqual(percentile([1, 2, 3], 1), 3)
    }

    func testDuplicateArgumentsUseLastValue() {
        XCTAssertEqual(keyValueArguments(["events=5000", "ignored", "events=50"]), ["events": "50"])
    }

    func testDurableEventJSONParsing() throws {
        let data = Data("{\"cursor\":7,\"receipt_wall_clock_ms\":1234,\"event\":{\"session_id\":\"session\",\"kind\":\"agent_message_chunk\",\"payload\":{\"text\":\"hello\"}}}".utf8)
        let row = try JSONDecoder().decode(DurableRow.self, from: data)
        XCTAssertEqual(row.cursor, 7)
        XCTAssertEqual(row.event.session_id, "session")
        XCTAssertEqual(row.event.kind, "agent_message_chunk")
        XCTAssertEqual(row.receipt_wall_clock_ms, 1234)
    }

    func testDurableFrameDecodesWithoutReencodingEvent() throws {
        let frame = try DaemonConnection.decodeFrame(Data("{\"kind\":\"durable_event\",\"cursor\":7,\"receipt_wall_clock_ms\":1234,\"event\":{\"session_id\":\"session\",\"kind\":\"agent_message_chunk\",\"payload\":{\"text\":\"hello\"}}}".utf8))
        XCTAssertEqual(frame.cursor, 7)
        XCTAssertEqual(frame.event?.payload, .object(["text": .string("hello")]))
        XCTAssertThrowsError(try DaemonConnection.decodeFrame(Data("{bad json}".utf8))) { XCTAssertEqual($0 as? ProtocolError, .malformedFrame) }
    }

    func testStoreDeduplicatesWithoutDroppingHistory() {
        var store = EventStore()
        for n in 0..<10_000 { XCTAssertTrue(store.append(DurableRow(cursor: n, event: .init(session_id: "s", kind: "agent_message_chunk")))) }
        XCTAssertFalse(store.append(store.rows[0])); XCTAssertEqual(store.rows.count, 10_000)
    }

    func testClockMatchesWebOffsetSpreadUncertainty() {
        let mapping = ClockMapping(samples: [(1000, 999, 1001), (2004, 1998, 2002), (3002, 2999, 3001)])
        XCTAssertEqual(mapping.offsetMs, 2); XCTAssertEqual(mapping.uncertaintyMs, 3)
        XCTAssertEqual(mapping.monotonicMs(forWallMs: 5000), 4998)
    }

    func testB4NeedsTwoFramesDeduplicatesAndHonorsGates() {
        let event = DurableRow.Event(session_id: "s", kind: "agent_message_chunk")
        let row = DurableRow(cursor: 1, event: event, receipt_wall_clock_ms: 1000)
        var sampler = B4Sampler(expectedChunks: 1); sampler.setVisible(true); sampler.received(row)
        sampler.committedVisible(row, commitMonoMs: 10, receiptMonoMs: 10)
        sampler.frameEntered(monotonicMs: 9); sampler.frameEntered(monotonicMs: 10); XCTAssertTrue(sampler.samples.isEmpty)
        sampler.frameEntered(monotonicMs: 20); XCTAssertTrue(sampler.samples.isEmpty)
        sampler.frameEntered(monotonicMs: 26); XCTAssertEqual(sampler.samples, [16]); XCTAssertTrue(sampler.gatesPass)
        sampler.committedVisible(row, commitMonoMs: 27, receiptMonoMs: 10); sampler.frameEntered(monotonicMs: 30)
        XCTAssertEqual(sampler.samples.count, 1)
        var hidden = B4Sampler(expectedChunks: 1); hidden.received(row); hidden.committedVisible(row, commitMonoMs: 0, receiptMonoMs: 0); hidden.frameEntered(monotonicMs: 1); hidden.frameEntered(monotonicMs: 2); XCTAssertFalse(hidden.gatesPass)

        var floored = B4Sampler(expectedChunks: 5_000); floored.setVisible(true)
        for _ in 0..<5_000 { floored.received(row) }
        for cursor in 1...49 {
            let visibleRow = DurableRow(cursor: cursor, event: event)
            floored.committedVisible(visibleRow, commitMonoMs: Double(cursor * 3), receiptMonoMs: 0)
            floored.frameEntered(monotonicMs: Double(cursor * 3 + 1))
            floored.frameEntered(monotonicMs: Double(cursor * 3 + 2))
        }
        XCTAssertEqual(floored.sampleFloor, 50); XCTAssertFalse(floored.gatesPass)
        let fiftieth = DurableRow(cursor: 50, event: event)
        floored.committedVisible(fiftieth, commitMonoMs: 150, receiptMonoMs: 0); floored.frameEntered(monotonicMs: 151); floored.frameEntered(monotonicMs: 152)
        XCTAssertTrue(floored.gatesPass)
    }
}
