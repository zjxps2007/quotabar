import SwiftUI
import UsageCore

struct AppSettings: View {
    @ObservedObject var store: MonitorStore
    private var state: AppUpdateState { store.appUpdate }
    private var busy: Bool { [.checking, .downloading, .installing].contains(state.phase) }
    private var status: String {
        if let message = state.message { return message }
        switch state.phase {
        case .idle: return store.settings.areAutomaticUpdatesEnabled
            ? "앱 시작 후와 하루에 한 번 새 정식 버전을 확인합니다."
            : "아래에서 새 버전을 직접 확인할 수 있습니다."
        case .checking: return "새 버전을 확인하고 있습니다…"
        case .current: return "최신 버전입니다."
        case .available: return "새 버전 \(state.version ?? "")을 사용할 수 있습니다."
        case .downloading: return "\(state.version ?? "") 다운로드 중 · \(state.progress ?? 0)%"
        case .ready: return store.settings.areAutomaticUpdatesEnabled
            ? "Dashboard를 닫으면 \(state.version ?? "")을 설치하고 다시 시작합니다."
            : "\(state.version ?? "") 설치 준비가 끝났습니다."
        case .installing: return "업데이트 설치를 준비하고 있습니다…"
        case .failed: return "업데이트를 완료하지 못했습니다. 현재 앱을 유지합니다."
        }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack {
                AppLogo(size: 24)
                Text("QuotaBar").font(.headline)
                Spacer()
                Text("버전 \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—")")
                    .font(.caption).foregroundStyle(.secondary)
            }
            startup
            Divider()
            updates
            Divider()
            VStack(alignment: .leading, spacing: 12) {
                Text("sylearn/AIUsage의 조회 코드와 Claude 연결 기능을 바탕으로 만든 독립 경량 버전입니다.")
                    .font(.caption).foregroundStyle(.secondary)
                HStack(spacing: 16) {
                    Link("QuotaBar · GitHub", destination: URL(string: "https://github.com/zjxps2007/quotabar")!)
                        .buttonStyle(.plain).foregroundStyle(AppTheme.accent)
                    Link(destination: URL(string: "https://github.com/sylearn/AIUsage")!) {
                        Text("원본 프로젝트 · Apache 2.0").foregroundStyle(AppTheme.accent).underline()
                    }.buttonStyle(.plain)
                }
            }
        }.frame(maxWidth: .infinity, alignment: .leading).card()
    }

    private var startup: some View {
        VStack(alignment: .leading, spacing: 14) {
            Toggle("로그인 시 자동 실행", isOn: Binding(get: { store.launchAtLogin.isRegistered }, set: store.setLaunchAtLogin))
                .toggleStyle(.switch).disabled(store.launchAtLogin == .unavailable)
            Text("Mac 재시동 후 로그인하면 Dashboard를 열지 않고 상단바에서 시작합니다.")
                .font(.caption).foregroundStyle(.secondary)
            if store.launchAtLogin == .requiresApproval {
                Text("macOS 로그인 항목 설정에서 QuotaBar의 실행을 허용해 주세요.")
                    .font(.caption).foregroundStyle(.secondary)
                Button("로그인 항목 설정 열기", action: store.openLoginItemsSettings)
            } else if store.launchAtLogin == .unavailable {
                Text("빌드된 QuotaBar.app에서 자동 실행을 설정할 수 있습니다.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private var updates: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("앱 업데이트", systemImage: "arrow.triangle.2.circlepath").font(.headline)
            Toggle("자동 업데이트", isOn: Binding(get: { store.settings.areAutomaticUpdatesEnabled }, set: store.setAutomaticUpdates))
                .toggleStyle(.switch)
            Text("새 정식 버전을 자동으로 내려받고, Dashboard가 닫혀 있으면 설치 후 다시 시작합니다.")
                .font(.caption).foregroundStyle(.secondary)
            HStack(spacing: 8) {
                if busy { ProgressView().controlSize(.small) }
                Text(status).font(.callout).foregroundStyle(state.phase == .failed ? .orange : .primary)
            }
            if state.phase == .downloading { ProgressView(value: Double(state.progress ?? 0), total: 100) }
            HStack {
                Button("업데이트 확인") { store.checkAppUpdate() }.disabled(busy)
                if [.available, .ready].contains(state.phase) {
                    Button("설치 후 다시 시작") { store.installAppUpdate() }.disabled(store.demo)
                }
            }
        }
    }
}
