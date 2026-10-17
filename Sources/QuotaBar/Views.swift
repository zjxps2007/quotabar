import SwiftUI
import AppKit
import UsageCore

// Select the back-deployable property wrapper when newer SDKs also export a State macro.
private typealias ViewState<Value> = SwiftUI.State<Value>

extension ProviderKind {
    var color: Color {
        switch self { case .codex: return AppTheme.codexAccent
        case .claude: return AppTheme.claudeAccent
        case .gemini: return AppTheme.geminiAccent }
    }
    var symbol: String {
        switch self { case .codex: return "terminal"; case .claude: return "asterisk"; case .gemini: return "sparkles" }
    }
}

private func compact(_ value: Int) -> String {
    if value >= 1_000_000 { return String(format: "%.1fM", Double(value) / 1_000_000) }
    if value >= 1_000 { return String(format: "%.1fK", Double(value) / 1_000) }
    return "\(value)"
}

struct BrandIcon: View {
    let provider: ProviderKind
    var size: CGFloat = 36
    private var artworkScale: CGFloat {
        switch provider { case .codex: return 1; case .claude: return 0.75; case .gemini: return 0.82 }
    }
    var body: some View {
        Image(nsImage: ProviderAssets.image(for: provider)).resizable().renderingMode(.original)
            .interpolation(.high).scaledToFit()
            .frame(width: size * artworkScale, height: size * artworkScale)
            .frame(width: size, height: size)
            .accessibilityLabel(provider.title)
    }
}

private enum ProviderAssets {
    static let codex = load("CodexLogo", extension: "png", provider: .codex)
    static let claude = load("ClaudeLogo", extension: "svg", provider: .claude)
    static let gemini = load("GeminiLogo", extension: "png", provider: .gemini)
    static func image(for provider: ProviderKind) -> NSImage {
        switch provider { case .codex: return codex; case .claude: return claude; case .gemini: return gemini }
    }
    private static func load(_ name: String, extension ext: String, provider: ProviderKind) -> NSImage {
        guard let url = AppResources.bundle.url(forResource: name, withExtension: ext),
              let image = NSImage(contentsOf: url) else {
            return NSImage(systemSymbolName: provider.symbol, accessibilityDescription: provider.title) ?? NSImage()
        }
        return image
    }
}

struct CardSurface: ViewModifier {
    func body(content: Content) -> some View {
        content.padding(20).background(AppTheme.cardBackground, in: RoundedRectangle(cornerRadius: 16))
            .overlay(RoundedRectangle(cornerRadius: 16).stroke(AppTheme.cardBorder, lineWidth: 1))
    }
}
extension View { func card() -> some View { modifier(CardSurface()) } }

struct RootView: View {
    @EnvironmentObject var store: MonitorStore
    var body: some View {
        HStack(spacing: 0) {
            sidebar
            Divider()
            VStack(spacing: 0) {
                header
                if store.demo {
                    HStack(spacing: 7) {
                        Image(systemName: "eye")
                        Text("미리보기 · 샘플 데이터입니다. 실제 계정이나 로그인 파일에 접근하지 않습니다.")
                        Spacer()
                    }.font(.caption).foregroundStyle(.secondary).padding(.horizontal, 28).padding(.vertical, 9)
                    .background(AppTheme.trackBackground)
                }
                if let message = store.message {
                    HStack {
                        Image(systemName: "exclamationmark.triangle")
                        Text(message).textSelection(.enabled)
                        Spacer()
                        Button { store.message = nil } label: { Image(systemName: "xmark") }.buttonStyle(.plain)
                    }.font(.callout).foregroundStyle(.orange).padding(14).background(Color.orange.opacity(0.08))
                }
                Group {
                    switch store.route {
                    case "accounts": AccountsView()
                    case "settings": PreferencesView()
                    default: DashboardView()
                    }
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
            }.background(AppTheme.background)
        }
        .sheet(isPresented: $store.connectionPresented, onDismiss: { store.loginTarget = nil }) { ConnectView().environmentObject(store) }
        .tint(AppTheme.accent)
    }
    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 30) {
            HStack(spacing: 10) {
                AppLogo()
                Text("QuotaBar").font(.system(size: 18, weight: .bold))
            }.padding(.top, 34)
            VStack(alignment: .leading, spacing: 5) {
                Text("WORKSPACE").font(.system(size: 9, weight: .semibold)).tracking(1.5).foregroundStyle(.tertiary).padding(.leading, 12).padding(.bottom, 7)
                nav("dashboard", "Dashboard", "square.grid.2x2")
                nav("accounts", "Providers & Accounts", "person.2")
                nav("settings", "설정", "slider.horizontal.3")
            }
            Spacer()
        }.padding(.horizontal, 18).padding(.bottom, 20).frame(width: 210)
            .background(AppTheme.sidebarBackground)
    }
    private func nav(_ id: String, _ title: String, _ icon: String) -> some View {
        Button { store.route = id } label: {
            HStack(spacing: 9) { Image(systemName: icon).frame(width: 18); Text(title); Spacer() }
                .font(.system(size: 12, weight: store.route == id ? .semibold : .regular))
                .padding(.horizontal, 12).padding(.vertical, 11).contentShape(Rectangle())
                .background(store.route == id ? AppTheme.accent : .clear, in: RoundedRectangle(cornerRadius: 9))
                .foregroundStyle(store.route == id ? AppTheme.onAccent : .primary)
        }.buttonStyle(.plain)
    }
    private var header: some View {
        HStack(alignment: .center) {
            VStack(alignment: .leading, spacing: 5) {
                Text(store.route == "accounts" ? "Providers & Accounts" : store.route == "settings" ? "설정" : "Dashboard")
                    .font(.system(size: 25, weight: .bold))
                Text(store.route == "accounts" ? "계정 연결과 로그인, 모니터링 상태를 관리하세요." : store.route == "settings" ? "화면 모드, Dock과 상단바 표시, 조회 방식을 설정하세요." : "남은 사용량과 활동을 확인하세요.")
                    .font(.system(size: 12)).foregroundStyle(.secondary)
            }
            Spacer()
            if store.route == "accounts" {
                Button { store.beginConnection() } label: { Label("계정 연결", systemImage: "plus") }
                    .buttonStyle(AppPrimaryButtonStyle())
            } else if store.route != "settings" {
                if store.busy { ProgressView().controlSize(.small).help("사용량 조회 중") }
                Button { store.refreshAll() } label: { Label("새로고침", systemImage: "arrow.clockwise") }
                    .help("새로고침 (⌘R)")
            }
        }.padding(.horizontal, 28).padding(.top, 32).padding(.bottom, 22)
    }
}

