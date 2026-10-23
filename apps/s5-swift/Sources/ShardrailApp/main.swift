import AppKit
import CoreVideo
import ShardrailCore
import Foundation

final class AppDelegate: NSObject, NSApplicationDelegate, NSTableViewDataSource, NSTableViewDelegate, @unchecked Sendable {
    private var window: NSWindow!
    private var table: NSTableView!
    private var status: NSTextField!
    private var store = EventStore()
    private var pending: [DurableRow] = []
    private var sampler: B4Sampler?
    private var mapping = ClockMapping(samples: [])
    private var link: CVDisplayLink?
    private var framePending = false
    private var pendingFrameMs = 0.0
    private var resultWritten = false
    private var connection: DaemonConnection?
    private var args: [String: String] = [:]
    private let lock = NSLock()
    func applicationDidFinishLaunching(_ notification: Notification) {
        args = keyValueArguments(Array(CommandLine.arguments.dropFirst()))
        let b4 = args["b4"] == "1", count = Int(args["events"] ?? "0") ?? 0
        sampler = b4 ? B4Sampler(expectedChunks: count) : nil; sampler?.setVisible(true)
        let samples = (0..<5).map { _ -> (wallMs: Double, before: Double, after: Double) in let before = ProcessInfo.processInfo.systemUptime * 1000, wall = Date().timeIntervalSince1970 * 1000, after = ProcessInfo.processInfo.systemUptime * 1000; return (wall, before, after) }
        mapping = ClockMapping(samples: samples)
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 700), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "Shardrail Swift AppKit Spike"; window.center(); window.isReleasedWhenClosed = false
        NotificationCenter.default.addObserver(self, selector: #selector(occlusionChanged), name: NSWindow.didChangeOcclusionStateNotification, object: window)
        status = NSTextField(labelWithString: "Connecting to local daemon…")
        table = NSTableView(); table.rowHeight = 42; table.usesAutomaticRowHeights = false; table.headerView = nil
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("event")); column.title = "Event"; table.addTableColumn(column); table.dataSource = self; table.delegate = self
        let scroll = NSScrollView(); scroll.documentView = table; scroll.hasVerticalScroller = true
        let root = NSStackView(views: [status, scroll]); root.orientation = .vertical; root.spacing = 8; root.edgeInsets = NSEdgeInsets(top: 12, left: 12, bottom: 12, right: 12); root.setHuggingPriority(.defaultHigh, for: .vertical)
        NSLayoutConstraint.activate([scroll.widthAnchor.constraint(greaterThanOrEqualToConstant: 860), scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 600)])
        window.contentView = root; root.translatesAutoresizingMaskIntoConstraints = false
        if let content = window.contentView { NSLayoutConstraint.activate([root.leadingAnchor.constraint(equalTo: content.leadingAnchor), root.trailingAnchor.constraint(equalTo: content.trailingAnchor), root.topAnchor.constraint(equalTo: content.topAnchor), root.bottomAnchor.constraint(equalTo: content.bottomAnchor)]) }
        window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
        startDisplayLink(); DispatchQueue.global(qos: .userInitiated).async { self.connectAndRead() }
    }
    func numberOfRows(in tableView: NSTableView) -> Int { store.rows.count }
    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let cell = tableView.makeView(withIdentifier: NSUserInterfaceItemIdentifier("cell"), owner: self) as? NSTableCellView ?? { let v = NSTableCellView(); let label = NSTextField(labelWithString: ""); label.lineBreakMode = .byTruncatingTail; v.textField = label; v.addSubview(label); label.frame = NSRect(x: 4, y: 0, width: 850, height: 40); label.autoresizingMask = [.width]; v.identifier = NSUserInterfaceItemIdentifier("cell"); return v }()
        let item = store.rows[row]; cell.textField?.stringValue = "\(item.cursor)  \(item.event.session_id)  \(item.event.kind)"
        return cell
    }
    private func startDisplayLink() {
        var created: CVDisplayLink?; CVDisplayLinkCreateWithActiveCGDisplays(&created); guard let created else { return }; link = created
        CVDisplayLinkSetOutputCallback(created, { _, _, _, _, _, context in
            let app = Unmanaged<AppDelegate>.fromOpaque(context!).takeUnretainedValue()
            let callbackMs = ProcessInfo.processInfo.systemUptime * 1000
            app.queueFrame(monotonicMs: callbackMs); return kCVReturnSuccess
        }, Unmanaged.passUnretained(self).toOpaque()); CVDisplayLinkStart(created)
    }
    @objc @MainActor private func occlusionChanged() { sampler?.setVisible(window.isVisible && !window.isMiniaturized && window.occlusionState.contains(.visible)) }
    private func queueFrame(monotonicMs: Double) {
        lock.lock(); pendingFrameMs = monotonicMs
        guard !framePending else { lock.unlock(); return }
        framePending = true; lock.unlock()
        DispatchQueue.main.async {
            self.lock.lock(); let latestFrameMs = self.pendingFrameMs; self.framePending = false; self.lock.unlock()
            self.onFrame(monotonicMs: latestFrameMs)
        }
    }
    @MainActor private func onFrame(monotonicMs: Double) {
        sampler?.setVisible(window.isVisible && !window.isMiniaturized && window.occlusionState.contains(.visible))
        lock.lock(); let batch = sampler == nil ? pending : Array(pending.prefix(96)); pending.removeFirst(batch.count); lock.unlock()
        if !batch.isEmpty {
            let visibleRows = table.rows(in: table.visibleRect); let wasAtTail = store.rows.isEmpty || visibleRows.location == NSNotFound || NSMaxRange(visibleRows) >= store.rows.count
            for row in batch { if store.append(row) { sampler?.received(row) } }
            table.reloadData()
            if wasAtTail && store.rows.count > 0 { table.scrollRowToVisible(store.rows.count - 1) }
        }
        if sampler?.visible == true {
            let visible = table.rows(in: table.visibleRect)
            for index in visible.location..<NSMaxRange(visible) where index >= 0 && index < store.rows.count {
                let row = store.rows[index]
                if let receipt = row.receipt_wall_clock_ms { sampler?.committedVisible(row, commitMonoMs: ProcessInfo.processInfo.systemUptime * 1000, receiptMonoMs: mapping.monotonicMs(forWallMs: Double(receipt))) }
            }
        }
        sampler?.frameEntered(monotonicMs: monotonicMs)
        if sampler?.gatesPass == true { saveResult() }
    }
    private func connectAndRead() {
        do {
            let dataDir = ProcessInfo.processInfo.environment["S5_SHARDRAIL_DATA_DIR"] ?? NSHomeDirectory() + "/Library/Application Support/Shardrail"
            let daemon = ProcessInfo.processInfo.environment["SHARDRAIL_DAEMON"] ?? "shardraild"
            let registration = Process(); registration.executableURL = URL(fileURLWithPath: daemon); registration.arguments = ["--data-dir", dataDir, "client", "add", "--label", "s5-swift-bench", "--scope", "full"]
            let out = Pipe(); registration.standardOutput = out; try registration.run(); registration.waitUntilExit(); let text = String(data: out.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
            let values = Dictionary(uniqueKeysWithValues: text.split(whereSeparator: \.isWhitespace).compactMap { part -> (String, String)? in let bits = part.split(separator: "=", maxSplits: 1); return bits.count == 2 ? (String(bits[0]), String(bits[1])) : nil })
            guard let id = values["client_id"], let host = values["host_public_key"] else { throw ProtocolError.authRejected }
            let key = try Data(contentsOf: URL(fileURLWithPath: dataDir).appendingPathComponent("client-\(id).key"))
            let client = try DaemonConnection(socketPath: dataDir + "/shardraild.sock", clientID: id, privateKeySeed: key, hostPublicKey: host); lock.lock(); connection = client; lock.unlock()
            let streams = Int(args["streams"] ?? "1") ?? 1
            for i in 0..<streams { try client.send(["kind":"request", "request":["command_id":UUID().uuidString.lowercased(), "kind":"session.create", "expected_state":NSNull(), "expected_turn_id":NSNull(), "payload":["s5_stream":i]]]) }
            DispatchQueue.main.async { self.status.stringValue = "Connected · streaming \(streams) session(s)" }
            try client.readRows { row in self.lock.lock(); self.pending.append(row); self.lock.unlock() }
        } catch { DispatchQueue.main.async { self.status.stringValue = "Connection failed: \(error)"; self.saveResult(error: String(describing: error)) } }
    }
    @MainActor private func saveResult(error: String? = nil) {
        guard let path = ProcessInfo.processInfo.environment["S5_SWIFT_RESULT_PATH"] else { return }
        lock.lock(); guard !resultWritten else { lock.unlock(); return }; resultWritten = true; lock.unlock()
        var result: [String: Any] = ["date": ISO8601DateFormatter().string(from: Date()), "stack":"swift-appkit", "temperature":args["temperature"] ?? "unspecified", "streams":Int(args["streams"] ?? "1") ?? 1, "events":Int(args["events"] ?? "0") ?? 0, "scenario":"\(args["events"] ?? "0") events × \(args["streams"] ?? "1") streams", "coldLaunchMs":NSNull(), "navigationMs":NSNull(), "requiredGeneratedTokens":Int(args["events"] ?? "0") ?? 0, "pageVisibility":"visible", "pageFocused":window.isKeyWindow, "inputMethod":"not used for B4 daemon stream measurement", "daemonAgentChunkCount":sampler?.receivedChunks ?? 0, "b4":["count":sampler?.samples.count ?? 0, "medianMs":percentile(sampler?.samples ?? [], 0.5) as Any? ?? NSNull(), "p95Ms":percentile(sampler?.samples ?? [], 0.95) as Any? ?? NSNull(), "maxMs":(sampler?.samples.max() as Any?) ?? NSNull(), "samplesMs":sampler?.samples ?? [], "clockOffsetMs":mapping.offsetMs, "clockUncertaintyMs":mapping.uncertaintyMs], "clockOffsetMs":mapping.offsetMs, "clockUncertaintyMs":mapping.uncertaintyMs]
        for key in ["firstTranscriptRenderOpportunityMs", "longTasks", "maxLongTaskMs", "frameP95", "frameMax", "heapMB", "mockCreationToMountedRowFollowupP95", "composerP95", "composerMin", "composerMax"] { result[key] = NSNull() }
        result["firstTranscriptRenderOpportunityMs"] = NSNull(); result["frameSamples"] = 0; result["commitCount"] = 0; result["commitMs"] = 0; result["eventStoreCount"] = store.rows.count; result["generatedTokens"] = sampler?.receivedChunks ?? 0; result["composerSamples"] = 0; result["matchesByFirstFrame"] = 0; result["firstFrameSamples"] = 0; result["firstFrameDeltasMs"] = [Double](); result["firstFrameRafTimestampDeltasMs"] = [Double](); result["userAgent"] = "AppKit"; result["scrollSmoothness"] = ["frameP95Ms":NSNull(), "frameMaxMs":NSNull(), "frames":0]
        if let error { result["error"] = error }
        do { let data = try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]); try data.write(to: URL(fileURLWithPath: path), options: .atomic); lock.lock(); let activeConnection = connection; lock.unlock(); activeConnection?.close(); NSApp.terminate(nil) } catch { NSLog("result write failed: %@", String(describing: error)) }
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
    func applicationWillTerminate(_ notification: Notification) { if let link { CVDisplayLinkStop(link) }; lock.lock(); let activeConnection = connection; lock.unlock(); activeConnection?.close() }
}
let app = NSApplication.shared; app.setActivationPolicy(.regular); let delegate = AppDelegate(); app.delegate = delegate; app.run()
