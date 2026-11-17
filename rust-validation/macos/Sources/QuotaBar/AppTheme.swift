import CQuotaBar
import UsageCore

enum AppTheme {
    @MainActor static func apply(_ mode: AppearanceMode) {
        switch mode {
        case .system: qb_mac_appearance(0)
        case .light: qb_mac_appearance(1)
        case .dark: qb_mac_appearance(2)
        }
    }
}