struct DashboardView: View {
    @EnvironmentObject var store: MonitorStore
    @ViewState private var activitySelection: ActivitySelection?
    private var activityProviders: [ProviderKind] {
        let registered = Set(store.accounts.map(\.provider))
        return [.claude, .codex].filter { registered.contains($0) }
    }
    var body: some View {
        GeometryReader { geometry in
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    HStack(spacing: 12) {
                        StatTile(title: "모니터링 중", value: "\(store.enabledAccounts.count)", subtitle: "\(Set(store.enabledAccounts.map(\.provider)).count)개 Provider", icon: "person.2", color: AppTheme.accent)
                        StatTile(title: "확인 필요", value: "\(store.attentionCount)", subtitle: "잔여량 20% 이하 · 연결 상태", icon: "exclamationmark.circle", color: store.attentionCount > 0 ? .orange : AppTheme.accent)
                        StatTile(title: "오늘의 토큰", value: tokenValue(month: false), subtitle: "이 Mac의 로컬 기록", icon: "bolt", color: AppTheme.accent)
                        StatTile(title: "이번 달 토큰", value: tokenValue(month: true), subtitle: Date().formatted(.dateTime.month(.wide)), icon: "chart.bar", color: AppTheme.accent)
                    }
                    if store.accounts.isEmpty {
                        VStack(spacing: 15) {
                            HStack(spacing: 10) { ForEach(ProviderKind.allCases) { BrandIcon(provider: $0, size: 44) } }
                            Text("첫 번째 계정을 연결하세요").font(.title3.bold())
                            Text("Codex, Claude, Gemini의 잔여량을 나란히 확인할 수 있습니다.")
                                .foregroundStyle(.secondary)
                            Button("계정 관리로 이동") { store.route = "accounts" }
                                .buttonStyle(AppPrimaryButtonStyle())
                        }.padding(.vertical, 28).frame(maxWidth: .infinity).card()
                    } else {
                        HStack { sectionTitle("구독 현황", "계정별 잔여량 · 상단바에 표시할 계정을 선택하세요"); Spacer()
                            Button("계정 관리 →") { store.route = "accounts" }.buttonStyle(.plain).foregroundStyle(AppTheme.accent)
                        }
                        LazyVGrid(columns: [GridItem(.adaptive(minimum: 290), spacing: 14)], spacing: 14) {
                            ForEach(store.accounts) { account in UsageAccountCard(account: account) }
                        }
                    }
                    if !activityProviders.isEmpty { activitySection }
                }.padding(.horizontal, 28).padding(.bottom, 28)
            }
            .overlay(alignment: .topLeading) {
                if let activitySelection {
                    ActivityTooltipLayer(selection: activitySelection, container: geometry.size)
                }
            }
            .coordinateSpace(name: "activity-tooltip-root")
        }
        .onChange(of: activityProviders) { _, providers in
            if let activitySelection, !providers.contains(activitySelection.provider) { self.activitySelection = nil }
        }
    }
    private var activitySection: some View {
        Group {
            HStack {
                sectionTitle("사용 활동", "최근 26주 · 이 Mac의 \(activityProviders.map(\.title).joined(separator: " / ")) 로그")
                Spacer()
                if store.scanning { ProgressView().controlSize(.small) }
            }
            if !store.settings.scanLocalLogs {
                Label("로컬 통계가 꺼져 있습니다. 설정에서 켤 수 있습니다.", systemImage: "pause.circle").foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading).card()
            } else if store.local.scannedAt == nil {
                Label(store.scanning ? "로컬 기록을 읽고 있습니다…" : "로컬 사용 기록을 확인하세요.", systemImage: "clock").foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading).card()
            } else {
                ActivityView(report: store.local, providers: activityProviders) { provider, selection in
                    if let selection { activitySelection = selection }
                    else if activitySelection?.provider == provider { activitySelection = nil }
                }.equatable()
                ForEach(store.local.warnings, id: \.self) { Text($0).font(.caption).foregroundStyle(.orange) }
                HStack(spacing: 5) {
                    Image(systemName: "info.circle")
                    Text("토큰은 계정별 할당이 아닌 기기 전체 기록입니다. 구독 요금이나 API 청구 금액으로 환산하지 않습니다.")
                }.font(.caption).foregroundStyle(.secondary)
            }
        }
    }
    private func tokenValue(month: Bool) -> String {
        guard store.settings.scanLocalLogs, store.local.scannedAt != nil else { return "—" }
        return compact(month ? store.local.tokens(inMonth: DateFormat.string(from: Date(), format: "yyyy-MM"))
            : store.local.tokens(on: DateFormat.string(from: Date(), format: "yyyy-MM-dd")))
    }
    private func sectionTitle(_ title: String, _ subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 4) { Text(title).font(.system(size: 15, weight: .semibold)); Text(subtitle).font(.caption).foregroundStyle(.secondary) }
    }
}

