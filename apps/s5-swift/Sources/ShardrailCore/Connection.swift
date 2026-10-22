import Foundation
import CryptoKit
import Darwin

public final class DaemonConnection {
    private var fd: Int32 = -1
    private var decoder = FrameDecoder()
    private var frames: [Data] = []
    public init(socketPath: String, clientID: String, privateKeySeed: Data, hostPublicKey: String) throws {
        guard privateKeySeed.count == 32, let hostData = Data(hex: hostPublicKey), hostData.count == 32 else { throw ProtocolError.invalidHex }
        fd = socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw POSIXError(.EIO) }
        var initialized = false
        defer { if !initialized { close() } }
        try Self.configureNoSigPipe(fd: fd)
        var address = sockaddr_un(); address.sun_family = sa_family_t(AF_UNIX)
        let pathBytes = Array(socketPath.utf8) + [0]
        guard pathBytes.count <= MemoryLayout.size(ofValue: address.sun_path) else { throw POSIXError(.ENAMETOOLONG) }
        withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: pathBytes) }
        let connected = withUnsafePointer(to: &address) { ptr in ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) } }
        guard connected == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
        let privateKey = try Curve25519.Signing.PrivateKey(rawRepresentation: privateKeySeed)
        let clientNonce = Data((0..<32).map { _ in UInt8.random(in: .min ... .max) })
        try writeJSON(["min_version": 1, "max_version": 1, "client_id": clientID, "public_key": privateKey.publicKey.rawRepresentation.hex, "nonce": clientNonce.hex, "resume_after": 0, "journal_epoch": NSNull()])
        let hello = try JSONSerialization.jsonObject(with: readLine()) as? [String: Any] ?? [:]
        guard hello["protocol_version"] as? Int == 1, let publicKey = hello["public_key"] as? String, publicKey == hostPublicKey, let nonce = hello["nonce"] as? String, let signature = hello["signature"] as? String else { throw ProtocolError.hostKeyMismatch }
        let digest = try Handshake.transcript(fieldsHex: [clientNonce.hex, nonce, privateKey.publicKey.rawRepresentation.hex, publicKey])
        try Handshake.verify(signatureHex: signature, message: digest, publicKeyHex: hostPublicKey)
        try writeJSON(["kind": "authenticate", "signature": privateKey.signature(for: digest).hex])
        let auth = try JSONSerialization.jsonObject(with: readLine()) as? [String: Any]
        guard auth?["kind"] as? String == "authenticated" else { throw ProtocolError.authRejected }
        initialized = true
    }
    deinit { close() }
    public func send(_ value: [String: Any]) throws { try writeJSON(value) }
    public func readRows(_ handler: (DurableRow) -> Void) throws {
        while true {
            let frame = try Self.decodeFrame(readLine())
            guard frame.kind == "durable_event", let cursor = frame.cursor, let event = frame.event else { continue }
            handler(DurableRow(cursor: cursor, event: event, receipt_wall_clock_ms: frame.receipt_wall_clock_ms))
        }
    }
    static func decodeFrame(_ data: Data) throws -> DurableFrame {
        do { return try JSONDecoder().decode(DurableFrame.self, from: data) } catch { throw ProtocolError.malformedFrame }
    }
    static func configureNoSigPipe(fd: Int32) throws {
        var enabled: Int32 = 1
        guard setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &enabled, socklen_t(MemoryLayout<Int32>.size)) == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    }
    public func close() { if fd >= 0 { Darwin.close(fd); fd = -1 } }
    private func writeJSON(_ value: [String: Any]) throws {
        let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) + Data([10])
        try data.withUnsafeBytes { raw in var offset = 0; while offset < raw.count { let n = Darwin.write(fd, raw.baseAddress!.advanced(by: offset), raw.count - offset); guard n > 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }; offset += n } }
    }
    private func readLine() throws -> Data {
        while frames.isEmpty {
            var bytes = [UInt8](repeating: 0, count: 16_384); let n = Darwin.read(fd, &bytes, bytes.count)
            guard n > 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .ECONNRESET) }
            frames.append(contentsOf: try decoder.append(Data(bytes[..<n])))
        }
        return frames.removeFirst()
    }
}
