// Exercises the same native status item, AppKit controls and callbacks as the packaged monitor.
import AppKit
import CQuotaBar

private final class Actions {
    var received = [Int32]()
}
private final class MonitorEvents {
    var received = [QBMacStatusEvent]()
}

private struct ProbeError: Error, CustomStringConvertible {
    let description: String
    init(_ reason: String, line: Int = #line) { description = "check-popover.swift:\(line): \(reason)" }
}

@MainActor private func run() throws {
    let args = CommandLine.arguments
    let state = try Data(contentsOf: URL(fileURLWithPath: args[1]))
    let output = URL(fileURLWithPath: args[2])
    let app = NSApplication.shared
    app.finishLaunching()
    guard qb_mac_dock_visible(1) != 0, app.activationPolicy() == .regular,
          qb_mac_dock_visible(0) != 0, app.activationPolicy() == .accessory else {
        throw ProbeError("Dock visibility did not change the activation policy")
    }
    guard let fixture = try JSONSerialization.jsonObject(with: state) as? [String: Any],
          let allAccounts = fixture["accounts"] as? [[String: Any]] else { throw ProbeError("Invalid accounts fixture") }
    let accounts = allAccounts.filter { $0["enabled"] as? Bool == true && $0["visible"] as? Bool == true }
    let dark = fixture["appearance"] as? String == "dark"
    qb_mac_appearance(dark ? 2 : 1)
    guard app.appearance?.name == (dark ? NSAppearance.Name.darkAqua : .aqua) else {
        throw ProbeError("Application appearance did not change")
    }
    let actions = Actions()
    guard let status = qb_mac_status_new(Unmanaged.passUnretained(actions).toOpaque(), { context, tag in
        guard let context else { return }
        Unmanaged<Actions>.fromOpaque(context).takeUnretainedValue().received.append(tag)
    }) else { throw ProbeError("Could not create status item") }
    defer { qb_mac_status_free(status) }
    func publish(_ data: Data) throws {
        guard data.withUnsafeBytes({ qb_mac_status_publish(status, $0.bindMemory(to: UInt8.self).baseAddress, $0.count) }) != 0
        else { throw ProbeError("Status item rejected the fixture") }
    }
    try publish(state)
    guard qb_mac_status_control(status, 0) != 0 else { throw ProbeError("Status item did not open") }
    RunLoop.main.run(until: Date().addingTimeInterval(1))
    func descendants(_ view: NSView) -> [NSView] { [view] + view.subviews.flatMap(descendants) }
    func currentPanel() throws -> NSView {
        let views = app.windows.filter(\.isVisible).compactMap(\.contentView).flatMap(descendants)
        guard let refresh = views.compactMap({ $0 as? NSButton }).first(where: { $0.toolTip == "새로고침" }),
              let panel = refresh.superview else { throw ProbeError("Visible popover has no refresh button") }
        return panel
    }
    let panel = try currentPanel()
    if let first = accounts.first {
        guard let period = (first["windows"] as? [[String: Any]])?.first,
              let title = period["title"] as? String, let remaining = period["remaining"] as? Double else {
            throw ProbeError("Fixture contains no quota window")
        }
        let views = descendants(panel)
        let spoken = "\(title) 잔여량 \(Int(remaining)) 퍼센트"
        guard views.compactMap({ $0 as? NSTextField }).contains(where: { $0.stringValue == "\(Int(remaining.rounded()))% 남음" }),
              let track = views.first(where: { $0.accessibilityLabel() == spoken }),
              track.accessibilityRole() == .progressIndicator,
              (track.accessibilityValue() as? NSNumber)?.doubleValue == remaining else {
            throw ProbeError("Missing visible percentage or accessible progress for \(spoken)")
        }
    }
    panel.layoutSubtreeIfNeeded()
    panel.displayIfNeeded()
    guard let bitmap = panel.bitmapImageRepForCachingDisplay(in: panel.bounds) else { throw CocoaError(.fileWriteUnknown) }
    panel.cacheDisplay(in: panel.bounds, to: bitmap)
    guard let png = bitmap.representation(using: .png, properties: [:]) else { throw CocoaError(.fileWriteUnknown) }
    try png.write(to: output)
    captureVisiblePopover(panel)
    if accounts.count > 1 {
        func updateAccounts(_ updated: [[String: Any]]) throws -> NSScrollView {
            var value = fixture
            value["accounts"] = updated
            let data = try JSONSerialization.data(withJSONObject: value)
            try publish(data)
            let view = try currentPanel()
            view.layoutSubtreeIfNeeded()
            guard let scroll = view.subviews.compactMap({ $0 as? NSScrollView }).first else { throw ProbeError("No scroll view after update") }
            return scroll
        }
        func distanceFromTop(_ scroll: NSScrollView) throws -> CGFloat {
            guard let document = scroll.documentView else { throw ProbeError("Scroll view has no document") }
            return document.bounds.maxY - scroll.documentVisibleRect.maxY
        }
        guard let scroll = panel.subviews.compactMap({ $0 as? NSScrollView }).first,
              scroll.hasVerticalScroller, abs(try distanceFromTop(scroll)) <= 1 else { throw ProbeError("Overflow list did not start at the top") }
        guard let document = scroll.documentView, abs(document.bounds.width - scroll.contentSize.width) <= 1,
              let bar = descendants(document).first(where: { $0.accessibilityRole() == .progressIndicator }),
              abs(bar.frame.maxX - scroll.contentSize.width) <= 1 else { throw ProbeError("Scrollbar covers the quota values") }
        let clip = scroll.contentView
        clip.scroll(to: NSPoint(x: 0, y: clip.bounds.minY - 90))
        scroll.reflectScrolledClipView(clip)
        guard abs(try distanceFromTop(scroll) - 90) <= 1 else { throw ProbeError("Overflow list did not scroll by 90 points") }
        let longer = try updateAccounts(accounts + [accounts[0]])
        guard abs(try distanceFromTop(longer) - 90) <= 1 else { throw ProbeError("Adding an account lost the scroll position") }
        let shorter = try updateAccounts([accounts[0]])
        guard !shorter.hasVerticalScroller, abs(try distanceFromTop(shorter)) <= 1 else { throw ProbeError("Shrinking the list left a scrollbar or offset") }
    }
    let buttons = try currentPanel().subviews.compactMap { $0 as? NSButton }
    guard Set(buttons.map(\.tag)) == [1, 2, 3, 4] else { throw ProbeError("Unexpected popover button tags \(buttons.map(\.tag))") }
    for button in buttons.sorted(by: { $0.tag < $1.tag }) { button.performClick(nil) }
    guard actions.received == [1, 2, 3, 4] else { throw ProbeError("Unexpected button callbacks \(actions.received)") }
    _ = qb_mac_status_control(status, 1)
    RunLoop.main.run(until: Date().addingTimeInterval(0.5))
    guard qb_mac_status_control(status, 3) == 0, actions.received.last == 5 else { throw ProbeError("Popover closure was not reported") }

    let service = MonitorEvents()
    guard let monitor = qb_mac_monitor_status_new(Unmanaged.passUnretained(service).toOpaque(), { context, event in
        guard let context else { return }
        Unmanaged<MonitorEvents>.fromOpaque(context).takeUnretainedValue().received.append(event)
    }) else { throw ProbeError("Could not create monitor status item") }
    defer { qb_mac_status_free(monitor) }
    guard state.withUnsafeBytes({ qb_mac_status_publish(monitor, $0.bindMemory(to: UInt8.self).baseAddress, $0.count) }) != 0
    else { throw ProbeError("Monitor rejected the fixture") }
    qb_mac_status_show_popover(monitor)
    RunLoop.main.run(until: Date().addingTimeInterval(0.5))
    let menu = try currentPanel().subviews.compactMap { $0 as? NSButton }
    menu.first { $0.tag == 1 }?.performClick(nil)
    menu.first { $0.tag == 3 }?.performClick(nil)
    RunLoop.main.run(until: Date().addingTimeInterval(0.5))
    guard service.received.first == QBMacStatusOpened, service.received.contains(QBMacStatusRefresh),
          service.received.contains(QBMacStatusSettings), service.received.contains(QBMacStatusClosed),
          qb_mac_status_control(monitor, 3) == 0 else { throw ProbeError("Unexpected monitor status events \(service.received)") }
}

do { try MainActor.assumeIsolated { try run() } }
catch { FileHandle.standardError.write(Data("Native popover probe failed: \(error)\n".utf8)); exit(1) }
