// Both UI probes load the same process-scoped WindowServer capture driver.
import AppKit
import Darwin

@MainActor func captureVisiblePopover(_ view: NSView) {
    guard let path = ProcessInfo.processInfo.environment["QUOTABAR_TEST_POPOVER_FRAME"],
          let process = dlopen(nil, RTLD_LAZY) else { return }
    defer { dlclose(process) }
    guard let address = dlsym(process, "QBTestCaptureView") else { return }
    typealias Capture = @convention(c) (UnsafeMutableRawPointer, UnsafePointer<CChar>) -> Int32
    let capture = unsafeBitCast(address, to: Capture.self)
    _ = path.withCString { capture(Unmanaged.passUnretained(view).toOpaque(), $0) }
    let output = URL(fileURLWithPath: path).deletingPathExtension().appendingPathExtension("controls.json")
    var buttons: [[String: Any]] = []
    var queue = [view]
    while let next = queue.popLast() {
        queue.append(contentsOf: next.subviews)
        guard let button = next as? NSButton, !button.isHiddenOrHasHiddenAncestor else { continue }
        let frame = view.convert(button.bounds, from: button)
        buttons.append(["title": button.title, "class": NSStringFromClass(type(of: button)),
                        "x": frame.minX, "top": view.isFlipped ? frame.minY : view.bounds.maxY - frame.maxY,
                        "width": frame.width, "height": frame.height, "bordered": button.isBordered,
                        "bezelStyle": button.bezelStyle.rawValue, "controlSize": button.controlSize.rawValue,
                        "font": button.font?.fontName ?? "", "points": button.font?.pointSize ?? 0])
    }
    if let data = try? JSONSerialization.data(withJSONObject: buttons) { try? data.write(to: output) }
}
