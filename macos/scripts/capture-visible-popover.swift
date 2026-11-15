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
}
