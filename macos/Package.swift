// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "QuotaBar",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "QuotaBar", targets: ["QuotaBar"])],
    targets: [
        .systemLibrary(name: "CQuotaBar"),
        .target(name: "UsageCore", dependencies: ["CQuotaBar"], linkerSettings: [.linkedLibrary("iconv"), .linkedFramework("ServiceManagement"), .linkedFramework("AppKit"), .linkedFramework("QuartzCore"), .linkedFramework("CoreGraphics")]),
        .executableTarget(name: "QuotaBar", dependencies: ["UsageCore", "CQuotaBar"],
                          exclude: ["Views.swift", "ActivityView.swift", "AppLogo.swift", "AppSettings.swift", "OpenSourceLicensesSettings.swift"],
                          resources: [.process("Resources")]),
        .testTarget(name: "UsageCoreTests", dependencies: ["UsageCore"])
    ]
)
