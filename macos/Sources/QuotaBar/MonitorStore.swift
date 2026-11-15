import AppKit
import CQuotaBar
import UsageCore
import Darwin

struct FetchOutcome: Sendable {
    let snapshot: AccountSnapshot?
    let error: String?
    let note: String?
    var updatedCredential: Data? = nil
}

actor MonitorService {
    let vault = SecretVault()
    let profiles: ClaudeSubscriptionStore
    let codexSessions: ManagedCodexAccounts
    init(root: URL, codexSessions: ManagedCodexAccounts) {
        profiles = ClaudeSubscriptionStore(root: root.appendingPathComponent("ClaudeProfiles"))
        self.codexSessions = codexSessions
    }
    func fetch(_ account: MonitoredAccount) async -> FetchOutcome {
        do {
            let usage: ProviderUsage
            var note: String?
            var updatedCredential: Data?
            if account.usesAppLogin {
                usage = try await codexSessions.fetch(id: account.id, identity: account.identity)
            } else if account.provider == .claude {
                usage = try await ClaudeSubscriptionProvider(store: profiles).fetchUsage(with:
                    AccountCredential(providerId: "claude-subscription", authMethod: .auto,
                        credential: account.sourcePath, metadata: ["sourceKind": "claude-cli-profile"]))
            } else {
                let selected = try vault.imported(account)
                let data = selected.data
                note = selected.note
                if selected.updated { updatedCredential = data }
                switch account.provider {
                case .codex: usage = try await CodexProvider().fetch(authData: data)
                case .gemini: usage = try await GeminiProvider().fetch(authData: data)
                case .claude: fatalError("Handled above")
                }
            }
            try Task.checkCancellation()
            return FetchOutcome(snapshot: AccountSnapshot(usage: usage, provider: account.provider), error: nil, note: note, updatedCredential: updatedCredential)
        } catch is CancellationError { return FetchOutcome(snapshot: nil, error: nil, note: nil) }
        catch {
            let message: String
            if let e = error as? ProviderError {
                switch e.code {
                case "unauthorized", "not_logged_in":
                    message = account.usesAppLogin ? "계정 접근이 거부되었습니다. 앱에서 다시 로그인하세요."
                        : "로그인이 만료되었거나 접근이 거부되었습니다. Codex는 앱에서 다시 로그인하거나, 해당 CLI의 로그인 파일을 갱신하세요."
                case "gemini_individual_discontinued": message = "이 계정의 Gemini CLI 접근이 지원되지 않습니다. Code Assist Standard 또는 Enterprise가 필요합니다."
                default: message = e.message
                }
            } else if let e = error as? URLError {
                message = e.code == .timedOut ? "조회 시간이 초과되었습니다. 잠시 후 다시 시도하세요." : "네트워크 연결을 확인하세요. 마지막 성공 결과를 유지합니다."
            } else { message = "조회에 실패했습니다. 연결한 파일과 앱의 접근 권한을 확인하세요." }
            return FetchOutcome(snapshot: nil, error: message, note: nil)
        }
    }
}

@MainActor
final class MonitorStore {
    var onChange: (() -> Void)?
    private(set) var settings = LiteSettings() { didSet { onChange?() } }
    private(set) var snapshots: [UUID: AccountSnapshot] = [:] { didSet { onChange?() } }
    private(set) var errors: [UUID: String] = [:] { didSet { onChange?() } }
    private(set) var notes: [UUID: String] = [:] { didSet { onChange?() } }
    private(set) var loading: Set<UUID> = [] { didSet { onChange?() } }
    private(set) var local = LocalUsageReport() { didSet { onChange?() } }
    private(set) var windowVisible = false { didSet { onChange?() } }
    private(set) var launchAtLogin: LaunchAtLoginState = .disabled { didSet { onChange?() } }
    private(set) var appUpdate = AppUpdateState() { didSet { onChange?() } }
    private var appUpdater: AppUpdateManager?
    private var installingUpdate: Bool { refreshQueue.status.installing != 0 }
    private var dashboardRunning = false
    var message: String? { didSet { onChange?() } }
    let demo: Bool
    let root: URL
    private let settingsFile: SettingsFile
    private let service: MonitorService
    private let loginItem = LaunchAtLoginManager()
    private let vault = SecretVault()
    private let codexSessions = ManagedCodexAccounts()
    private var storageHealthy = true
    private let refreshQueue = AccountRefreshQueue()
    private var workers: [UUID: Task<Void, Never>] = [:]
    private var timer: Timer?
    private var scheduledDeadline: Date?
    private var idleCleanup: Task<Void, Never>?
    private var sleeping: Bool { refreshQueue.status.sleeping != 0 }
    private var lastScheduled = Date.distantPast
    private var observers: [NSObjectProtocol] = []
    var accounts: [MonitoredAccount] { settings.accounts }
    var enabledAccounts: [MonitoredAccount] { accounts.filter(\.enabled) }
    var menuBarAccounts: [MonitoredAccount] { accounts.filter { $0.enabled && $0.isShownInMenuBar } }