struct StatTile: View {
    let title: String; let value: String; let subtitle: String; let icon: String; let color: Color
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack { Text(title).font(.system(size: 11)).foregroundStyle(.secondary); Spacer(); Image(systemName: icon).foregroundStyle(color) }
            Text(value).font(.system(size: 28, weight: .semibold, design: .rounded)).monospacedDigit().lineLimit(1).minimumScaleFactor(0.7)
            Text(subtitle).font(.system(size: 10)).foregroundStyle(.tertiary).lineLimit(1).minimumScaleFactor(0.7)
        }.frame(maxWidth: .infinity, alignment: .leading).card()
    }
}

struct UsageAccountCard: View {
    @EnvironmentObject var store: MonitorStore
    let account: MonitoredAccount
    private var snapshot: AccountSnapshot? { store.snapshots[account.id] }
    var body: some View {
        VStack(alignment: .leading, spacing: 17) {
            HStack(spacing: 10) {
                BrandIcon(provider: account.provider)
                VStack(alignment: .leading, spacing: 3) {
                    HStack { Text(account.provider.title).font(.system(size: 13, weight: .semibold))
                        if let plan = snapshot?.plan { Text(plan).font(.system(size: 9, weight: .medium)).padding(.horizontal, 6).padding(.vertical, 3).background(Color.primary.opacity(0.05), in: Capsule()) }
                    }
                    Text(account.name).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(1).help(account.name)
                }
                Spacer(minLength: 2)
                if store.loading.contains(account.id) { ProgressView().controlSize(.small) }
            }
            if !account.enabled {
                Label("모니터링 일시 정지", systemImage: "pause.circle").foregroundStyle(.secondary).font(.callout)
            } else if let snapshot, !snapshot.windows.isEmpty {
                ForEach(snapshot.windows) { window in
                    QuotaRow(window: window, color: account.provider.color, stale: store.errors[account.id] != nil || snapshot.state == "stale")
                }
            } else {
                Label(stateText, systemImage: "clock").font(.callout).foregroundStyle(.secondary).padding(.vertical, 9)
            }
            if store.errors[account.id] != nil {
                VStack(alignment: .leading, spacing: 6) {
                    Label("최신 사용량을 확인하지 못했습니다.", systemImage: "exclamationmark.circle").foregroundStyle(.orange)
                    Button("계정 관리에서 확인 →") { store.route = "accounts" }.buttonStyle(.link)
                }.font(.caption)
            } else if snapshot?.state == "stale" {
                Text("이전 스냅샷 · Claude에서 메시지를 보내면 갱신됩니다.").font(.caption).foregroundStyle(.orange)
            }
            if let snapshot {
                HStack(spacing: 4) {
                    Circle().fill(store.errors[account.id] == nil ? account.provider.color : .orange).frame(width: 4, height: 4)
                    if let observed = snapshot.observedAt { Text("마지막 데이터 \(observed.formatted(date: .omitted, time: .shortened))") }
                    else { Text("스냅샷 대기 중") }
                    Spacer()
                    if store.errors[account.id] != nil { Text("이전 값") }
                }.font(.system(size: 10)).foregroundStyle(.tertiary)
            }
            Divider()
            Toggle("상단바에 표시", isOn: Binding(
                get: { account.isShownInMenuBar },
                set: { store.setMenuBarVisibility($0, for: account) }
            ))
            .toggleStyle(.switch).controlSize(.mini).font(.caption)
            .accessibilityLabel("\(account.provider.title)(\(account.name)) 상단바에 표시")
            .help(account.enabled ? "상단바 요약과 팝업에 표시합니다. 꺼도 사용량 조회는 계속됩니다."
                  : "모니터링을 재개하면 이 설정에 따라 상단바에 표시됩니다.")
        }.frame(maxWidth: .infinity, alignment: .leading).card()
            .tint(account.provider.color)
            .opacity(account.enabled ? 1 : 0.65)
    }
    private var stateText: String {
        switch snapshot?.state {
        case "api-routed": return "API 연결 세션은 구독 quota를 제공하지 않습니다."
        case "disconnected": return "Claude 연결이 해제되었습니다. 다시 연결하세요."
        case "waiting": return "Claude에서 첫 메시지를 보내면 잔여량이 표시됩니다."
        default: return store.loading.contains(account.id) ? "사용량을 확인하고 있습니다…" : "아직 조회된 잔여량이 없습니다."
        }
    }
}

