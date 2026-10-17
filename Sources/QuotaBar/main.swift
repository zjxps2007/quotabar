import Foundation
import AppKit
import Darwin
import UsageCore

// Claude invokes this short-lived mode before SwiftUI or monitoring is started.
if CommandLine.arguments.contains("--claude-statusline") {
    exit(ClaudeSubscriptionStatusLine.run(arguments: CommandLine.arguments))
}
let dashboardHelper = CommandLine.arguments.contains("--dashboard-helper")
if dashboardHelper {
    // A helper can only run over the anonymous pipes supplied by its parent.
    // It must not silently become another monitoring instance when double-clicked.
    var input = stat(), output = stat()
    guard fstat(STDIN_FILENO, &input) == 0, fstat(STDOUT_FILENO, &output) == 0,
          input.st_mode & S_IFMT == S_IFIFO, output.st_mode & S_IFMT == S_IFIFO else { exit(1) }
}
signal(SIGPIPE, SIG_IGN)
// A second normal instance must not rotate the same refresh token concurrently.
// The descriptor stays open for the app's lifetime and is not inherited by login children.
var instanceLock: Int32 = -1
if !dashboardHelper, !CommandLine.arguments.contains("--demo") {
    // Retain the pre-QuotaBar path and bundle ID so upgrades share credentials and the instance lock.
    let directory = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/AIUsage Lite")
    do {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        instanceLock = open(directory.appendingPathComponent("instance.lock").path, O_CREAT | O_RDWR | O_CLOEXEC, 0o600)
        guard instanceLock >= 0 else { throw CocoaError(.fileWriteUnknown) }
        if flock(instanceLock, LOCK_EX | LOCK_NB) != 0 {
            if errno == EWOULDBLOCK {
                NSRunningApplication.runningApplications(withBundleIdentifier: "com.aiusage.lite")
                    .first { $0.processIdentifier != ProcessInfo.processInfo.processIdentifier }?.activate(options: [])
                exit(0)
            }
            throw CocoaError(.fileWriteUnknown)
        }
    } catch {
        fputs("QuotaBar 실행 상태를 확인하지 못했습니다. 앱 저장 폴더의 권한을 확인하세요.\n", stderr)
        exit(1)
    }
}
MainActor.assumeIsolated {
    let application = NSApplication.shared
    let delegate = AppDelegate()
    application.delegate = delegate
    withExtendedLifetime(delegate) { application.run() }
}
