import Foundation
import CryptoKit
import Testing
@testable import UsageCore

struct ReleaseUpdateTests {
    private var root: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("shared/fixtures")
    }
    private func fixture(_ name: String) throws -> Data { try Data(contentsOf: root.appendingPathComponent(name)) }
    private func changed(_ mutation: (inout [String: Any]) -> Void) throws -> Data {
        var object = try JSONSerialization.jsonObject(with: fixture("update-release.json")) as! [String: Any]
        mutation(&object)
        return try JSONSerialization.data(withJSONObject: object)
    }

    @Test func numericVersionsNeverOfferPreviewsOrDowngrades() throws {
        #expect(ReleaseVersion("0.1.10")! > ReleaseVersion("v0.1.9")!)
        #expect(ReleaseVersion("1.0.0")! > ReleaseVersion("0.999.999")!)
        for value in ["", "0.1", "1.2.3.4", "01.2.3", "1.2.3-beta", "1.2.3+build", "v1.2.3/evil", "1.2.3?x", "１.2.3", "2147483648.0.0", " 1.2.3"] {
            #expect(ReleaseVersion(value) == nil)
        }
        let release = try #require(try ReleaseFeed.parse(fixture("update-release.json"), platform: .macOSArm64))
        #expect(release.isNewer(than: "0.1.9"))
        #expect(!release.isNewer(than: "0.1.10"))
        #expect(!release.isNewer(than: "0.2.0"))
        #expect(!release.isNewer(than: "dev"))
    }

    @Test func sharedFeedSelectsExactPlatformAndIgnoresUnpublishedVersions() throws {
        let data = try fixture("update-release.json")
        #expect(try ReleaseFeed.parse(data, platform: .macOSArm64)?.asset?.name == "QuotaBar-0.1.10-macOS-arm64.zip")
        #expect(try ReleaseFeed.parse(data, platform: .macOSX64)?.asset == nil)
        #expect(try ReleaseFeed.parse(data, platform: .windowsInstaller)?.asset?.name.hasSuffix("Setup.exe") == true)
        for key in ["draft", "prerelease"] {
            #expect(try ReleaseFeed.parse(changed { $0[key] = true }, platform: .macOSArm64) == nil)
        }
    }

    @Test func untrustedURLsAndAmbiguousOrUnverifiedAssetsAreRejected() throws {
        let badPage = try changed { $0["html_url"] = "https://example.com/releases/tag/v0.1.10" }
        #expect(throws: (any Error).self) { try ReleaseFeed.parse(badPage, platform: .macOSArm64) }
        for key in ["browser_download_url", "digest", "state", "size", "duplicate"] {
            let data = try changed { object in
                var assets = object["assets"] as! [[String: Any]]
                switch key {
                case "browser_download_url": assets[0][key] = "https://github.com/other/project/releases/download/v0.1.10/a.zip"
                case "digest": assets[0][key] = "sha256:invalid"
                case "state": assets[0][key] = "starter"
                case "size": assets[0][key] = ReleaseFeed.maximumAssetBytes + 1
                default: assets.append(assets[0])
                }
                object["assets"] = assets
            }
            #expect(throws: (any Error).self) { try ReleaseFeed.parse(data, platform: .macOSArm64) }
        }
    }

    @Test func sharedP256SignatureBindsManifestVersionAndExactAsset() throws {
        let release = try #require(try ReleaseFeed.parse(fixture("update-release.json"), platform: .macOSArm64))
        let manifest = try fixture("update-manifest.json"), signature = try fixture("update-manifest.sig")
        let key = try String(contentsOf: root.appendingPathComponent("update-test-public.pem"), encoding: .utf8)
        let verified = try UpdateSignature.verify(manifest: manifest, signature: signature, publicKeyPEM: key, release: release, currentVersion: "0.1.0")
        #expect(verified.build == 7 && verified.asset == release.asset)
        var modified = manifest; modified[0] ^= 1
        #expect(throws: (any Error).self) { try UpdateSignature.verify(manifest: modified, signature: signature, publicKeyPEM: key, release: release, currentVersion: "0.1.0") }
        #expect(throws: (any Error).self) { try UpdateSignature.verify(manifest: manifest, signature: signature, publicKeyPEM: P256.Signing.PrivateKey().publicKey.pemRepresentation, release: release, currentVersion: "0.1.0") }
        #expect(throws: (any Error).self) { try UpdateSignature.verify(manifest: manifest, signature: signature, publicKeyPEM: key, release: release, currentVersion: "0.2.0") }
        let mismatched = try #require(try ReleaseFeed.parse(changed { object in
            var assets = object["assets"] as! [[String: Any]]; assets[0]["size"] = 2048; object["assets"] = assets
        }, platform: .macOSArm64))
        #expect(throws: (any Error).self) { try UpdateSignature.verify(manifest: manifest, signature: signature, publicKeyPEM: key, release: mismatched, currentVersion: "0.1.0") }
    }

    @Test func downloadedBytesMustMatchSignedSizeAndDigest() throws {
        let file = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: file) }
        let data = Data("synthetic update payload".utf8)
        let hash = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        let asset = ReleaseAsset(name: "fixture.zip", url: URL(string: "https://example.invalid/fixture.zip")!, bytes: Int64(data.count), sha256: hash)
        try data.write(to: file)
        try UpdateSignature.verifyFile(file, asset: asset)
        try Data(repeating: 0, count: data.count).write(to: file)
        #expect(throws: (any Error).self) { try UpdateSignature.verifyFile(file, asset: asset) }
        try Data(repeating: 0, count: data.count + 1).write(to: file)
        #expect(throws: (any Error).self) { try UpdateSignature.verifyFile(file, asset: asset) }
    }

    @Test func atomicReplacementRecoversFailureAndUnconfirmedStartup() throws {
        let fm = FileManager.default
        let root = fm.temporaryDirectory.resolvingSymlinksInPath().appendingPathComponent(UUID().uuidString)
        defer { try? fm.removeItem(at: root) }
        try fm.createDirectory(at: root, withIntermediateDirectories: true)
        let paths = try UpdateReplacement(target: root.appendingPathComponent("QuotaBar.app"), transaction: UUID())
        for directory in [paths.target, paths.incoming] { try fm.createDirectory(at: directory, withIntermediateDirectories: false) }
        try Data("old".utf8).write(to: paths.target.appendingPathComponent("version"))
        try Data("new".utf8).write(to: paths.incoming.appendingPathComponent("version"))
        #expect(throws: (any Error).self) { try paths.commit { throw CocoaError(.fileWriteUnknown) } }
        #expect(try String(contentsOf: paths.target.appendingPathComponent("version"), encoding: .utf8) == "old")
        #expect(!fm.fileExists(atPath: paths.backup.path))
        try paths.commit()
        #expect(try String(contentsOf: paths.target.appendingPathComponent("version"), encoding: .utf8) == "new")
        #expect(try String(contentsOf: paths.backup.appendingPathComponent("version"), encoding: .utf8) == "old")
        try paths.rollback()
        #expect(try String(contentsOf: paths.target.appendingPathComponent("version"), encoding: .utf8) == "old")
        #expect(!fm.fileExists(atPath: paths.backup.path))
    }

    @Test func updatePreferencesPreserveLegacyDefaultsAndDashboardCommands() throws {
        let encoder = JSONEncoder(), decoder = JSONDecoder()
        var settings = LiteSettings()
        #expect(settings.areAutomaticUpdatesEnabled)
        settings.automaticUpdates = false
        #expect(try !decoder.decode(LiteSettings.self, from: encoder.encode(settings)).areAutomaticUpdatesEnabled)
        let setting = try decoder.decode(DashboardCommand.self, from: encoder.encode(DashboardCommand.automaticUpdates(false)))
        let check = try decoder.decode(DashboardCommand.self, from: encoder.encode(DashboardCommand.checkAppUpdate))
        let install = try decoder.decode(DashboardCommand.self, from: encoder.encode(DashboardCommand.installAppUpdate))
        guard case .automaticUpdates(false) = setting, case .checkAppUpdate = check, case .installAppUpdate = install else {
            Issue.record("Update command changed in transit"); return
        }
        let state = DashboardState(settings: settings, snapshots: [:], errors: [:], notes: [:], loading: [], message: nil,
            demoReport: nil, appUpdate: AppUpdateState(phase: .downloading, version: "0.1.10", progress: 42))
        let copy = try decoder.decode(DashboardState.self, from: encoder.encode(state))
        #expect(copy.appUpdate == state.appUpdate && !copy.settings.areAutomaticUpdatesEnabled)
    }
}
