// Compare the material of a SwiftUI mini switch with the system AppKit control.
import AppKit
import SwiftUI
import QuartzCore
import ObjectiveC

let destination = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
app.finishLaunching()

func rgb(_ r: Double, _ g: Double, _ b: Double) -> NSColor {
    NSColor(srgbRed: r, green: g, blue: b, alpha: 1)
}

func channels(_ color: NSColor) -> [Double] {
    guard let c = color.usingColorSpace(.sRGB) else { return [] }
    return [c.redComponent, c.greenComponent, c.blueComponent, c.alphaComponent]
}

@MainActor func appearanceDetails(_ view: NSView) -> [String: Any] {
    var colors = [String: [Double]]()
    view.effectiveAppearance.performAsCurrentDrawingAppearance {
        colors["accent"] = channels(.controlAccentColor)
        for property in ["contentTintColor", "bezelColor", "_accentColor", "_tintColor"] {
            if view.responds(to: NSSelectorFromString(property)),
               let color = view.value(forKey: property) as? NSColor {
                colors[property] = channels(color)
            }
        }
    }
    var custom = [String]()
    var cls: AnyClass? = type(of: view)
    while let current = cls, current != NSView.self {
        var count: UInt32 = 0
        if let methods = class_copyMethodList(current, &count) {
            for i in 0..<Int(count) {
                let name = NSStringFromSelector(method_getName(methods[i]))
                if ["accent", "tint", "bezelcolor"].contains(where: name.lowercased().contains) {
                    custom.append(name)
                }
            }
            free(methods)
        }
        cls = class_getSuperclass(current)
    }
    func layerInfo(_ layer: CALayer) -> [String: Any] {
        ["class": NSStringFromClass(type(of: layer)), "filters": layer.filters?.map { String(describing: $0) } ?? [],
         "contents": String(describing: layer.contents.map { type(of: $0) }), "frame": NSStringFromRect(layer.frame),
         "children": layer.sublayers?.map(layerInfo) ?? []]
    }
    return ["class": NSStringFromClass(type(of: view)), "frame": NSStringFromRect(view.frame),
            "colors": colors, "appearance": String(describing: view.effectiveAppearance),
            "colorMethods": custom.sorted(), "layer": view.layer.map(layerInfo) ?? [:],
            "subviews": view.subviews.map(appearanceDetails)]
}

let providers = [("codex", rgb(0.310, 0.361, 0.784), rgb(0.537, 0.584, 1.000)),
                 ("claude", rgb(0.659, 0.310, 0.149), rgb(0.902, 0.510, 0.290)),
                 ("gemini", rgb(0.102, 0.451, 0.910), rgb(0.541, 0.706, 0.973))]
for (theme, appearanceName) in [("light", NSAppearance.Name.aqua), ("dark", NSAppearance.Name.darkAqua)] {
    let appearance = NSAppearance(named: appearanceName)!
    app.appearance = appearance
    var samples: [(String, NSColor?)] = [("system", nil)]
    for (key, light, dark) in providers {
        let dynamic = NSColor(name: NSColor.Name(key)) { appearance in
            appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
        }
        samples.append((key + "-dynamic", dynamic))
        samples.append((key + "-fixed", theme == "dark" ? dark : light))
    }
    for r in 0..<4 {
        for g in 0..<4 {
            for b in 0..<4 {
                samples.append(("rgb-\(r)\(g)\(b)", rgb(Double(r)/3, Double(g)/3, Double(b)/3)))
            }
        }
    }
    let cell = NSSize(width: 50, height: 28)
    let canvas = NSView(frame: NSRect(x: 0, y: 0, width: cell.width*8, height: cell.height*9))
    let window = NSWindow(contentRect: canvas.bounds, styleMask: [.titled], backing: .buffered, defer: false)
    window.contentView = canvas
    var entries: [(String, NSView)] = []
    @MainActor func install(_ name: String, _ view: NSView) {
        let n = entries.count
        let alignment = NSRect(x: CGFloat(n % 8)*cell.width, y: CGFloat(n / 8)*cell.height, width: cell.width, height: cell.height)
        view.frame = alignment
        canvas.addSubview(view)
        entries.append((name, view))
    }
    for (name, color) in samples {
        let toggle = Toggle("test", isOn: .constant(true)).labelsHidden().toggleStyle(.switch)
            .controlSize(.mini).font(.caption).tint(color.map { Color(nsColor: $0) })
            .frame(width: cell.width, height: cell.height)
        install(name, NSHostingView(rootView: toggle))
    }
    let plain = NSView(frame: NSRect(origin: .zero, size: cell))
    let control = NSSwitch()
    control.controlSize = .mini
    control.state = .on
    control.sizeToFit()
    var alignment = control.alignmentRect(forFrame: control.frame)
    alignment.origin = NSPoint(x: (cell.width-alignment.width)/2, y: (cell.height-alignment.height)/2)
    control.frame = control.frame(forAlignmentRect: alignment)
    plain.addSubview(control)
    install("appkit", plain)
    window.makeKeyAndOrderFront(nil)
    app.activate(ignoringOtherApps: true)
    RunLoop.main.run(until: Date().addingTimeInterval(0.5))
    window.displayIfNeeded()
    var info = [String: Any]()
    for (name, view) in entries {
        let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds)!
        view.cacheDisplay(in: view.bounds, to: rep)
        try rep.representation(using: .png, properties: [:])!.write(to: destination.appendingPathComponent("\(theme)-\(name).png"))
        if ["system", "appkit", "codex-dynamic", "claude-fixed", "rgb-030"].contains(name) {
            info[name] = appearanceDetails(view)
        }
    }
    try JSONSerialization.data(withJSONObject: info, options: [.sortedKeys, .prettyPrinted]).write(to: destination.appendingPathComponent(theme + ".json"))
    window.orderOut(nil)
}