struct AccountConnectionCard: View {
    @EnvironmentObject var store: MonitorStore
    let account: MonitoredAccount
    @ViewState private var removing = false
    private var snapshot: AccountSnapshot? { store.snapshots[account.id] }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 12) {
                BrandIcon(provider: account.provider)
                VStack(alignment: .leading, spacing: 4) {
                    Text(account.provider.title).font(.system(size: 13, weight: .semibold))
                    Text(account.name).font(.callout).lineLimit(1).help(account.name).textSelection(.enabled)
                    if let email = snapshot?.email,
                       email.caseInsensitiveCompare(account.name.trimmingCharacters(in: .whitespacesAndNewlines)) != .orderedSame {
                        Text(email).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
                    }
                }
                Spacer(minLength: 12)
                HStack(spacing: 6) {
                    if store.loading.contains(account.id) { ProgressView().controlSize(.mini) }
                    else { Circle().fill(statusColor).frame(width: 6, height: 6) }
                    Text(statusTitle).font(.caption).foregroundStyle(statusColor)
                }
                Menu {
                    Button("새로고침") { store.refresh(account) }.disabled(!account.enabled)
                    Divider()
                    Button("계정 삭제…", role: .destructive) { removing = true }
                } label: { Image(systemName: "ellipsis") }
                    .menuStyle(.borderlessButton).fixedSize().accessibilityLabel("\(account.name) 계정 메뉴")
            }
            if let message = connectionMessage {
                Label(message, systemImage: "exclamationmark.circle")
                    .font(.caption).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true)
            }
            if let note = store.notes[account.id] {
                Text(note).font(.caption).foregroundStyle(.secondary)
            }
            HStack(spacing: 16) {
                Label(connectionTitle, systemImage: account.usesAppLogin ? "arrow.triangle.2.circlepath" : account.provider == .claude ? "folder" : "doc")
                    .font(.caption).foregroundStyle(.secondary)
                Spacer()
                if account.provider == .codex {
                    Button(account.usesAppLogin ? "다시 로그인…" : "앱에서 로그인…") { store.beginConnection(for: account) }
                        .controlSize(.small).disabled(store.demo)
                }
                Toggle("모니터링", isOn: Binding(
                    get: { account.enabled },
                    set: { if $0 != account.enabled { store.toggle(account) } }
                ))
                .toggleStyle(.switch).controlSize(.mini).font(.caption)
                .accessibilityLabel("\(account.provider.title)(\(account.name)) 모니터링")
                .help("끄면 이 계정의 조회를 일시 정지합니다.")
            }
            if !account.sourcePath.isEmpty {
                DisclosureGroup(account.provider == .claude ? "연결한 설정 폴더" : "연결한 로그인 파일") {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(account.sourcePath).font(.system(size: 11, design: .monospaced)).textSelection(.enabled)
                        Text(account.provider == .claude
                             ? "Claude에서 메시지를 보내면 잔여량이 갱신됩니다. 계정 삭제 시 기존 statusLine 명령을 복구합니다."
                             : "CLI가 갱신한 로그인 파일을 읽습니다. 로그인 만료 시 CLI에서 다시 로그인하세요.")
                            .font(.caption).foregroundStyle(.secondary)
                    }.frame(maxWidth: .infinity, alignment: .leading).padding(.top, 8)
                }.font(.caption).foregroundStyle(.secondary)
            }
        }.frame(maxWidth: .infinity, alignment: .leading).card()
        .tint(account.provider.color)
        .alert("\(account.name) 계정을 삭제할까요?", isPresented: $removing) {
            Button("취소", role: .cancel) {}
            Button("삭제", role: .destructive) { store.remove(account) }
        } message: {
            Text(account.provider == .claude ? "이 앱의 프로필과 스냅샷을 지우고 연결한 statusLine을 복구합니다. CLI 로그인은 유지됩니다."
                : "이 앱에 저장한 로그인과 모니터링 정보를 삭제합니다. 원본 CLI 로그인 파일은 유지됩니다.")
        }
    }

    private var connectionTitle: String {
        if account.usesAppLogin { return "앱 로그인 · 자동 갱신" }
        return account.provider == .claude ? "Claude CLI 연결" : "로그인 파일 연결"
    }
    private var statusTitle: String {
        if !account.enabled { return "일시 정지" }
        if store.loading.contains(account.id) { return "확인 중" }
        if store.needsConnectionAttention(account) { return "확인 필요" }
        return snapshot == nil ? "조회 대기" : "연결됨"
    }
    private var statusColor: Color {
        if !account.enabled || store.loading.contains(account.id) { return .secondary }
        if store.needsConnectionAttention(account) { return .orange }
        return snapshot == nil ? .secondary : account.provider.color
    }
    private var connectionMessage: String? {
        if let error = store.errors[account.id] { return error }
        switch snapshot?.state {
        case "api-routed": return "API 연결 세션은 구독 잔여량을 제공하지 않습니다."
        case "disconnected": return "Claude 연결이 해제되었습니다. 계정 연결에서 설정 폴더를 다시 연결하세요."
        case "waiting": return "Claude에서 첫 메시지를 보내면 데이터를 받을 수 있습니다."
        case "stale": return "새 데이터가 없습니다. Claude에서 메시지를 보내 연결을 확인하세요."
        default: return nil
        }
    }
}

