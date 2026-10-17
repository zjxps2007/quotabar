// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "QuotaBar",
    platforms: [.macOS(.v14)],
    products: [.executable(name: "QuotaBar", targets: ["QuotaBar"])],
    targets: [
        .target(name: "UsageCore"),
        .executableTarget(name: "QuotaBar", dependencies: ["UsageCore"], resources: [.process("Resources")]),
        .testTarget(name: "UsageCoreTests", dependencies: ["UsageCore"])
    ]
)
