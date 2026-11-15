import Foundation
import UsageCore
import CQuotaBar

@MainActor
final class AppUpdateManager {
    private let client = ReleaseClient()
    private let downloader = UpdateDownloader()
    private let onState: (AppUpdateState) -> Void
    private let prepareExit: () async throws -> Void
    private let resume: () -> Void
    private let exit: () -> Void
    private var task: Task<Void, Never>?
    private var timer: Timer?
    private let policy = qb_update_policy_new()!
    private var staged: StagedUpdate?
    private var job: URL?
    private var state = AppUpdateState() { didSet { onState(state) } }
    private var currentVersion: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev" }

    init(onState: @escaping (AppUpdateState) -> Void, prepareExit: @escaping () async throws -> Void,
         resume: @escaping () -> Void, exit: @escaping () -> Void) {
        self.onState = onState; self.prepareExit = prepareExit; self.resume = resume; self.exit = exit
        if let data = try? Data(contentsOf: MacUpdateInstaller.resultURL), data.count < 4096,
           let previous = try? JSONDecoder().decode(UpdateInstallResult.self, from: data), !previous.success {
            qb_update_block(policy, previous.version)
            state = AppUpdateState(phase: .failed, version: previous.version, message: previous.message)
        }
    }

    deinit { qb_update_policy_free(policy) }

    func configure(enabled: Bool, canInstall: Bool, suspended: Bool = false) {
        let action = qb_update_configure(policy, enabled ? 1 : 0, canInstall ? 1 : 0, suspended ? 1 : 0)
        onState(state)
        if action == QBUpdateCancel { task?.cancel() }
        if action == QBUpdateInstall { install(force: false) }
        else { schedule() }
    }

    func check(manual: Bool = true, install: Bool = false) {
        guard qb_update_begin_check(policy, manual ? 1 : 0) != 0 else { return }
        timer?.invalidate(); timer = nil
        task = Task { [weak self] in
            guard let self else { return }
            var cancelled = false
            defer {
                self.task = nil
                qb_update_finish(self.policy, cancelled ? 1 : 0)
                self.schedule()
            }
            do {
                self.discardPreparedUpdate()
                self.state = AppUpdateState(phase: .checking)
                qb_update_attempted(self.policy, Date().timeIntervalSince1970)
                #if arch(arm64)
                let platform: ReleasePlatform = .macOSArm64
                #else
                let platform: ReleasePlatform = .macOSX64
                #endif
                guard let release = try await self.client.latest(platform: platform, currentVersion: self.currentVersion) else {
                    self.state = AppUpdateState(phase: .idle, message: "공개된 정식 릴리즈가 없습니다.", checkedAt: Date()); return
                }
                try Task.checkCancellation()
                let offer = qb_update_offer(self.policy, self.currentVersion, release.version)
                if offer == QBUpdateCurrent {
                    self.state = AppUpdateState(phase: .current, version: self.currentVersion, checkedAt: Date()); return
                }
                if offer == QBUpdateBlocked {
                    self.state = AppUpdateState(phase: .failed, version: release.version,
                        message: "이 버전의 자동 설치에 실패했습니다. ‘업데이트 확인’으로 다시 시도할 수 있습니다.", checkedAt: Date()); return
                }
                self.state = AppUpdateState(phase: .available, version: release.version, checkedAt: Date())
                guard qb_update_should_download(self.policy, install ? 1 : 0) != 0 else { return }
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
                qb_update_staged(self.policy, release.version)
                try Task.checkCancellation()
                self.state = AppUpdateState(phase: .ready, version: release.version, checkedAt: Date())
                await self.apply(force: install)
            } catch is CancellationError {
                cancelled = true; self.discardPreparedUpdate(); self.state = AppUpdateState()
            } catch {
                if Task.isCancelled {
                    cancelled = true; self.discardPreparedUpdate(); self.state = AppUpdateState(); return
                }
                self.state = AppUpdateState(phase: .failed, version: self.state.version,
                    message: (error as? ProviderError)?.message ?? "자동 업데이트를 완료하지 못했습니다. 현재 앱을 유지합니다.", checkedAt: Date())
            }
        }
    }

    func install(force: Bool) {
        guard qb_update_begin_install(policy, force ? 1 : 0) != 0 else { return }
        task = Task { [weak self] in
            guard let self else { return }
            await self.apply(force: force)
            self.task = nil
            qb_update_finish(self.policy, 0)
            self.schedule()
        }
    }

    func installNow() {
        if staged == nil { check(install: true) }
        else { install(force: true) }
    }

    private func apply(force: Bool) async {
        let requested: Int32 = force ? 1 : 0
        guard let staged, qb_update_begin_handoff(policy, requested) != 0 else { return }
        var helper: MacUpdateInstaller.Helper?
        var paused = false
        do {
            state.phase = .installing
            if job == nil {
                let target = Bundle.main.bundleURL, version = currentVersion
                job = try await Task.detached { try MacUpdateInstaller.prepare(staged, target: target, currentVersion: version) }.value
            }
            try Task.checkCancellation()
            guard qb_update_may_install(policy, requested) != 0 else { state.phase = .ready; return }
            paused = true; try await prepareExit()
            try Task.checkCancellation()
            guard qb_update_may_install(policy, requested) != 0 else { resume(); state.phase = .ready; return }
            let process = try MacUpdateInstaller.Helper(job: job!); helper = process
            var acknowledged = false
            for _ in 0..<200 {
                try Task.checkCancellation()
                if try process.isReady() { acknowledged = true; break }
                try await Task.sleep(for: .milliseconds(100))
            }
            guard acknowledged, try process.isReady() else { throw CocoaError(.executableLoad) }
            guard qb_update_may_install(policy, requested) != 0 else {
                process.finish(terminate: true); resume(); state.phase = .ready; return
            }
            process.finish(terminate: false)
            exit()
        } catch {
            helper?.finish(terminate: true)
            if paused { resume() }
            qb_update_block(policy, staged.update.release.version)
            state = AppUpdateState(phase: .failed, version: staged.update.release.version,
                message: "업데이트 설치를 시작하지 못했습니다. 현재 앱을 유지합니다.", checkedAt: Date())
        }
    }

    private func schedule() {
        timer?.invalidate(); timer = nil
        let delay = qb_update_delay(policy, Date().timeIntervalSince1970)
        guard delay > 0 else { return }
        timer = Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.check(manual: false) }
        }
        timer?.tolerance = min(60, delay / 10)
    }

    private func discardPreparedUpdate() {
        guard let staged else { return }
        MacUpdateInstaller.discard(staged)
        self.staged = nil; job = nil
    }
}