struct QuotaRow: View {
    let window: QuotaWindow
    let color: Color
    var stale = false
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text(window.title).font(.system(size: 11)).foregroundStyle(.secondary)
                Spacer()
                Text("\(Int(window.remaining.rounded()))% 남음").font(.system(size: 11, weight: .semibold)).monospacedDigit()
                    .foregroundStyle(window.remaining <= 20 ? .orange : .primary)
            }
            GeometryReader { geometry in
                ZStack(alignment: .leading) {
                    Capsule().fill(AppTheme.trackBackground)
                    Capsule().fill(window.remaining <= 20 ? Color.orange : color)
                        .frame(width: geometry.size.width * window.remaining / 100).opacity(stale ? 0.45 : 1)
                }
            }.frame(height: 5).accessibilityLabel("\(window.title) 잔여량 \(Int(window.remaining)) 퍼센트")
            if let reset = window.resetAt {
                Text(reset > Date() ? "리셋 \(reset.formatted(date: .abbreviated, time: .shortened))" : "리셋 시간 경과 · 새로고침 필요")
                    .font(.system(size: 9)).foregroundStyle(.tertiary)
            }
        }
    }
}

struct AccountsView: View {
    @EnvironmentObject var store: MonitorStore
    @ViewState private var query = ""
    @ViewState private var provider = "all"
    @ViewState private var attentionOnly = false
    private var matches: [MonitoredAccount] {
        store.accounts.filter { (provider == "all" || $0.provider.rawValue == provider)
            && (query.isEmpty || "\($0.name) \($0.provider.title) \(store.snapshots[$0.id]?.email ?? "")".localizedCaseInsensitiveContains(query))
            && (!attentionOnly || store.needsConnectionAttention($0)) }
    }
    var body: some View {
        VStack(spacing: 18) {
            HStack(spacing: 12) {
                TextField("계정 이름 또는 이메일 검색", text: $query).textFieldStyle(.roundedBorder).frame(maxWidth: 280)
                Picker("Provider", selection: $provider) {
                    Text("전체 Provider").tag("all")
                    ForEach(ProviderKind.allCases) { Text($0.title).tag($0.rawValue) }
                }.labelsHidden().frame(width: 150)
                Spacer()
                Toggle("연결 확인 필요", isOn: $attentionOnly).toggleStyle(.checkbox)
                Text("\(matches.count) 계정").font(.caption).foregroundStyle(.secondary)
            }
            if matches.isEmpty {
                ContentUnavailableView {
                    Label(store.accounts.isEmpty ? "연결된 계정이 없습니다" : "조건에 맞는 계정이 없습니다", systemImage: "person.2")
                } description: {
                    Text(store.accounts.isEmpty ? "오른쪽 위의 계정 연결에서 Codex, Claude, Gemini를 추가하세요." : "다른 검색어나 Provider를 선택하세요.")
                } actions: {
                    if !store.accounts.isEmpty {
                        Button("필터 초기화") { query = ""; provider = "all"; attentionOnly = false }
                    }
                }
            } else {
                ScrollView {
                    LazyVStack(spacing: 14) {
                        ForEach(matches) { AccountConnectionCard(account: $0) }
                    }.padding(.bottom, 24)
                }
            }
            Spacer(minLength: 0)
        }.padding(.horizontal, 28)
    }
}

