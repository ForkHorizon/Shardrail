import XCTest
@testable import ShardrailCore

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
    }

    func testDurableEventJSONParsing() throws {
        let data = Data("{\"cursor\":7,\"receipt_wall_clock_ms\":1234,\"event\":{\"session_id\":\"session\",\"kind\":\"agent_message_chunk\",\"payload\":{\"text\":\"hello\"}}}".utf8)
        let row = try JSONDecoder().decode(DurableRow.self, from: data)
        XCTAssertEqual(row.cursor, 7)
        XCTAssertEqual(row.event.session_id, "session")
        XCTAssertEqual(row.event.kind, "agent_message_chunk")
        XCTAssertEqual(row.receipt_wall_clock_ms, 1234)
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
        sampler.committedVisible(row, frame: 4, receiptMonoMs: 10)
        sampler.frameEntered(5, monotonicMs: 20); XCTAssertTrue(sampler.samples.isEmpty)
        sampler.frameEntered(6, monotonicMs: 26); XCTAssertEqual(sampler.samples, [16]); XCTAssertTrue(sampler.gatesPass)
        sampler.committedVisible(row, frame: 6, receiptMonoMs: 10); sampler.frameEntered(8, monotonicMs: 30)
        XCTAssertEqual(sampler.samples.count, 1)
        var hidden = B4Sampler(expectedChunks: 1); hidden.received(row); hidden.committedVisible(row, frame: 1, receiptMonoMs: 0); hidden.frameEntered(3, monotonicMs: 3); XCTAssertFalse(hidden.gatesPass)

        var floored = B4Sampler(expectedChunks: 5_000); floored.setVisible(true)
        for _ in 0..<5_000 { floored.received(row) }
        for cursor in 1...49 {
            let visibleRow = DurableRow(cursor: cursor, event: event)
            floored.committedVisible(visibleRow, frame: cursor * 3, receiptMonoMs: 0)
            floored.frameEntered(cursor * 3 + 2, monotonicMs: Double(cursor * 3 + 2))
        }
        XCTAssertEqual(floored.sampleFloor, 50); XCTAssertFalse(floored.gatesPass)
        let fiftieth = DurableRow(cursor: 50, event: event)
        floored.committedVisible(fiftieth, frame: 150, receiptMonoMs: 0); floored.frameEntered(152, monotonicMs: 152)
        XCTAssertTrue(floored.gatesPass)
    }
}
