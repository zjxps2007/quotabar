import Foundation

enum AppResources {
    /// SwiftPM's command-line accessor falls back to the build directory.
    /// A packaged app (including its helper) must resolve its own resources first.
    static let bundle: Bundle = {
        if let url = Bundle.main.url(forResource: "QuotaBar_QuotaBar", withExtension: "bundle"),
           let bundle = Bundle(url: url) { return bundle }
        return Bundle.module
    }()
}