struct ConnectView: View {
    @EnvironmentObject var store: MonitorStore
    @Environment(\.dismiss) private var dismiss
    @ViewState private var provider = ProviderKind.codex
    @ViewState private var alias = ""
    @ViewState private var path = ProviderKind.codex.defaultPath
    @ViewState private var error: String?
    @ViewState private var agrees = false
    @ViewState private var method = "browser"
    @ViewState private var loginTask: Task<Void, Never>?
    @ViewState private var attempt: UUID?
    @ViewState private var loginURL: URL?
    @ViewState private var loginStatus = ""
    private var browserLogin: Bool { provider == .codex && method == "browser" }
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack { Text(store.loginTarget == nil ? "계정 연결" : "Codex 다시 로그인").font(.title2.bold()); Spacer()
                Button { cancelLogin(); dismiss() } label: { Image(systemName: "xmark") }.buttonStyle(.plain)
            }
            Text(browserLogin ? "브라우저에서 로그인하면 계정 연결과 토큰 갱신을 앱이 관리합니다." : "CLI에 로그인한 계정을 가져와 모니터링합니다.").foregroundStyle(.secondary)
            Picker("Provider", selection: $provider) { ForEach(ProviderKind.allCases) { Text($0.title).tag($0) } }
                .pickerStyle(.segmented).disabled(loginTask != nil || store.loginTarget != nil)
            if provider == .codex, store.loginTarget == nil {
                Picker("연결 방법", selection: $method) {
                    Text("브라우저 로그인").tag("browser")
                    Text("파일 가져오기").tag("file")
                }.pickerStyle(.segmented).disabled(loginTask != nil)
            }
            VStack(alignment: .leading, spacing: 8) {
                Text("계정 이름").font(.caption.bold())
                TextField("예: 개인 계정, 연구실", text: $alias).textFieldStyle(.roundedBorder).disabled(loginTask != nil)
            }
            if !browserLogin {
                VStack(alignment: .leading, spacing: 8) {
                    Text(provider == .claude ? "Claude 설정 폴더" : "로그인 JSON 파일").font(.caption.bold())
                    HStack { TextField("경로", text: $path).textFieldStyle(.roundedBorder); Button("찾아보기…", action: choosePath) }
                }
            }
            VStack(alignment: .leading, spacing: 10) {
                if browserLogin {
                    Text("ChatGPT 계정으로 로그인하세요. 인증 정보는 이 Mac의 사용자 전용 파일에 저장하고, 만료 시 자동 갱신합니다.")
                    Text("키체인 승인창은 사용하지 않습니다. 로그인 파일은 별도 암호화 없이 파일 접근 권한으로 보호됩니다.").foregroundStyle(.secondary)
                    Text("각 계정을 별도로 연결할 수 있습니다. 기존 Codex CLI의 로그인 상태는 그대로 유지됩니다.").foregroundStyle(.secondary)
                    if store.loginTarget != nil {
                        Text("선택한 카드와 같은 계정·워크스페이스로 로그인하세요. 연결이 완료되면 자동 갱신으로 전환됩니다.").foregroundStyle(.secondary)
                    }
                    if loginTask != nil {
                        HStack { ProgressView().controlSize(.small); Text(loginStatus) }
                        if let loginURL { Button("로그인 페이지 다시 열기") { NSWorkspace.shared.open(loginURL) } }
                    }
                } else if provider == .claude {
                    Text("공식 CLI의 statusLine에서 5시간·주간 한도를 받습니다. 연결 후 Claude에서 메시지를 보내면 표시됩니다. 계정마다 별도 설정 폴더를 사용하세요.")
                    Toggle("이 폴더의 statusLine 연결 허용 (기존 명령은 보존)", isOn: $agrees).toggleStyle(.checkbox)
                } else {
                    Text("로그인은 이 Mac의 사용자 전용 파일에 저장합니다. 파일 연결은 CLI가 갱신한 인증을 읽습니다. Codex 자동 갱신은 브라우저 로그인에서 사용할 수 있습니다.")
                    if provider == .gemini {
                        Text("Code Assist Standard · Enterprise 전용. 개인용 Google 로그인은 2026년 6월 18일에 종료되었습니다.").foregroundStyle(.orange)
                        Link(destination: URL(string: "https://developers.google.com/gemini-code-assist/docs/deprecations/code-assist-individuals")!) {
                            Text("Google 지원 범위 확인").foregroundStyle(AppTheme.accent).underline()
                        }.buttonStyle(.plain)
                    }
                    Text(provider == .codex ? "먼저 터미널에서 codex login을 실행하세요." : "먼저 Gemini CLI에서 지원되는 계정으로 로그인하세요.").foregroundStyle(.secondary)
                }
            }.font(.callout).lineSpacing(3).padding(15).background(Color.primary.opacity(0.035), in: RoundedRectangle(cornerRadius: 10))
            if let error { Text(error).font(.callout).foregroundStyle(.red).textSelection(.enabled) }
            HStack { Spacer(); Button("취소") { cancelLogin(); dismiss() }.keyboardShortcut(.cancelAction)
                Button(browserLogin ? "ChatGPT로 로그인" : "연결") {
                    if browserLogin { startLogin() }
                    else {
                        loginTask = Task { @MainActor in
                            defer { loginTask = nil }
                            do { try await store.connectFromUI(provider: provider, path: path, alias: alias); dismiss() }
                            catch is CancellationError {}
                            catch { self.error = error.localizedDescription }
                        }
                    }
                }.buttonStyle(AppPrimaryButtonStyle()).keyboardShortcut(.defaultAction)
                    .disabled((!browserLogin && path.isEmpty) || (provider == .claude && !agrees) || store.demo || loginTask != nil)
            }
        }.padding(28).frame(width: 550).background(AppTheme.cardBackground).tint(AppTheme.accent)
        .onChange(of: provider) { _, new in path = new.defaultPath; agrees = false; error = nil }
        .onChange(of: method) { _, _ in error = nil }
        .onAppear { if let target = store.loginTarget { alias = target.name } }
        .onDisappear { cancelLogin() }
        .interactiveDismissDisabled(loginTask != nil)
    }
    private func startLogin() {
        let id = UUID(); attempt = id; error = nil; loginURL = nil
        loginStatus = "로그인을 시작하고 있습니다…"
        let name = alias; let target = store.loginTarget
        loginTask = Task { @MainActor in
            defer { if attempt == id { loginTask = nil; attempt = nil; loginURL = nil } }
            do {
                let data = try await CodexLoginSession().login { url in
                    guard attempt == id else { return }
                    loginURL = url; loginStatus = "브라우저에서 로그인을 완료하세요."
                }
                guard attempt == id, !Task.isCancelled else { return }
                try await store.connectCodexLoginFromUI(data, alias: name, replacing: target)
                dismiss()
            } catch is CancellationError {}
            catch {
                guard attempt == id else { return }
                self.error = (error as? ProviderError)?.message ?? "로그인을 완료하지 못했습니다. 다시 시도하세요."
            }
        }
    }
    private func cancelLogin() {
        attempt = nil; loginTask?.cancel(); loginTask = nil; loginURL = nil
    }
    private func choosePath() {
        let panel = NSOpenPanel(); panel.canChooseDirectories = provider == .claude; panel.canChooseFiles = provider != .claude
        panel.allowsMultipleSelection = false; panel.showsHiddenFiles = true
        panel.directoryURL = URL(fileURLWithPath: NSString(string: path).expandingTildeInPath).deletingLastPathComponent()
        if panel.runModal() == .OK, let url = panel.url { path = url.path }
    }
}

