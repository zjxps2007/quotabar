import AppKit

let directory = URL(fileURLWithPath: CommandLine.arguments[1])
let markURL = URL(fileURLWithPath: CommandLine.arguments[2])
guard let mark = NSImage(contentsOf: markURL) else {
    fatalError("Cannot read the shared app logo at \(markURL.path)")
}
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
for size in [16, 32, 128, 256, 512] {
    for scale in [1, 2] {
        let pixels = size * scale
        let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: pixels, pixelsHigh: pixels,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
        let side = CGFloat(pixels)
        let transform = AffineTransform(scale: side / 512)
        (transform as NSAffineTransform).concat()
        let shape = NSBezierPath(roundedRect: NSRect(x: 28, y: 28, width: 456, height: 456), xRadius: 108, yRadius: 108)
        NSColor.black.setFill(); shape.fill()
        mark.draw(in: NSRect(x: 123, y: 115, width: 266, height: 282),
                  from: .zero, operation: .sourceOver, fraction: 1)
        NSGraphicsContext.restoreGraphicsState()
        let data = bitmap.representation(using: .png, properties: [:])!
        let suffix = scale == 1 ? "" : "@2x"
        try data.write(to: directory.appendingPathComponent("icon_\(size)x\(size)\(suffix).png"))
    }
}
