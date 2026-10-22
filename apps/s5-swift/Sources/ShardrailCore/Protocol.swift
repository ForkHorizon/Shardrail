import Foundation
import CryptoKit

public enum ProtocolError: Error, Equatable { case frameTooLarge, malformedFrame, invalidHex, invalidSignature, hostKeyMismatch, authRejected }

public struct FrameDecoder {
    private var buffer = Data()
    public let limit: Int
    public init(limit: Int = 1_048_576) { self.limit = limit }
    public mutating func append(_ data: Data) throws -> [Data] {
        buffer.append(data)
        guard buffer.count <= limit || buffer.prefix(while: { $0 != 10 }).count <= limit else { throw ProtocolError.frameTooLarge }
        var frames: [Data] = []
        while let newline = buffer.firstIndex(of: 10) {
            guard buffer.distance(from: buffer.startIndex, to: newline) <= limit else { throw ProtocolError.frameTooLarge }
            frames.append(buffer[..<newline])
            buffer.removeSubrange(...newline)
        }
        guard buffer.count <= limit else { throw ProtocolError.frameTooLarge }
        return frames
    }
}

public struct DurableRow: Codable, Equatable, Identifiable {
    public let cursor: Int
    public let event: Event
    public let receipt_wall_clock_ms: Int64?
    public var id: Int { cursor }
    public struct Event: Codable, Equatable {
        public let session_id: String
        public let kind: String
        public let payload: JSONValue?
        public init(session_id: String, kind: String, payload: JSONValue? = nil) { self.session_id = session_id; self.kind = kind; self.payload = payload }
    }
    public init(cursor: Int, event: Event, receipt_wall_clock_ms: Int64? = nil) { self.cursor = cursor; self.event = event; self.receipt_wall_clock_ms = receipt_wall_clock_ms }
}

struct DurableFrame: Decodable {
    let kind: String?
    let cursor: Int?
    let event: DurableRow.Event?
    let receipt_wall_clock_ms: Int64?
}

public enum JSONValue: Codable, Equatable {
    case string(String), number(Double), bool(Bool), object([String: JSONValue]), array([JSONValue]), null
    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null } else if let v = try? c.decode(Bool.self) { self = .bool(v) } else if let v = try? c.decode(Double.self) { self = .number(v) } else if let v = try? c.decode(String.self) { self = .string(v) } else if let v = try? c.decode([String: JSONValue].self) { self = .object(v) } else { self = .array(try c.decode([JSONValue].self)) }
    }
    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self { case .string(let v): try c.encode(v); case .number(let v): try c.encode(v); case .bool(let v): try c.encode(v); case .object(let v): try c.encode(v); case .array(let v): try c.encode(v); case .null: try c.encodeNil() }
    }
}

public enum Handshake {
    public static func transcript(versions: [UInt32] = [1, 1, 1], fieldsHex: [String]) throws -> Data {
        guard fieldsHex.count == 4 else { throw ProtocolError.invalidHex }
        var bytes = Data("Shardrail mutual client handshake v1\0".utf8)
        for version in versions { var value = version.bigEndian; withUnsafeBytes(of: &value) { bytes.append(contentsOf: $0) } }
        for field in fieldsHex {
            guard let raw = Data(hex: field) else { throw ProtocolError.invalidHex }
            var length = UInt32(raw.count).bigEndian; withUnsafeBytes(of: &length) { bytes.append(contentsOf: $0) }; bytes.append(raw)
        }
        return Data(SHA256.hash(data: bytes))
    }
    public static func verify(signatureHex: String, message: Data, publicKeyHex: String) throws {
        guard let sig = Data(hex: signatureHex), let key = Data(hex: publicKeyHex), let publicKey = try? Curve25519.Signing.PublicKey(rawRepresentation: key), publicKey.isValidSignature(sig, for: message) else { throw ProtocolError.invalidSignature }
    }
}

extension Data {
    init?(hex: String) {
        guard hex.count.isMultiple(of: 2) else { return nil }
        var result = Data(); var i = hex.startIndex
        while i < hex.endIndex { let j = hex.index(i, offsetBy: 2); guard let b = UInt8(hex[i..<j], radix: 16) else { return nil }; result.append(b); i = j }
        self = result
    }
    var hex: String { map { String(format: "%02x", $0) }.joined() }
}