    private let menuBarCache = MenuBarSummary.Cache()
    private var menuBarSummary: MenuBarSummary {
        menuBarCache.summary(accounts: accounts, snapshots: snapshots, errors: errors)
    }
    var menuBarTitle: String { menuBarSummary.title }
    var menuBarRemaining: String { menuBarSummary.remaining }
    var menuBarHelp: String { menuBarSummary.help }
    var menuBarEmptyMessage: String { menuBarSummary.emptyMessage }

    init(demo: Bool = ProcessInfo.processInfo.arguments.contains("--demo")) {
        self.demo = demo
        // The display name changed to QuotaBar; keep the existing accounts and login storage.
        root = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/AIUsage Lite")
        settingsFile = SettingsFile(url: root.appendingPathComponent("settings.json"))
        service = MonitorService(root: root, codexSessions: codexSessions)
        if demo {
            let args = ProcessInfo.processInfo.arguments
            if let index = args.firstIndex(of: "--demo-appearance"), args.indices.contains(index + 1) {
                settings.appearance = AppearanceMode(rawValue: args[index + 1])
            }
            AppTheme.apply(settings.selectedAppearance)
            loadDemo()
            return
        }
        do { settings = try settingsFile.load() }
        catch {
            storageHealthy = false
            message = "저장된 설정을 읽을 수 없어 쓰기를 중단했습니다. settings.json을 확인한 뒤 앱을 다시 실행하세요."
        }
        AppTheme.apply(settings.selectedAppearance)
        if storageHealthy {
            do { settings = try CredentialMigration.run(settings: settings, file: settingsFile, vault: vault) }
            catch {
                storageHealthy = false
                message = "로그인 저장소를 준비하지 못했습니다. 앱 데이터 폴더의 권한과 저장 공간을 확인한 뒤 다시 실행하세요."
            }
        }
        settings.refreshSeconds = RefreshSchedule.intervals.contains(settings.refreshSeconds) ? settings.refreshSeconds : 300
        refreshLaunchAtLogin()
        let center = NSWorkspace.shared.notificationCenter
        observers.append(center.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.suspend() }
        })
        observers.append(center.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.wake() }
        })
        refreshAccounts()
        appUpdater = AppUpdateManager(onState: { [weak self] value in
            if self?.appUpdate != value { self?.appUpdate = value }
        }, prepareExit: { [weak self] in
            guard let self else { return }
            self.refreshQueue.setInstalling(true)
            self.publishLoading()
            self.timer?.invalidate(); self.timer = nil
            let running = Array(self.workers.values)
            for task in running { await task.value }
            try await self.codexSessions.finishPendingWrites()
        }, resume: { [weak self] in
            self?.refreshQueue.setInstalling(false); self?.refreshAccounts(); self?.scheduleNextRefresh()
        }, exit: { NSApp.terminate(nil) })
        configureAppUpdates()
    }

    func setVisible(_ value: Bool) {
        guard value || windowVisible else { return }
        idleCleanup?.cancel(); idleCleanup = nil
        let changed = windowVisible != value
        if windowVisible != value {
            windowVisible = value
        }
        if !value { reclaimIdleMemory() }
        scheduleNextRefresh()
        if changed { diagnostic(value ? "window-visible" : "window-hidden") }
        configureAppUpdates()
    }
    func reclaimIdleMemory() {
        guard !windowVisible else { return }
        idleCleanup?.cancel()
        idleCleanup = Task { [weak self] in
            // Let the closed window/popover finish releasing its hosting view first.
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled, let self, !self.windowVisible else { return }
            let released = malloc_zone_pressure_relief(nil, 0)
            self.idleCleanup = nil
            self.diagnostic("idle-reclaimed", details: ["releasedBytes": released])
        }
    }
    private var refreshSchedule: RefreshSchedule {
        RefreshSchedule(suspended: sleeping || demo || !storageHealthy || installingUpdate,
            hasAccounts: accounts.contains(where: \.enabled), windowVisible: windowVisible,
            dashboardSelected: false, scanLocalLogs: false,
            scanning: false, refreshSeconds: settings.refreshSeconds,
            lastAccountRefresh: lastScheduled, lastLocalAttempt: nil,
            autoRefresh: settings.isAutoRefreshEnabled)
    }
    private func scheduleNextRefresh() {
        let deadline = refreshSchedule.next
        if timer?.isValid == true, scheduledDeadline == deadline { return }
        timer?.invalidate(); timer = nil; scheduledDeadline = deadline
        guard let deadline else { return }
        let delay = max(1, deadline.timeIntervalSinceNow)
        timer = Timer.scheduledTimer(withTimeInterval: delay, repeats: false) { [weak self] _ in
            // This timer is installed on the main run loop by the main actor.
            MainActor.assumeIsolated { self?.performScheduledRefresh() }
        }
        timer?.tolerance = min(30, delay / 10)
    }
    private func performScheduledRefresh() {
        timer = nil; scheduledDeadline = nil
        let schedule = refreshSchedule, now = Date()
        if let due = schedule.accounts, due <= now { refreshAccounts() }
        scheduleNextRefresh()
        diagnostic("scheduled-refresh")
    }
    private func suspend() {
        refreshQueue.setSleeping(true)
        for task in workers.values { task.cancel() }
        workers.removeAll(); publishLoading()
        scheduleNextRefresh()
        configureAppUpdates()
    }
    private func wake() {
        refreshQueue.setSleeping(false)
        if settings.isAutoRefreshEnabled {
            refreshAccounts()
        }
        scheduleNextRefresh()
        configureAppUpdates()
    }
    func refreshAccounts() {
        guard !demo, storageHealthy, !installingUpdate else { return }
        lastScheduled = Date()
        enqueue(enabledAccounts.map(\.id))
        scheduleNextRefresh()
    }
    func refresh(_ account: MonitoredAccount) {
        guard account.enabled, !demo, storageHealthy, !installingUpdate else { return }
        enqueue([account.id])
    }
    private func enqueue(_ ids: [UUID]) {
        for id in ids { refreshQueue.enqueue(id) }
        drain()
    }
    private func publishLoading() {
        let ids = refreshQueue.loading
        if loading != ids { loading = ids }
    }
    private func drain() {
        while let job = refreshQueue.next() {
            let id = UUID(uuid: job.account.bytes)
            guard let account = accounts.first(where: { $0.id == id && $0.enabled }) else {
                _ = refreshQueue.finish(job)
                continue
            }
            workers[id] = Task { [weak self, service] in
                let result = await service.fetch(account)
                guard let self, self.refreshQueue.finish(job) else { return }
                self.workers.removeValue(forKey: id)
                defer { self.drain() }
                guard !Task.isCancelled,
                      self.accounts.contains(where: { $0.id == id && $0.enabled }) else { return }
                // Only the current worker can publish refreshed CLI credentials.
                if let data = result.updatedCredential {
                    do { try self.vault.save(data, id: id) }
                    catch { self.message = error.localizedDescription }
                }
                if let snapshot = result.snapshot { self.snapshots[id] = snapshot }
                if self.errors[id] != result.error { self.errors[id] = result.error }
                if self.notes[id] != result.note { self.notes[id] = result.note }
            }
        }
        publishLoading()
    }

    @discardableResult
    private func change(_ edit: SettingsEdit, credential: Data = Data()) throws -> SettingsChange {
        let storage: SettingsStorage = storageHealthy ? (demo ? .memory : .disk) : .unavailable
        let result = try settingsFile.apply(edit, to: settings, storage: storage, vault: vault, credential: credential)
        if result.changed {
            settings = result.settings
            scheduleNextRefresh()
            configureAppUpdates()
        }
        return result
    }
    private func update(_ edit: SettingsEdit, then: (SettingsChange) -> Void = { _ in }) {
        do {
            let result = try change(edit)
            if result.changed { then(result) }
        } catch { message = error.localizedDescription }
    }
    private func configureAppUpdates() {
        appUpdater?.configure(enabled: settings.areAutomaticUpdatesEnabled && storageHealthy,
            canInstall: !windowVisible && !dashboardRunning, suspended: sleeping)
    }
    func setDashboardRunning(_ value: Bool) { dashboardRunning = value; configureAppUpdates() }
    func setAutomaticUpdates(_ enabled: Bool) {
        update(.automaticUpdates(enabled: enabled))
    }
    func checkAppUpdate() {
        guard !demo else { appUpdate = AppUpdateState(message: "미리보기에서는 업데이트를 내려받거나 설치하지 않습니다."); return }
        appUpdater?.check()
    }
    func installAppUpdate() {
        guard !demo else { return }
        appUpdater?.installNow()
    }
    func setInterval(_ seconds: Int) {
        update(.interval(seconds: seconds))
    }
    func setAutoRefresh(_ enabled: Bool) {
        update(.autoRefresh(enabled: enabled))
    }
    func refreshLaunchAtLogin() {
        guard !demo else { return }
        let state = loginItem.state
        if launchAtLogin != state { launchAtLogin = state }
    }
    func setLaunchAtLogin(_ enabled: Bool) {
        guard !demo else { launchAtLogin = enabled ? .enabled : .disabled; return }
        defer { refreshLaunchAtLogin() }
        do { try loginItem.setEnabled(enabled) }
        catch {
            message = (error as? ProviderError)?.message
                ?? "자동 실행 설정을 변경하지 못했습니다. 시스템 설정의 로그인 항목을 확인하세요."
        }
    }
    func openLoginItemsSettings() {
        if !demo { LaunchAtLoginManager.openSettings() }
    }
    func setAppearance(_ mode: AppearanceMode) {
        update(.appearance(mode: mode)) { _ in AppTheme.apply(mode) }
    }
    func setDockIconVisible(_ visible: Bool) {
        update(.showDockIcon(visible: visible)) { _ in applyDockIconVisibility() }
    }
    func applyDockIconVisibility() {
        if qb_mac_dock_visible(settings.isDockIconVisible ? 1 : 0) == 0 {
            message = "Dock 표시를 변경하지 못했습니다. 앱을 다시 실행해 주세요."
        }
    }
    func setMenuBarDisplayMode(_ mode: MenuBarDisplayMode) {
        update(.menuBarDisplayMode(mode: mode))
    }
    func setScanLogs(_ value: Bool) {
        update(.scanLocalLogs(enabled: value)) { _ in if demo && !value { local = LocalUsageReport() } }
    }
    func toggle(_ account: MonitoredAccount) {
        update(.toggle(id: account.id)) { result in
            guard let row = result.account else { return }
            if row.enabled { refresh(row) } else { cancel(row.id) }
        }
    }
    func setMenuBarVisibility(_ visible: Bool, for account: MonitoredAccount) {
        update(.menuVisibility(id: account.id, visible: visible))
    }
    private func cancel(_ id: UUID) {
        refreshQueue.cancel(id)
        workers.removeValue(forKey: id)?.cancel()
        drain()
    }

    func connectCodexLogin(_ data: Data, alias: String, replacing target: MonitoredAccount?) throws {
        guard !demo, storageHealthy else { throw ProviderError("login_unavailable", "현재 상태에서는 계정을 연결할 수 없습니다.") }
        let name = alias.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let account = try change(.managedLogin(alias: name, target: target), credential: data).account else { return }
        codexSessions.invalidate(account.id)
        cancel(account.id); errors.removeValue(forKey: account.id); notes.removeValue(forKey: account.id)
        refresh(account)
    }

    func connect(provider: ProviderKind, path: String, alias: String) throws {
        guard !demo else { throw ProviderError("demo", "미리보기에서는 계정을 저장하지 않습니다. 실제 앱을 열어 연결하세요.") }
        guard storageHealthy else { throw ProviderError("settings_unavailable", "설정을 먼저 복구하세요.") }
        let url = URL(fileURLWithPath: NSString(string: path.trimmingCharacters(in: .whitespacesAndNewlines)).expandingTildeInPath)
            .standardizedFileURL.resolvingSymlinksInPath()
        let name = alias.trimmingCharacters(in: .whitespacesAndNewlines)
        if provider == .claude { try connectClaude(directory: url, name: name); return }
        guard let account = try change(.importLogin(provider: provider, path: url.path, alias: name)).account else { return }
        codexSessions.invalidate(account.id)
        cancel(account.id); refresh(account)
    }

    private func connectClaude(directory: URL, name: String) throws {
        guard let helper = Bundle.main.executableURL?.path else { throw ProviderError("helper", "앱 실행 파일을 찾을 수 없습니다.") }
        let result = try change(.bindClaude(directory: directory.path, helper: helper, alias: name))
        if let account = result.account { cancel(account.id); refresh(account) }
    }

    func remove(_ account: MonitoredAccount) {
        do {
            guard demo || storageHealthy else { throw ProviderError("settings_unavailable", "설정을 먼저 복구하세요.") }
            if !demo && account.provider == .claude {
                try change(.removeClaude(id: account.id, identity: account.identity))
            } else {
                try change(.remove(id: account.id, deleteCredential: account.provider != .claude))
            }
            codexSessions.invalidate(account.id)
            cancel(account.id); snapshots.removeValue(forKey: account.id); errors.removeValue(forKey: account.id); notes.removeValue(forKey: account.id)
        } catch { message = error.localizedDescription }
    }

    var dashboardState: DashboardState {
        DashboardState(settings: settings, snapshots: snapshots, errors: errors, notes: notes,
                       loading: loading, message: message, demoReport: demo ? local : nil, launchAtLogin: launchAtLogin, appUpdate: appUpdate)
    }

    /// Explicit local smoke-test diagnostics contain only lifecycle counters.
    func diagnostic(_ event: String, details: [String: Any] = [:]) {
        let args = ProcessInfo.processInfo.arguments
        guard let index = args.firstIndex(of: "--diagnostics"), args.indices.contains(index + 1) else { return }
        let url = URL(fileURLWithPath: args[index + 1])
        let refresh = refreshQueue.status
        var fields: [String: Any] = ["event": event, "date": Date().timeIntervalSince1970,
            "windowVisible": windowVisible, "scanning": false, "runningRequests": refresh.running,
            "queuedRequests": refresh.queued, "demo": demo,
            "nextRefresh": scheduledDeadline?.timeIntervalSince1970 as Any? ?? NSNull(),
            "timerActive": timer?.isValid == true]
        fields.merge(details) { _, value in value }
        guard var data = try? JSONSerialization.data(withJSONObject: fields, options: [.sortedKeys]) else { return }
        data.append(10)
        if let file = try? FileHandle(forWritingTo: url) {
            defer { try? file.close() }
            _ = try? file.seekToEnd(); try? file.write(contentsOf: data)
        } else { try? data.write(to: url) }
    }

    private func loadDemo() {
        let now = Date()
        for (kind, name, remaining, plan) in [(ProviderKind.codex, "Personal", 72.0, "Plus"), (.codex, "Research lab", 38.0, "Business"),
                                             (.claude, "Writing & research", 16.0, "Max"), (.gemini, "Workspace", 84.0, "Standard")] {
            let account = MonitoredAccount(provider: kind, name: name, identity: UUID().uuidString, sourcePath: kind.defaultPath)
            settings.accounts.append(account)
            var usage = ProviderUsage(provider: kind.rawValue, label: kind.title)
            usage.accountPlan = plan; usage.accountEmail = "preview@example.com"
            var first = RawQuotaWindow(); first.remainingPercent = remaining
            first.resetAt = SharedFormatters.iso8601String(from: now.addingTimeInterval(7200))
            var second = RawQuotaWindow(); second.remainingPercent = min(95, remaining + 12)
            second.resetAt = SharedFormatters.iso8601String(from: now.addingTimeInterval(172800))
            usage.primary = first; usage.secondary = second
            snapshots[account.id] = AccountSnapshot(usage: usage, provider: kind)
        }
        // Deterministic sample data; no real credentials, logs or network are read.
        let points = (0..<UsageActivity.historyDays).flatMap { day -> [[String: Any]] in
            let date = Calendar.current.date(byAdding: .day, value: -day, to: now)!
            let key = DateFormat.string(from: date, format: "yyyy-MM-dd")
            return ["codex", "claude"].enumerated().map { index, provider in
                let value = ((day * 7919 + index * 1777) % 34000 + 16000) * (day % 7 == 5 ? 1 : 4)
                return ["day": key, "provider": provider, "model": index == 0 ? "gpt-5" : "claude-sonnet",
                        "counts": ["input": value, "cached": value / 3, "output": value / 5]]
            }
        }
        if let data = try? JSONSerialization.data(withJSONObject: ["days": points, "scannedFiles": 42, "warnings": []]),
           var report = try? JSONDecoder().decode(LocalUsageReport.self, from: data) {
            report.scannedAt = now; local = report
        }
    }
}