struct PreferencesView: View {
    @EnvironmentObject var store: MonitorStore
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                VStack(alignment: .leading, spacing: 14) {
                    Label("화면 모드", systemImage: "circle.lefthalf.filled").font(.headline)
                    Picker("화면 모드", selection: Binding(get: { store.settings.selectedAppearance }, set: store.setAppearance)) {
                        Text("시스템 설정 따르기").tag(AppearanceMode.system)
                        Text("라이트").tag(AppearanceMode.light)
                        Text("다크").tag(AppearanceMode.dark)
                    }.pickerStyle(.segmented).labelsHidden().frame(maxWidth: 420)
                    Text("앱 창과 상단바 팝업에 바로 적용되며, 다음 실행에도 선택한 모드를 유지합니다.")
                        .font(.caption).foregroundStyle(.secondary)
                }.frame(maxWidth: .infinity, alignment: .leading).card()
                VStack(alignment: .leading, spacing: 18) {
                    Label("Dock 및 상단바", systemImage: "menubar.dock.rectangle").font(.headline)
                    Toggle(isOn: Binding(get: { store.settings.isDockIconVisible }, set: store.setDockIconVisible)) {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Dock에 앱 아이콘 표시")
                            Text("끄면 상단바에서 Dashboard와 설정을 열 수 있습니다.")
                                .font(.caption).foregroundStyle(.secondary)
                        }.frame(maxWidth: .infinity, alignment: .leading)
                    }.toggleStyle(.switch)
                        .accessibilityLabel("Dock에 앱 아이콘 표시")
                        .accessibilityHint("끄면 상단바에서 Dashboard와 설정을 열 수 있습니다.")
                    Divider()
                    VStack(alignment: .leading, spacing: 12) {
                        Text("상단바 표시 형식")
                        Picker("상단바 표시 형식", selection: Binding(get: { store.settings.selectedMenuBarDisplayMode }, set: store.setMenuBarDisplayMode)) {
                            Text("이름 + 잔여량").tag(MenuBarDisplayMode.providerAndRemaining)
                            Text("잔여량만").tag(MenuBarDisplayMode.remainingOnly)
                            Text("아이콘만").tag(MenuBarDisplayMode.iconOnly)
                        }.pickerStyle(.segmented).labelsHidden().frame(maxWidth: 420)
                        HStack(spacing: 12) {
                            Text("미리보기").font(.caption).foregroundStyle(.secondary)
                            MenuBarLabel(store: store)
                                .font(.system(size: NSFont.menuBarFont(ofSize: 0).pointSize))
                                .padding(.horizontal, 12).padding(.vertical, 7)
                                .background(AppTheme.background, in: RoundedRectangle(cornerRadius: 7))
                                .accessibilityElement(children: .ignore)
                                .accessibilityLabel("상단바 미리보기")
                                .accessibilityValue(store.settings.selectedMenuBarDisplayMode == .iconOnly ? "아이콘만" : store.settings.selectedMenuBarDisplayMode == .remainingOnly ? store.menuBarRemaining : store.menuBarTitle)
                        }
                        Text("잔여량은 Codex · Claude · Gemini 순서로 표시하며, 등록한 계정만 포함합니다. 표시할 계정은 Dashboard에서 선택하세요.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }.frame(maxWidth: .infinity, alignment: .leading).card()
                VStack(alignment: .leading, spacing: 18) {
                    Label("모니터링", systemImage: "arrow.triangle.2.circlepath").font(.headline)
                    HStack {
                        VStack(alignment: .leading, spacing: 4) { Text("자동 새로고침"); Text("창을 닫으면 최소 15분 간격으로 조회합니다.").font(.caption).foregroundStyle(.secondary) }
                        Spacer()
                        Picker("조회 간격", selection: Binding(get: { store.settings.refreshSeconds }, set: store.setInterval)) {
                            Text("1분").tag(60); Text("5분").tag(300); Text("15분").tag(900); Text("30분").tag(1800)
                        }.labelsHidden().frame(width: 130)
                    }
                    Divider()
                    Toggle(isOn: Binding(get: { store.settings.scanLocalLogs }, set: store.setScanLogs)) {
                        VStack(alignment: .leading, spacing: 4) { Text("로컬 토큰 통계"); Text("Dashboard가 열려 있을 때만 로그를 확인합니다. 대화 본문은 저장하지 않습니다.").font(.caption).foregroundStyle(.secondary) }
                    }.toggleStyle(.switch)
                }.frame(maxWidth: .infinity, alignment: .leading).card()
                VStack(alignment: .leading, spacing: 12) {
                    Label("데이터 저장", systemImage: "lock.shield").font(.headline)
                    Text("Codex · Gemini 로그인은 이 Mac의 사용자 전용 파일에 저장됩니다. 키체인 승인창을 사용하지 않으며, 별도 암호화 대신 파일 접근 권한으로 보호됩니다.")
                    Text("설정 위치").font(.caption).foregroundStyle(.secondary)
                    Text(store.root.path).font(.system(size: 11, design: .monospaced)).textSelection(.enabled)
                    Button("데이터 폴더 열기") { NSWorkspace.shared.open(store.root) }.disabled(store.demo)
                }.font(.callout).frame(maxWidth: .infinity, alignment: .leading).card()
                VStack(alignment: .leading, spacing: 12) {
                    HStack {
                        AppLogo(size: 24)
                        Text("QuotaBar").font(.headline)
                        Spacer()
                        Text("버전 \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—")")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                    Text("sylearn/AIUsage의 조회 코드와 Claude 연결 기능을 바탕으로 만든 독립 경량 버전입니다. 업데이트는 새 앱으로 교체하는 방식입니다.").font(.callout)
                    Link("QuotaBar · GitHub", destination: URL(string: "https://github.com/zjxps2007/quotabar")!)
                        .buttonStyle(.plain).foregroundStyle(AppTheme.accent)
                    Link(destination: URL(string: "https://github.com/sylearn/AIUsage")!) {
                        Text("원본 프로젝트 · Apache 2.0").foregroundStyle(AppTheme.accent).underline()
                    }.buttonStyle(.plain)
                }.frame(maxWidth: .infinity, alignment: .leading).card()
            }.padding(.horizontal, 28).padding(.bottom, 28)
        }
    }
}

