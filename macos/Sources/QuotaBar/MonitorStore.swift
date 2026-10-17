import AppKit
import SwiftUI
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
    let codexSessions: CodexSessionManager
    init(root: URL, codexSessions: CodexSessionManager) {
        profiles = ClaudeSubscriptionStore(root: root.appendingPathComponent("ClaudeProfiles"))
        self.codexSessions = codexSessions
    }
    func fetch(_ account: MonitoredAccount) async -> FetchOutcome {
        do {
            let usage: ProviderUsage
            var note: String?
            var updatedCredential: Data?
            if account.usesAppLogin {
                let data = try await codexSessions.credential(id: account.id, identity: account.identity)
                do { usage = try await CodexProvider().fetch(authData: data) }
                catch let error as ProviderError where error.code == "unauthorized" {
                    let refreshed = try await codexSessions.credential(id: account.id, identity: account.identity, rejected: data)
                    usage = try await CodexProvider().fetch(authData: refreshed)
                }
            } else if account.provider == .claude {
                usage = try await ClaudeSubscriptionProvider(store: profiles).fetchUsage(with:
                    AccountCredential(providerId: "claude-subscription", authMethod: .auto,
                        credential: account.sourcePath, metadata: ["sourceKind": "claude-cli-profile"]))
            } else {
                guard var data = try vault.load(account.id) else {
                    throw ProviderError("missing_credentials", "저장된 로그인이 없습니다. 계정을 다시 연결하세요.")
                }
                let url = URL(fileURLWithPath: account.sourcePath)
                if let attrs = try? FileManager.default.attributesOfItem(atPath: url.path),
                   let size = attrs[.size] as? NSNumber, size.intValue <= 1_048_576,
                   let fresh = try? Data(contentsOf: url),
                   let identity = try? ImportedIdentity.parse(fresh, provider: account.provider) {
                    if identity.key == account.identity {
                        if fresh != data { updatedCredential = fresh; data = fresh }
                    } else { note = "CLI가 다른 계정으로 전환되어 보관된 로그인을 사용합니다." }
                }
                guard try ImportedIdentity.parse(data, provider: account.provider).key == account.identity else {
                    throw ProviderError("account_mismatch", "저장된 로그인과 계정 정보가 일치하지 않습니다. 다시 연결하세요.")
                }
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
final class MonitorStore: ObservableObject {
    @Published private(set) var settings = LiteSettings()
    @Published private(set) var snapshots: [UUID: AccountSnapshot] = [:]
    @Published private(set) var errors: [UUID: String] = [:]
    @Published private(set) var notes: [UUID: String] = [:]
    @Published private(set) var loading: Set<UUID> = []
    @Published private(set) var local = LocalUsageReport()
    @Published private(set) var scanning = false
    @Published private(set) var windowVisible = false
    @Published var message: String? {
        didSet { if !applyingRemoteState, message != oldValue { remote?.send(.message(message)) } }
    }
    @Published var connectionPresented = false
    @Published var loginTarget: MonitoredAccount?
    @Published var route = "dashboard" {
        didSet {
            guard route != oldValue else { return }
            remote?.send(.route(route))
            if route == "dashboard" { refreshLocalIfNeeded() }
            else { scanTask?.cancel(); Task { await scanner.releaseCache() } }
            scheduleNextRefresh()
        }
    }
    let demo: Bool
    let root: URL
    private let remote: DashboardClient?
    private let managesDashboard: Bool
    private var applyingRemoteState = false
    private let settingsFile: SettingsFile
    private let service: MonitorService
    private let scanner = LocalUsageScanner()
    private let vault = SecretVault()
    private let codexSessions = CodexSessionManager()
    private var storageHealthy = true
    private var queue: [UUID] = []
    private var jobs: [UUID: (UUID, Task<Void, Never>)] = [:]
    private var scanTask: Task<Void, Never>?
    private var timer: Timer?
    private var scheduledDeadline: Date?
    private var idleCleanup: Task<Void, Never>?
    private var sleeping = false
    private var lastScheduled = Date.distantPast
    private var lastLocalAttempt: Date?
    private var observers: [NSObjectProtocol] = []
    var accounts: [MonitoredAccount] { settings.accounts }
    var busy: Bool { !loading.isEmpty || scanning }
    var enabledAccounts: [MonitoredAccount] { accounts.filter(\.enabled) }
    var menuBarAccounts: [MonitoredAccount] { accounts.filter { $0.enabled && $0.isShownInMenuBar } }
    var attentionCount: Int { accounts.lazy.filter { $0.enabled && self.needsAttention($0) }.count }

    var menuBarTitle: String { menuBarSummary(includeProvider: true) }
    var menuBarRemaining: String { menuBarSummary(includeProvider: false) }

    private func menuBarSummary(includeProvider: Bool) -> String {
        let visible = menuBarAccounts
        guard !visible.isEmpty else {
            if !includeProvider { return "—" }
            return accounts.isEmpty ? "계정 연결" : enabledAccounts.isEmpty ? "일시 정지" : "QuotaBar"
        }
        return ProviderKind.allCases.compactMap { provider -> String? in
            let accounts = visible.filter { $0.provider == provider }
            guard !accounts.isEmpty else { return nil }
            let prefix = includeProvider ? "\(provider.title) " : ""
            let remaining = accounts.lazy.compactMap { self.snapshots[$0.id]?.lowestRemaining }.min()
            guard let remaining else { return "\(prefix)—" }
            let incomplete = accounts.contains { menuBarValueNeedsUpdate($0) }
            return "\(prefix)\(Int(remaining.rounded()))%\(incomplete ? "*" : "")"
        }.joined(separator: " · ")
    }

    var menuBarHelp: String {
        let visible = menuBarAccounts
        guard !visible.isEmpty else { return "QuotaBar · \(menuBarEmptyMessage)" }
        var lines = ["남은 사용량 · 상단바에 표시한 계정과 사용 기간 중 Provider별 가장 낮은 잔여율"]
        for account in visible {
            let value = snapshots[account.id]?.lowestRemaining.map { "\(Int($0.rounded()))% 남음" } ?? "잔여량 확인 중"
            let status = menuBarValueNeedsUpdate(account) && snapshots[account.id]?.lowestRemaining != nil ? " · 이전 값" : ""
            lines.append("\(account.provider.title)(\(account.name)): \(value)\(status)")
        }
        if visible.contains(where: menuBarValueNeedsUpdate) {
            lines.append("* 일부 계정의 최신 잔여량 확인 필요 · — 조회된 잔여량 없음")
        }
        return lines.joined(separator: "\n")
    }

    var menuBarEmptyMessage: String {
        if accounts.isEmpty { return "Dashboard에서 계정을 연결하세요." }
        if accounts.contains(where: \.isShownInMenuBar) {
            return "표시할 계정의 모니터링이 일시 정지되어 있습니다."
        }
        return "Dashboard의 계정 카드에서 ‘상단바에 표시’를 켜세요."
    }

    private func menuBarValueNeedsUpdate(_ account: MonitoredAccount) -> Bool {
        errors[account.id] != nil || snapshots[account.id]?.lowestRemaining == nil
            || ["waiting", "stale", "disconnected", "api-routed"].contains(snapshots[account.id]?.state ?? "")
    }

    init(demo: Bool = ProcessInfo.processInfo.arguments.contains("--demo"),
         remote: DashboardClient? = nil, managesDashboard: Bool = true) {
        self.demo = demo
        self.remote = remote
        self.managesDashboard = managesDashboard
        // The display name changed to QuotaBar; keep the existing accounts and login storage.
        root = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/AIUsage Lite")
        settingsFile = SettingsFile(url: root.appendingPathComponent("settings.json"))
        service = MonitorService(root: root, codexSessions: codexSessions)
        // The UI is a mirror: it never opens the vault or starts quota requests.
        if remote != nil { return }
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
        settings.refreshSeconds = [60, 300, 900, 1800].contains(settings.refreshSeconds) ? settings.refreshSeconds : 300
        let center = NSWorkspace.shared.notificationCenter
        observers.append(center.addObserver(forName: NSWorkspace.willSleepNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.suspend() }
        })
        observers.append(center.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            Task { @MainActor in self?.wake() }
        })
        refreshAccounts()
    }

    func needsAttention(_ account: MonitoredAccount) -> Bool {
        needsConnectionAttention(account) || (snapshots[account.id]?.lowestRemaining.map { $0 <= 20 } ?? false)
    }
    func needsConnectionAttention(_ account: MonitoredAccount) -> Bool {
        errors[account.id] != nil || ["waiting", "stale", "disconnected", "api-routed"].contains(snapshots[account.id]?.state ?? "")
    }
    func setVisible(_ value: Bool) {
        guard value || windowVisible else { return }
        idleCleanup?.cancel(); idleCleanup = nil
        let changed = windowVisible != value
        if windowVisible != value {
            windowVisible = value
        }
        if changed { remote?.send(.visibility(value)) }
        if value { refreshLocalIfNeeded() }
        else {
            scanTask?.cancel()
            reclaimIdleMemory()
        }
        scheduleNextRefresh()
        if changed { diagnostic(value ? "window-visible" : "window-hidden") }
    }
    func reclaimIdleMemory() {
        guard !windowVisible else { return }
        idleCleanup?.cancel()
        idleCleanup = Task { [weak self, scanner] in
            await scanner.releaseCache()
            // Let the closed window/popover finish releasing its hosting view first.
            try? await Task.sleep(for: .seconds(2))
            guard !Task.isCancelled, let self, !self.windowVisible else { return }
            let released = malloc_zone_pressure_relief(nil, 0)
            self.idleCleanup = nil
            self.diagnostic("idle-reclaimed", details: ["releasedBytes": released])
        }
    }
    private var refreshSchedule: RefreshSchedule {
        RefreshSchedule(suspended: sleeping || demo || !storageHealthy,
            hasAccounts: remote == nil && accounts.contains(where: \.enabled), windowVisible: windowVisible,
            dashboardSelected: route == "dashboard", scanLocalLogs: managesDashboard && settings.scanLocalLogs,
            scanning: scanning, refreshSeconds: settings.refreshSeconds,
            lastAccountRefresh: lastScheduled, lastLocalAttempt: lastLocalAttempt ?? local.scannedAt)
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
        if let due = schedule.localLogs, due <= now { refreshLocalIfNeeded() }
        scheduleNextRefresh()
        diagnostic("scheduled-refresh")
    }
    private func suspend() {
        sleeping = true; queue.removeAll()
        for (_, task) in jobs.values { task.cancel() }
        jobs.removeAll(); loading.removeAll(); scanTask?.cancel()
        scheduleNextRefresh()
    }
    private func wake() { sleeping = false; refreshAccounts(); if windowVisible { refreshLocalIfNeeded() } }
    func refreshAll() {
        if let remote {
            remote.send(.refreshAll)
            if windowVisible { refreshLocalIfNeeded(force: true) }
            return
        }
        refreshAccounts()
        if windowVisible { refreshLocalIfNeeded(force: true) }
    }
    func refreshAccounts() {
        if let remote { remote.send(.refreshAccounts); return }
        guard !demo, storageHealthy else { return }
        lastScheduled = Date()
        enqueue(enabledAccounts.map(\.id))
        scheduleNextRefresh()
    }
    func refresh(_ account: MonitoredAccount) {
        if let remote { remote.send(.refresh(account.id)); return }
        guard account.enabled, !demo, storageHealthy else { return }
        enqueue([account.id])
    }
    private func enqueue(_ ids: [UUID]) {
        guard !sleeping else { return }
        for id in ids where !loading.contains(id) {
            queue.append(id); loading.insert(id)
        }
        drain()
    }
    private func drain() {
        while jobs.count < 3, !queue.isEmpty {
            let id = queue.removeFirst()
            guard let account = accounts.first(where: { $0.id == id && $0.enabled }) else { loading.remove(id); continue }
            let generation = UUID()
            let task = Task { [weak self, service] in
                let result = await service.fetch(account)
                guard let self, !Task.isCancelled, self.jobs[id]?.0 == generation,
                      self.accounts.contains(where: { $0.id == id && $0.enabled }) else { return }
                // Commit refreshed CLI credentials only after the generation check
                // on the main actor. A deleted account cannot recreate a vault item.
                if let data = result.updatedCredential {
                    do { try self.vault.save(data, id: id) }
                    catch { self.message = error.localizedDescription }
                }
                if let snapshot = result.snapshot { self.snapshots[id] = snapshot }
                if self.errors[id] != result.error { self.errors[id] = result.error }
                if self.notes[id] != result.note { self.notes[id] = result.note }
                self.jobs.removeValue(forKey: id); self.loading.remove(id); self.drain()
            }
            jobs[id] = (generation, task)
        }
    }

    private func save(_ next: LiteSettings) throws {
        guard remote == nil else { throw PipeError.disconnected }
        guard storageHealthy else { throw ProviderError("settings_unavailable", "설정을 복구한 후 다시 시도하세요.") }
        if !demo { try settingsFile.save(next) }
        settings = next
        scheduleNextRefresh()
    }
    func setInterval(_ seconds: Int) {
        if let remote { remote.send(.interval(seconds)); return }
        guard settings.refreshSeconds != seconds else { return }
        var next = settings; next.refreshSeconds = seconds
        do { try save(next) } catch { message = error.localizedDescription }
    }
    func setAppearance(_ mode: AppearanceMode) {
        if let remote { remote.send(.appearance(mode)); return }
        guard settings.selectedAppearance != mode else { return }
        var next = settings; next.appearance = mode
        do {
            try save(next)
            AppTheme.apply(mode)
        } catch { message = error.localizedDescription }
    }
    func setDockIconVisible(_ visible: Bool) {
        if let remote { remote.send(.dockIcon(visible)); return }
        guard settings.isDockIconVisible != visible else { return }
        var next = settings; next.showDockIcon = visible
        do {
            try save(next)
            applyDockIconVisibility()
        } catch { message = error.localizedDescription }
    }
    func applyDockIconVisibility() {
        guard remote == nil else { return }
        let app = NSApplication.shared
        let policy: NSApplication.ActivationPolicy = settings.isDockIconVisible ? .regular : .accessory
        guard app.activationPolicy() != policy else { return }
        let keyWindow = app.keyWindow
        let wasActive = app.isActive
        guard app.setActivationPolicy(policy) else {
            message = "Dock 표시를 변경하지 못했습니다. 앱을 다시 실행해 주세요."
            return
        }
        // Switching policy can temporarily take focus away from the Settings window.
        if wasActive, let keyWindow {
            DispatchQueue.main.async { [weak keyWindow] in
                guard let keyWindow else { return }
                app.activate(ignoringOtherApps: true)
                keyWindow.makeKeyAndOrderFront(nil)
            }
        }
    }
    func setMenuBarDisplayMode(_ mode: MenuBarDisplayMode) {
        if let remote { remote.send(.menuBarMode(mode)); return }
        guard settings.selectedMenuBarDisplayMode != mode else { return }
        var next = settings; next.menuBarDisplayMode = mode
        do { try save(next) } catch { message = error.localizedDescription }
    }
    func setScanLogs(_ value: Bool) {
        if let remote { remote.send(.scanLogs(value)); return }
        guard settings.scanLocalLogs != value else { return }
        var next = settings; next.scanLocalLogs = value
        do {
            try save(next)
            if value { refreshLocalIfNeeded(force: true) }
            else { scanTask?.cancel(); local = LocalUsageReport(); Task { await scanner.releaseCache() } }
        } catch { message = error.localizedDescription }
    }
    func toggle(_ account: MonitoredAccount) {
        if let remote { remote.send(.toggle(account.id)); return }
        var next = settings
        guard let i = next.accounts.firstIndex(where: { $0.id == account.id }) else { return }
        next.accounts[i].enabled.toggle()
        do {
            try save(next)
            if next.accounts[i].enabled { refresh(next.accounts[i]) }
            else { cancel(account.id) }
        } catch { message = error.localizedDescription }
    }
    func setMenuBarVisibility(_ visible: Bool, for account: MonitoredAccount) {
        if let remote { remote.send(.menuBarVisibility(account.id, visible)); return }
        var next = settings
        guard let i = next.accounts.firstIndex(where: { $0.id == account.id }),
              next.accounts[i].isShownInMenuBar != visible else { return }
        next.accounts[i].showInMenuBar = visible
        do { try save(next) } catch { message = error.localizedDescription }
    }
    private func cancel(_ id: UUID) {
        queue.removeAll { $0 == id }; jobs[id]?.1.cancel(); jobs.removeValue(forKey: id); loading.remove(id)
        drain()
    }

    func beginConnection(for account: MonitoredAccount? = nil) {
        loginTarget = account; connectionPresented = true
    }

    func connectFromUI(provider: ProviderKind, path: String, alias: String) async throws {
        if let remote {
            try await remote.request { .connect(id: $0, provider: provider, path: path, alias: alias) }
        } else { try connect(provider: provider, path: path, alias: alias) }
    }

    func connectCodexLoginFromUI(_ data: Data, alias: String, replacing target: MonitoredAccount?) async throws {
        if let remote {
            try await remote.request { .codexLogin(id: $0, data: data, alias: alias, replacing: target?.id) }
        } else { try connectCodexLogin(data, alias: alias, replacing: target) }
    }

    func connectCodexLogin(_ data: Data, alias: String, replacing target: MonitoredAccount?) throws {
        guard remote == nil else { throw PipeError.disconnected }
        guard !demo, storageHealthy else { throw ProviderError("login_unavailable", "현재 상태에서는 계정을 연결할 수 없습니다.") }
        let credentials = try CodexOAuthCredentials(data: data)
        guard credentials.refreshToken != nil else { throw ProviderError("missing_refresh", "갱신 가능한 로그인이 필요합니다. 다시 로그인하세요.") }
        if let target {
            guard accounts.contains(where: { $0.id == target.id }), target.identity == credentials.identity.key else {
                throw ProviderError("wrong_login_account", "선택한 계정과 로그인한 계정이 다릅니다. 기존 계정으로 로그인하거나 새 계정으로 연결하세요.")
            }
        }
        let existing = accounts.first { $0.provider == .codex && $0.identity == credentials.identity.key }
        var account = existing ?? MonitoredAccount(provider: .codex, name: credentials.identity.label,
            identity: credentials.identity.key, sourcePath: "")
        let name = alias.trimmingCharacters(in: .whitespacesAndNewlines)
        if !name.isEmpty { account.name = name }
        account.appManagedLogin = true; account.sourcePath = ""; account.enabled = true
        var next = settings
        if let i = next.accounts.firstIndex(where: { $0.id == account.id }) { next.accounts[i] = account }
        else { next.accounts.append(account) }
        let old = try vault.load(account.id)
        try vault.save(data, id: account.id)
        do { try save(next) }
        catch {
            if let old { try? vault.save(old, id: account.id) } else { try? vault.delete(account.id) }
            throw error
        }
        codexSessions.invalidate(account.id)
        cancel(account.id); errors.removeValue(forKey: account.id); notes.removeValue(forKey: account.id)
        refresh(account)
    }

    func connect(provider: ProviderKind, path: String, alias: String) throws {
        guard remote == nil else { throw PipeError.disconnected }
        guard !demo else { throw ProviderError("demo", "미리보기에서는 계정을 저장하지 않습니다. 실제 앱을 열어 연결하세요.") }
        guard storageHealthy else { throw ProviderError("settings_unavailable", "설정을 먼저 복구하세요.") }
        let url = URL(fileURLWithPath: NSString(string: path.trimmingCharacters(in: .whitespacesAndNewlines)).expandingTildeInPath)
            .standardizedFileURL.resolvingSymlinksInPath()
        let name = alias.trimmingCharacters(in: .whitespacesAndNewlines)
        if provider == .claude { try connectClaude(directory: url, name: name); return }
        let attrs = try FileManager.default.attributesOfItem(atPath: url.path)
        guard (attrs[.size] as? NSNumber)?.intValue ?? Int.max <= 1_048_576 else {
            throw ProviderError("large_auth", "로그인 파일 크기가 너무 큽니다.")
        }
        let data = try Data(contentsOf: url)
        let identity = try ImportedIdentity.parse(data, provider: provider)
        var next = settings
        let existing = accounts.first { $0.provider == provider && $0.identity == identity.key }
        var account = existing ?? MonitoredAccount(provider: provider, name: identity.label, identity: identity.key, sourcePath: url.path)
        account.name = name.isEmpty ? (existing?.name ?? identity.label) : name
        account.sourcePath = url.path; account.enabled = true; account.appManagedLogin = false
        let oldSecret = try vault.load(account.id)
        try vault.save(data, id: account.id)
        if let i = next.accounts.firstIndex(where: { $0.id == account.id }) { next.accounts[i] = account }
        else { next.accounts.append(account) }
        do { try save(next) }
        catch {
            if let oldSecret { try? vault.save(oldSecret, id: account.id) } else { try? vault.delete(account.id) }
            throw error
        }
        codexSessions.invalidate(account.id)
        cancel(account.id); refresh(account)
    }

    private func connectClaude(directory: URL, name: String) throws {
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: directory.path, isDirectory: &isDirectory), isDirectory.boolValue else {
            throw ProviderError("missing_directory", "Claude CLI에 로그인한 뒤 해당 설정 폴더를 선택하세요.")
        }
        guard let helper = Bundle.main.executableURL?.path else { throw ProviderError("helper", "앱 실행 파일을 찾을 수 없습니다.") }
        let profiles = ClaudeSubscriptionStore(root: root.appendingPathComponent("ClaudeProfiles"))
        let connection = ClaudeSubscriptionConnection(store: profiles)
        let key = ClaudeSubscriptionProfile(configDirectory: directory.path, name: "").id
        let previous = try? profiles.loadProfile(id: key)
        let profile = try connection.install(directory: directory.path, name: name.isEmpty ? "Claude profile" : name, helperPath: helper)
        var next = settings
        if let i = next.accounts.firstIndex(where: { $0.provider == .claude && $0.identity == key }) {
            next.accounts[i].name = name.isEmpty ? next.accounts[i].name : name
            next.accounts[i].enabled = true
        } else {
            next.accounts.append(MonitoredAccount(provider: .claude, name: name.isEmpty ? "Claude · \(directory.lastPathComponent)" : name,
                identity: profile.id, sourcePath: directory.path))
        }
        do { try save(next) }
        catch { try? connection.rollbackInstallation(profile, previous: previous); throw error }
        if let account = accounts.first(where: { $0.identity == key }) { cancel(account.id); refresh(account) }
    }

    func remove(_ account: MonitoredAccount) {
        if let remote { remote.send(.remove(account.id)); return }
        do {
            var next = settings; next.accounts.removeAll { $0.id == account.id }
            if demo { try save(next) }
            else {
                guard storageHealthy else { throw ProviderError("settings_unavailable", "설정을 먼저 복구하세요.") }
                if account.provider == .claude {
                    let profiles = ClaudeSubscriptionStore(root: root.appendingPathComponent("ClaudeProfiles"))
                    let connection = ClaudeSubscriptionConnection(store: profiles)
                    let profile = try profiles.loadProfile(id: account.identity)
                    let settingsURL = URL(fileURLWithPath: profile.configDirectory).appendingPathComponent("settings.json")
                    let json = (try? JSONSerialization.jsonObject(with: Data(contentsOf: settingsURL))) as? [String: Any]
                    let command = (json?["statusLine"] as? [String: Any])?["command"] as? String
                    let restored = try connection.uninstall(profileID: account.identity)
                    if !restored, let command, command.contains("--claude-statusline"), command.contains(account.identity) {
                        throw ProviderError("restore_needed", "statusLine이 수정되어 자동 복구하지 못했습니다. 설정에서 해당 연결 명령을 제거한 뒤 다시 삭제하세요.")
                    }
                    try save(next)
                    try? FileManager.default.removeItem(at: profiles.directory(for: account.identity))
                } else {
                    let data = try vault.load(account.id)
                    try vault.delete(account.id)
                    do { try save(next) }
                    catch { if let data { try? vault.save(data, id: account.id) }; throw error }
                }
            }
            codexSessions.invalidate(account.id)
            cancel(account.id); snapshots.removeValue(forKey: account.id); errors.removeValue(forKey: account.id); notes.removeValue(forKey: account.id)
        } catch { message = error.localizedDescription }
    }

    func refreshLocalIfNeeded(force: Bool = false) {
        guard managesDashboard, !demo, !sleeping, storageHealthy, settings.scanLocalLogs, !scanning, windowVisible, route == "dashboard",
              force || Date().timeIntervalSince(lastLocalAttempt ?? local.scannedAt ?? .distantPast) >= RefreshSchedule.localInterval else { return }
        scanning = true
        lastLocalAttempt = Date()
        scheduleNextRefresh()
        diagnostic("scan-start")
        let home = FileManager.default.homeDirectoryForCurrentUser
        let codexHome = URL(fileURLWithPath: ProcessInfo.processInfo.environment["CODEX_HOME"] ?? home.appendingPathComponent(".codex").path)
        let codexRoots = [codexHome.appendingPathComponent("sessions"), codexHome.appendingPathComponent("archived_sessions")]
        var claudeRoots = [URL(fileURLWithPath: ProcessInfo.processInfo.environment["CLAUDE_CONFIG_DIR"] ?? home.appendingPathComponent(".claude").path).appendingPathComponent("projects")]
        claudeRoots += accounts.filter { $0.provider == .claude && $0.enabled }.map { URL(fileURLWithPath: $0.sourcePath).appendingPathComponent("projects") }
        scanTask = Task { [weak self, scanner] in
            defer {
                self?.scanning = false; self?.scanTask = nil
                self?.scheduleNextRefresh(); self?.diagnostic("scan-finished")
            }
            do {
                let report = try await scanner.scan(codexRoots: codexRoots, claudeRoots: claudeRoots,
                    historyDays: UsageActivity.historyDays)
                try Task.checkCancellation()
                self?.local = report; self?.lastLocalAttempt = report.scannedAt
            } catch is CancellationError { self?.lastLocalAttempt = nil }
            catch {
                // Failed scans back off too; an overdue deadline must not spin.
                self?.lastLocalAttempt = Date()
                self?.message = "로컬 사용량을 읽지 못했습니다. 로그 폴더 접근 권한을 확인하세요."
            }
        }
    }

    var dashboardState: DashboardState {
        DashboardState(settings: settings, snapshots: snapshots, errors: errors, notes: notes,
                       loading: loading, message: message, demoReport: demo ? local : nil)
    }

    func apply(_ state: DashboardState) {
        guard remote != nil else { return }
        applyingRemoteState = true
        defer { applyingRemoteState = false }
        let appearanceChanged = settings.selectedAppearance != state.settings.selectedAppearance
        let scanChanged = settings.scanLocalLogs != state.settings.scanLocalLogs
        settings = state.settings; snapshots = state.snapshots; errors = state.errors
        notes = state.notes; loading = state.loading; message = state.message
        if let report = state.demoReport { local = report }
        if appearanceChanged { AppTheme.apply(settings.selectedAppearance) }
        if scanChanged {
            if settings.scanLocalLogs { refreshLocalIfNeeded(force: true) }
            else { scanTask?.cancel(); local = LocalUsageReport(); Task { await scanner.releaseCache() } }
        }
        scheduleNextRefresh()
    }

    /// Explicit local smoke-test diagnostics contain only lifecycle counters.
    func diagnostic(_ event: String, details: [String: Any] = [:]) {
        let args = ProcessInfo.processInfo.arguments
        guard let index = args.firstIndex(of: "--diagnostics"), args.indices.contains(index + 1) else { return }
        let url = URL(fileURLWithPath: args[index + 1])
        var fields: [String: Any] = ["event": event, "date": Date().timeIntervalSince1970,
            "windowVisible": windowVisible, "scanning": scanning, "runningRequests": jobs.count,
            "queuedRequests": queue.count, "demo": demo,
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
