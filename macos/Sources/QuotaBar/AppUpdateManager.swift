import Foundation
import UsageCore

@MainActor
final class AppUpdateManager {
    private let client = ReleaseClient()
    private let downloader = UpdateDownloader()
    private let onState: (AppUpdateState) -> Void
    private let prepareExit: () async -> Void
    private let resume: () -> Void
    private let exit: () -> Void
    private var task: Task<Void, Never>?
    private var timer: Timer?
    private var lastAttempt: Date?
    private var staged: StagedUpdate?
    private var job: URL?
    private var blockedVersion: String?
    private var enabled = false
    private var canInstall = false
    private var suspended = false
    private var state = AppUpdateState() { didSet { onState(state) } }
    private var currentVersion: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev" }

    init(onState: @escaping (AppUpdateState) -> Void, prepareExit: @escaping () async -> Void,
         resume: @escaping () -> Void, exit: @escaping () -> Void) {
        self.onState = onState; self.prepareExit = prepareExit; self.resume = resume; self.exit = exit
        if let data = try? Data(contentsOf: MacUpdateInstaller.resultURL), data.count < 4096,
           let previous = try? JSONDecoder().decode(UpdateInstallResult.self, from: data), !previous.success {
            blockedVersion = previous.version
            state = AppUpdateState(phase: .failed, version: previous.version, message: previous.message)
        }
    }

    func configure(enabled: Bool, canInstall: Bool, suspended: Bool = false) {
        self.enabled = enabled; self.canInstall = canInstall; self.suspended = suspended
        onState(state)
        if !enabled || suspended {
            timer?.invalidate(); timer = nil
            if state.phase != .installing { task?.cancel() }
        }
        if enabled, !suspended, canInstall, staged != nil, task == nil { install(force: false) }
        else { schedule() }
    }

    func check(manual: Bool = true, install: Bool = false) {
        guard task == nil, !suspended else { return }
        timer?.invalidate(); timer = nil
        if manual { blockedVersion = nil }
        task = Task { [weak self] in
            guard let self else { return }
            defer { self.task = nil; self.schedule() }
            do {
                self.discardPreparedUpdate()
                self.state = AppUpdateState(phase: .checking)
                self.lastAttempt = Date()
                #if arch(arm64)
                let platform: ReleasePlatform = .macOSArm64
                #else
                let platform: ReleasePlatform = .macOSX64
                #endif
                guard let release = try await self.client.latest(platform: platform, currentVersion: self.currentVersion) else {
                    self.state = AppUpdateState(phase: .idle, message: "공개된 정식 릴리즈가 없습니다.", checkedAt: Date()); return
                }
                try Task.checkCancellation()
                guard release.isNewer(than: self.currentVersion) else {
                    self.state = AppUpdateState(phase: .current, version: self.currentVersion, checkedAt: Date()); return
                }
                guard release.version != self.blockedVersion else {
                    self.state = AppUpdateState(phase: .failed, version: release.version,
                        message: "이 버전의 자동 설치에 실패했습니다. ‘업데이트 확인’으로 다시 시도할 수 있습니다.", checkedAt: Date()); return
                }
                self.state = AppUpdateState(phase: .available, version: release.version, checkedAt: Date())
                guard self.enabled || install else { return }
                guard release.asset != nil else {
                    self.state.message = "이 Mac에 맞는 업데이트 파일이 아직 없습니다."; return
                }
                guard let key = MacUpdateInstaller.publicKey else { throw CocoaError(.fileReadCorruptFile) }
                self.state.phase = .downloading; self.state.progress = 0
                let directory = MacUpdateInstaller.updatesRoot.appendingPathComponent(UUID().uuidString)
                let payload = try await self.downloader.prepare(release: release, currentVersion: self.currentVersion,
                    publicKeyPEM: key, directory: directory) { [weak self] value in
                        Task { @MainActor in
                            guard self?.state.phase == .downloading else { return }
                            self?.state.progress = value
                        }
                    }
                self.staged = payload
                try Task.checkCancellation()
                self.state = AppUpdateState(phase: .ready, version: release.version, checkedAt: Date())
                if install || (self.enabled && self.canInstall) { await self.apply(force: install) }
            } catch is CancellationError {
                self.lastAttempt = nil; self.discardPreparedUpdate(); self.state = AppUpdateState()
            } catch {
                if Task.isCancelled {
                    self.lastAttempt = nil; self.discardPreparedUpdate(); self.state = AppUpdateState(); return
                }
                self.state = AppUpdateState(phase: .failed, version: self.state.version,
                    message: (error as? ProviderError)?.message ?? "자동 업데이트를 완료하지 못했습니다. 현재 앱을 유지합니다.", checkedAt: Date())
            }
        }
    }

