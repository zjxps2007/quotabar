import AppKit
import SwiftUI
import UsageCore

enum AppTheme {
    static func colorScheme(for mode: AppearanceMode) -> ColorScheme? {
        switch mode {
        case .system: return nil
        case .light: return .light
        case .dark: return .dark
        }
    }

    @MainActor static func apply(_ mode: AppearanceMode) {
        switch mode {
        case .system: NSApplication.shared.appearance = nil
        case .light: NSApplication.shared.appearance = NSAppearance(named: .aqua)
        case .dark: NSApplication.shared.appearance = NSAppearance(named: .darkAqua)
        }
    }

    static let accent = monochrome("AppAccent", light: 0, dark: 1)
    static let onAccent = monochrome("AppOnAccent", light: 1, dark: 0)
    static let background = monochrome("AppBackground", light: 0.96, dark: 0.055)
    static let sidebarBackground = monochrome("AppSidebar", light: 1, dark: 0.075)
    static let codexAccent = Color(nsColor: NSColor(name: "CodexAccent") { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? NSColor(srgbRed: 0.537, green: 0.584, blue: 1.000, alpha: 1)
            : NSColor(srgbRed: 0.310, green: 0.361, blue: 0.784, alpha: 1)
    })
    static let claudeAccent = Color(nsColor: NSColor(name: "ClaudeAccent") { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? NSColor(srgbRed: 0.902, green: 0.510, blue: 0.290, alpha: 1)
            : NSColor(srgbRed: 0.659, green: 0.310, blue: 0.149, alpha: 1)
    })
    static let geminiAccent = Color(nsColor: NSColor(name: "GeminiAccent") { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? NSColor(srgbRed: 0.541, green: 0.706, blue: 0.973, alpha: 1)
            : NSColor(srgbRed: 0.102, green: 0.451, blue: 0.910, alpha: 1)
    })
    static let cardBackground = monochrome("AppCard", light: 1, dark: 0.11)
    static let cardBorder = Color.primary.opacity(0.07)
    static let trackBackground = Color.primary.opacity(0.06)

    private static func monochrome(_ name: String, light: CGFloat, dark: CGFloat) -> Color {
        Color(nsColor: NSColor(name: NSColor.Name(name)) { appearance in
            let white = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: white, green: white, blue: white, alpha: 1)
        })
    }
}

struct AppPrimaryButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(isEnabled ? AppTheme.onAccent : .secondary)
            .padding(.horizontal, 12).padding(.vertical, 6)
            .background(isEnabled ? AppTheme.accent : AppTheme.trackBackground,
                        in: RoundedRectangle(cornerRadius: 8))
            .opacity(configuration.isPressed ? 0.7 : 1)
    }
}
