import Foundation

public struct ClockMapping: Equatable {
    public let offsetMs: Double
    public let uncertaintyMs: Double
    public init(samples: [(wallMs: Double, before: Double, after: Double)]) {
        let offsets = samples.map { $0.wallMs - ($0.before + $0.after) / 2 }
        offsetMs = offsets.isEmpty ? 0 : offsets.reduce(0, +) / Double(offsets.count)
        uncertaintyMs = offsets.isEmpty ? .infinity : (offsets.max()! - offsets.min()!) / 2 + 1
    }
    public func monotonicMs(forWallMs wall: Double) -> Double { wall - offsetMs }
}

public struct B4Sampler {
    public let expectedChunks: Int
    public private(set) var receivedChunks = 0
    public private(set) var visible = false
    public private(set) var samples: [Double] = []
    private var scheduled: [Int: (frame: Int, receiptMs: Double)] = [:]
    private var seen = Set<Int>()
    public init(expectedChunks: Int) { self.expectedChunks = expectedChunks }
    public var sampleFloor: Int { min(expectedChunks, max(20, Int(ceil(Double(expectedChunks) * 0.01)))) }
    public mutating func setVisible(_ value: Bool) { visible = value }
    public mutating func received(_ row: DurableRow) {
        if row.event.kind == "agent_message_chunk" { receivedChunks += 1 }
    }
    public mutating func committedVisible(_ row: DurableRow, frame: Int, receiptMonoMs: Double) {
        guard visible, row.event.kind == "agent_message_chunk", !seen.contains(row.cursor), scheduled[row.cursor] == nil else { return }
        scheduled[row.cursor] = (frame + 2, receiptMonoMs)
    }
    public mutating func frameEntered(_ frame: Int, monotonicMs: Double) {
        let ready = scheduled.filter { frame >= $0.value.frame }.map(\.key)
        for id in ready {
            if visible, let receipt = scheduled[id]?.receiptMs { samples.append(monotonicMs - receipt); seen.insert(id) }
            scheduled.removeValue(forKey: id)
        }
    }
    public var gatesPass: Bool { visible && receivedChunks >= expectedChunks && samples.count >= sampleFloor }
}

public struct EventStore {
    public private(set) var rows: [DurableRow] = []
    private var cursors = Set<Int>()
    public init() {}
    @discardableResult public mutating func append(_ row: DurableRow) -> Bool {
        guard cursors.insert(row.cursor).inserted else { return false }
        rows.append(row); return true
    }
}

public func percentile(_ values: [Double], _ fraction: Double) -> Double? {
    guard !values.isEmpty else { return nil }
    let sorted = values.sorted(); return sorted[min(sorted.count - 1, max(0, Int(Double(sorted.count) * fraction)))]
}