    func install(force: Bool) {
        guard task == nil, let staged, force || staged.update.release.version != blockedVersion else { return }
        task = Task { [weak self] in
            guard let self else { return }
            await self.apply(force: force)
            self.task = nil; self.schedule()
        }
    }

    func installNow() {
        if staged == nil { check(install: true) }
        else { install(force: true) }
    }

    private func apply(force: Bool) async {
        guard let staged, !suspended, force || (enabled && canInstall) else { return }
        var helper: Process?
        var paused = false
        do {
            state.phase = .installing
            if job == nil {
                let target = Bundle.main.bundleURL, version = currentVersion
                job = try await Task.detached { try MacUpdateInstaller.prepare(staged, target: target, currentVersion: version) }.value
            }
            try Task.checkCancellation()
            guard !suspended, force || (enabled && canInstall) else { state.phase = .ready; return }
            await prepareExit(); paused = true
            try Task.checkCancellation()
            guard !suspended, force || (enabled && canInstall) else { resume(); state.phase = .ready; return }
            let process = try MacUpdateInstaller.launchHelper(job: job!); helper = process
            let ready = staged.directory.appendingPathComponent("installer-ready.json")
            var acknowledged = false
            for _ in 0..<200 {
                try Task.checkCancellation()
                guard process.isRunning else { throw CocoaError(.executableLoad) }
                if let data = try? Data(contentsOf: ready), data.count < 1024,
                   let result = try? JSONDecoder().decode(UpdateInstallResult.self, from: data), result.success,
                   result.version == staged.update.release.version { acknowledged = true; break }
                try await Task.sleep(for: .milliseconds(100))
            }
            guard acknowledged, process.isRunning else { throw CocoaError(.executableLoad) }
            guard !suspended, force || (enabled && canInstall) else {
                process.terminate(); resume(); state.phase = .ready; return
            }
            exit()
        } catch {
            if let helper, helper.isRunning { helper.terminate() }
            if paused { resume() }
            blockedVersion = staged.update.release.version
            state = AppUpdateState(phase: .failed, version: staged.update.release.version,
                message: "업데이트 설치를 시작하지 못했습니다. 현재 앱을 유지합니다.", checkedAt: Date())
        }
    }

    private func schedule() {
        timer?.invalidate(); timer = nil
        guard enabled, !suspended, task == nil, staged == nil else { return }
        let delay = max(15, (lastAttempt?.addingTimeInterval(24 * 60 * 60).timeIntervalSinceNow) ?? 15)
        timer = Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.check(manual: false) }
        }
        timer?.tolerance = min(60, delay / 10)
    }

    private func discardPreparedUpdate() {
        guard let staged else { return }
        if let transaction = UUID(uuidString: staged.directory.lastPathComponent),
           let replacement = try? UpdateReplacement(target: Bundle.main.bundleURL, transaction: transaction),
           !FileManager.default.fileExists(atPath: replacement.backup.path) {
            try? FileManager.default.removeItem(at: replacement.incoming)
        }
        try? FileManager.default.removeItem(at: staged.directory)
        self.staged = nil; job = nil
    }
}
