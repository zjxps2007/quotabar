import SwiftUI
import UsageCore

struct AppUpdateSettings: View {
    @ObservedObject var store: MonitorStore
    private var state: AppUpdateState { store.appUpdate }
    private var busy: Bool { [.checking, .downloading, .installing].contains(state.phase) }
    private var status: String {
        if let message = state.message { return message }
        switch state.phase {
        case .idle: return "앱 시작 후와 하루에 한 번 새 정식 버전을 확인합니다."
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
        }.frame(maxWidth: .infinity, alignment: .leading).card()
    }
}
