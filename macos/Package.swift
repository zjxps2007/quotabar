// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "QuotaBar",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "QuotaBar", targets: ["QuotaBar"])],
    targets: [
        .systemLibrary(name: "CQuotaBar"),
        .target(name: "UsageCore", dependencies: ["CQuotaBar"], linkerSettings: [.linkedLibrary("iconv")]),
        .executableTarget(name: "QuotaBar", dependencies: ["UsageCore"], resources: [.process("Resources")]),
        .testTarget(name: "UsageCoreTests", dependencies: ["UsageCore"])
    ]
)
