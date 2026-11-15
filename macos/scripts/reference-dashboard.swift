// Compiled only in a temporary checkout of the original SwiftUI application.
// This extension shares App.swift so it can drive the original window and sheet.
import CoreGraphics
import ImageIO

extension AppDelegate {
    @MainActor func capturePopoverReference(stateURL: URL, output: URL, nativeState: URL) throws {
        let state = try JSONDecoder().decode(DashboardState.self, from: Data(contentsOf: stateURL))
        store = MonitorStore(demo: true, remote: DashboardClient())
        store.apply(state)
        AppTheme.apply(store.settings.selectedAppearance)
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        updateStatusItem()
        togglePopover()
        defer { popover?.performClose(nil); NSStatusBar.system.removeStatusItem(statusItem) }
        guard let view = popover?.contentViewController?.view else { throw CocoaError(.fileReadUnknown) }
        RunLoop.main.run(until: Date().addingTimeInterval(1))
        try saveReferenceView(view, to: output)
        let accounts: [[String: Any]] = try store.accounts.map { account in
            let snapshot = store.snapshots[account.id]
            let windows = try JSONSerialization.jsonObject(with: JSONEncoder().encode(snapshot?.windows ?? []))
            var entry: [String: Any] = ["provider": account.provider.title, "name": account.name, "windows": windows,
                    "visible": account.isShownInMenuBar, "enabled": account.enabled,
                    "error": store.errors[account.id] != nil, "loading": store.loading.contains(account.id)]
            if let state = snapshot?.state { entry["state"] = state }
            return entry
        }
        let wire: [String: Any] = ["appearance": store.settings.selectedAppearance.rawValue, "mode": store.settings.selectedMenuBarDisplayMode.rawValue,
                                   "demo": store.demo, "accounts": accounts]
        try JSONSerialization.data(withJSONObject: wire).write(to: nativeState)
    }
    @MainActor func captureReference(stateURL: URL, route: String, output: URL) throws {
        let state = try JSONDecoder().decode(DashboardState.self, from: Data(contentsOf: stateURL))
        store = MonitorStore(demo: true, remote: DashboardClient())
        store.apply(state)
        AppTheme.apply(store.settings.selectedAppearance)
        dashboardStateReady = true
        let connect = route.hasPrefix("connect")
        showDashboard(route: connect ? "accounts" : route)
        guard let window = dashboard else { throw CocoaError(.fileReadUnknown) }
        window.setContentSize(NSSize(width: 1120, height: 810))
        if connect { store.beginConnection() }
        let deadline = Date().addingTimeInterval(2)
        repeat {
            RunLoop.main.run(until: Date().addingTimeInterval(0.05))
            window.displayIfNeeded()
        } while Date() < deadline
        let target = connect ? window.attachedSheet : window
        guard let view = target?.contentView else { throw CocoaError(.fileReadUnknown) }
        try saveReferenceView(view, to: output)
    }
}

@MainActor private func saveReferenceView(_ view: NSView, to output: URL) throws {
    view.layoutSubtreeIfNeeded()
    view.displayIfNeeded()
    guard let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds) else { throw CocoaError(.fileWriteUnknown) }
    view.cacheDisplay(in: view.bounds, to: bitmap)
    guard let png = bitmap.representation(using: .png, properties: [:]) else { throw CocoaError(.fileWriteUnknown) }
    try png.write(to: output)
}

private struct Raster {
    let width: Int
    let height: Int
    let pixels: [UInt8]

    var hasVisibleDetails: Bool {
        var first: (Int, Int, Int)?
        for i in stride(from: 0, to: pixels.count, by: 4) where pixels[i + 3] >= 128 {
            let rgb = (Int(pixels[i]), Int(pixels[i + 1]), Int(pixels[i + 2]))
            if let first {
                if abs(first.0 - rgb.0) + abs(first.1 - rgb.1) + abs(first.2 - rgb.2) > 48 { return true }
            } else { first = rgb }
        }
        return false
    }

    init(_ url: URL) throws {
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
              let bitmap = CGImageSourceCreateImageAtIndex(source, 0, nil) else { throw CocoaError(.fileReadCorruptFile) }
        let w = bitmap.width, h = bitmap.height
        var data = [UInt8](repeating: 0, count: w * h * 4)
        try data.withUnsafeMutableBytes { bytes in
            guard let context = CGContext(data: bytes.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                          space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { throw CocoaError(.fileReadCorruptFile) }
            context.draw(bitmap, in: CGRect(x: 0, y: 0, width: CGFloat(w), height: CGFloat(h)))
        }
        width = w
        height = h
        pixels = data
    }
}

func compareReference(original: URL, candidate: URL, output: URL) throws {
    let lhs = try Raster(original), rhs = try Raster(candidate)
    let w = min(lhs.width, rhs.width), h = min(lhs.height, rhs.height)
    let outside = lhs.width * lhs.height + rhs.width * rhs.height - 2 * w * h
    var changed = outside, error = outside * 4 * 255
    for y in 0..<h {
        for x in 0..<w {
            var distance = 0
            for channel in 0..<4 {
                distance += abs(Int(lhs.pixels[(y * lhs.width + x) * 4 + channel]) - Int(rhs.pixels[(y * rhs.width + x) * 4 + channel]))
            }
            if distance != 0 { changed += 1 }
            error += distance
        }
    }
    let originalVisible = lhs.hasVisibleDetails, candidateVisible = rhs.hasVisibleDetails
    let result: [String: Any] = ["originalWidth": lhs.width, "originalHeight": lhs.height, "width": rhs.width, "height": rhs.height,
                                 "originalHasContent": originalVisible, "candidateHasContent": candidateVisible, "changedPixels": changed,
                                 "meanChannelError": Double(error) / Double(4 * (w * h + outside)),
                                 "identical": originalVisible && candidateVisible && changed == 0]
    try JSONSerialization.data(withJSONObject: result).write(to: output)
}
