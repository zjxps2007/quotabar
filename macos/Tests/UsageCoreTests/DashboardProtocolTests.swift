import Foundation
import Testing
@testable import UsageCore

struct DashboardProtocolTests {
    @Test func arbitraryPipeFragmentsPreserveMessagesAndOrder() throws {
        let messages = [Data("첫 상태".utf8), Data(repeating: 91, count: 170_000), Data("닫기".utf8)]
        let stream = try messages.reduce(into: Data()) { $0.append(try PipeFrameDecoder.encode($1)) }
        for chunkSize in [1, 3, 4, 7, 65_536, stream.count] {
            var decoder = PipeFrameDecoder(), actual: [Data] = []
            for start in stride(from: 0, to: stream.count, by: chunkSize) {
                actual += try decoder.append(stream[start..<min(start + chunkSize, stream.count)])
            }
            try decoder.finish()
            #expect(actual == messages)
        }
    }

    @Test func invalidAndTruncatedFramesFailBeforeAllocation() throws {
        for header: Data in [Data([0, 0, 0, 0]), Data([0, 64, 0, 1]), Data([255, 255, 255, 255])] {
            var decoder = PipeFrameDecoder()
            #expect(throws: PipeError.self) { try decoder.append(header) }
        }
        for data in [Data([0, 0]), Data([0, 0, 0, 2, 1])] {
            var decoder = PipeFrameDecoder()
            _ = try decoder.append(data)
            #expect(throws: PipeError.self) { try decoder.finish() }
        }
        #expect(throws: PipeError.self) { try PipeFrameDecoder.encode(Data()) }
        #expect(throws: PipeError.self) { try PipeFrameDecoder.encode(Data(repeating: 0, count: PipeFrameDecoder.maximumSize + 1)) }
    }

    @Test func credentialPayloadAndRequestIdentityRoundTrip() throws {
        let requestID = UUID(), accountID = UUID(), credential = Data(repeating: 87, count: 1_048_576)
        let request = DashboardCommand.codexLogin(id: requestID, data: credential, alias: "테스트", replacing: accountID)
        let encoded = try JSONEncoder().encode(request)
        var decoder = PipeFrameDecoder()
        let frame = try #require(decoder.append(PipeFrameDecoder.encode(encoded)).first)
        guard case .codexLogin(let id, let data, let alias, let replacing) = try JSONDecoder().decode(DashboardCommand.self, from: frame) else {
            Issue.record("Wrong command after decoding"); return
        }
        #expect(id == requestID && replacing == accountID && data == credential && alias == "테스트")
    }

    @Test func displayStatePreservesMultiAccountSelectionAndErrors() throws {
        var settings = LiteSettings()
        var account = MonitoredAccount(provider: .codex, name: "Fixture", identity: "fixture", sourcePath: "")
        account.showInMenuBar = false; account.appManagedLogin = true
        settings.accounts = [account]
        settings.appearance = .dark; settings.showDockIcon = false; settings.menuBarDisplayMode = .remainingOnly
        let state = DashboardState(settings: settings, snapshots: [:], errors: [account.id: "Failed"],
                                   notes: [:], loading: [account.id], message: nil, demoReport: nil)
        let encoded = try JSONEncoder().encode(DashboardEvent.state(state))
        guard case .state(let copy) = try JSONDecoder().decode(DashboardEvent.self, from: encoded) else {
            Issue.record("Wrong event after decoding"); return
        }
        #expect(copy.settings.accounts == [account])
        #expect(copy.settings.selectedAppearance == .dark && !copy.settings.isDockIconVisible)
        #expect(copy.settings.selectedMenuBarDisplayMode == .remainingOnly)
        #expect(copy.errors[account.id] == "Failed" && copy.loading.contains(account.id))
        #expect(copy.demoReport == nil)
    }

    @Test func anonymousPipeDeliversOrderedMessagesAndClosesPeerOnce() async throws {
        let a = Pipe(), b = Pipe(), received = PipeTestResult()
        let sender = DashboardPipe<Int, Int>(input: b.fileHandleForReading, output: a.fileHandleForWriting)
        let receiver = DashboardPipe<Int, Int>(input: a.fileHandleForReading, output: b.fileHandleForWriting)
        sender.start(onMessage: { _ in }, onClose: {})
        receiver.start(onMessage: { received.add($0) }, onClose: { received.didClose() })
        defer { sender.close(); receiver.close() }
        for value in 0..<100 { sender.send(value) }
        for _ in 0..<150 {
            if received.values.count == 100 { break }
            try await Task.sleep(for: .milliseconds(20))
        }
        #expect(received.values == Array(0..<100))
        sender.close()
        for _ in 0..<150 {
            if received.closes == 1 { break }
            try await Task.sleep(for: .milliseconds(20))
        }
        receiver.close()
        #expect(received.closes == 1)
    }
}

private final class PipeTestResult: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: [Int] = []
    private var count = 0
    var values: [Int] { lock.lock(); defer { lock.unlock() }; return stored }
    var closes: Int { lock.lock(); defer { lock.unlock() }; return count }
    func add(_ value: Int) { lock.lock(); stored.append(value); lock.unlock() }
    func didClose() { lock.lock(); count += 1; lock.unlock() }
}
