import AppKit
import SwiftUI

/// The same vector artwork is also used by scripts/make-icon.swift for the Dock icon.
private enum AppLogoAssets {
    static let menuBarHeight: CGFloat = 14
    static let mark: NSImage = {
        guard let url = AppResources.bundle.url(forResource: "AppMark", withExtension: "svg"),
              let image = NSImage(contentsOf: url) else {
            preconditionFailure("The app bundle is missing AppMark.svg")
        }
        image.isTemplate = true
        // Keep the native image and SwiftUI layout at the same compact menu-bar size.
        image.size = NSSize(width: menuBarHeight * 266 / 282, height: menuBarHeight)
        return image
    }()
}

struct AppMark: View {
    var height: CGFloat = AppLogoAssets.menuBarHeight

    var body: some View {
        Image(nsImage: AppLogoAssets.mark).resizable().renderingMode(.template)
            .interpolation(.high).scaledToFit()
            .frame(width: height * 266 / 282, height: height)
            .accessibilityLabel("QuotaBar 로고")
    }
}

/// Status item and Settings preview share this image and its fixed icon-to-title gap.
struct MenuBarTitle: View {
    let title: String

    var body: some View {
        Image(nsImage: MenuBarTitleImage.image(for: title))
            .renderingMode(.template)
            .accessibilityLabel(title)
    }
}

@MainActor
enum MenuBarTitleImage {
    static let font = NSFont.monospacedDigitSystemFont(ofSize: NSFont.menuBarFont(ofSize: 0).pointSize, weight: .regular)
    static var icon: NSImage { AppLogoAssets.mark }
    private static let spacing: CGFloat = 6
    private static var cachedTitle: String?
    private static var cachedImage: NSImage?

    static func image(for title: String) -> NSImage {
        if cachedTitle == title, let cachedImage { return cachedImage }

        let mark = AppLogoAssets.mark
        let markSize = mark.size
        let text = NSAttributedString(string: title, attributes: [.font: font, .foregroundColor: NSColor.black])
        let textSize = text.size()
        let textX = markSize.width + spacing
        let size = NSSize(width: ceil(textX + textSize.width), height: ceil(max(markSize.height, textSize.height)))
        // Drawing at the destination resolution keeps the shared image sharp on Retina displays.
        let image = NSImage(size: size, flipped: false) { _ in
            mark.draw(in: NSRect(x: 0, y: (size.height - markSize.height) / 2,
                                width: markSize.width, height: markSize.height),
                      from: .zero, operation: .sourceOver, fraction: 1)
            text.draw(at: NSPoint(x: textX, y: (size.height - textSize.height) / 2))
            return true
        }
        image.isTemplate = true
        // Retain only the current value; routine store updates reuse the same image.
        cachedTitle = title
        cachedImage = image
        return image
    }
}

struct AppLogo: View {
    var size: CGFloat = 38

    var body: some View {
        AppMark(height: size * 282 / 456)
            .foregroundStyle(AppTheme.onAccent)
            .frame(width: size, height: size)
            .background(AppTheme.accent, in: RoundedRectangle(cornerRadius: size * 108 / 456, style: .circular))
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("QuotaBar 로고")
    }
}