struct MenuPanel: View {
    @EnvironmentObject var store: MonitorStore
    let openDashboard: (String) -> Void
    @ViewState private var accountListHeight: CGFloat = 1

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 8) { AppLogo(size: 24); Text("QuotaBar").font(.headline); Spacer()
                Button { store.refreshAccounts() } label: { Image(systemName: "arrow.clockwise") }.buttonStyle(.plain).help("새로고침")
            }
            if store.demo { Text("미리보기 · 샘플 데이터").font(.caption).foregroundStyle(.secondary) }
            if !store.menuBarAccounts.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("남은 사용량").font(.caption.weight(.semibold))
                    Text("상단바는 표시한 계정·기간 중 Provider별 가장 낮은 잔여율입니다. *는 최신 값 확인이 필요하다는 뜻입니다.")
                        .font(.caption2).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                }
            }
            if store.menuBarAccounts.isEmpty {
                Text(store.menuBarEmptyMessage).font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !store.menuBarAccounts.isEmpty {
                ScrollView {
                    VStack(spacing: 14) {
                        ForEach(store.menuBarAccounts) { account in
                            VStack(alignment: .leading, spacing: 8) {
                                HStack { BrandIcon(provider: account.provider, size: 24)
                                    Text("\(account.provider.title)(\(account.name))")
                                        .font(.system(size: 12, weight: .medium))
                                        .fixedSize(horizontal: false, vertical: true)
                                    Spacer()
                                    if store.loading.contains(account.id) { ProgressView().controlSize(.mini) }
                                }
                                if let snapshot = store.snapshots[account.id], !snapshot.windows.isEmpty {
                                    ForEach(snapshot.windows) { QuotaRow(window: $0, color: account.provider.color, stale: store.errors[account.id] != nil || snapshot.state == "stale") }
                                } else { Text("데이터 대기 중").font(.caption).foregroundStyle(.secondary) }
                                if store.errors[account.id] != nil { Text("연결 확인 필요 · Dashboard에서 자세히 보기").font(.caption2).foregroundStyle(.orange) }
                            }
                            Divider()
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                    .background {
                        GeometryReader { geometry in
                            Color.clear.preference(key: MenuAccountListHeightKey.self, value: geometry.size.height)
                        }
                    }
                }
                .frame(height: min(430, accountListHeight))
                .onPreferenceChange(MenuAccountListHeightKey.self) { accountListHeight = ceil($0) }
            }
            HStack {
                Button("Dashboard 열기") {
                    openDashboard("dashboard")
                }
                Button("설정…") {
                    openDashboard("settings")
                }
                Spacer()
                Button("종료") { NSApp.terminate(nil) }
            }.font(.caption)
        }.padding(18).frame(width: 330).fixedSize(horizontal: false, vertical: true)
            .background(AppTheme.sidebarBackground).tint(AppTheme.accent)
    }
}

private struct MenuAccountListHeightKey: PreferenceKey {
    static let defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = max(value, nextValue()) }
}
