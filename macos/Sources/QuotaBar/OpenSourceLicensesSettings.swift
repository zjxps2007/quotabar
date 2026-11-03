import SwiftUI

private typealias ViewState<Value> = SwiftUI.State<Value>

struct OpenSourceLicensesSettings: View {
    @ViewState private var showingNotices = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("오픈소스 라이선스", systemImage: "doc.text").font(.headline)
            Text("AIUsage · sylearn 및 기여자").font(.callout.bold())
            Text("Apache License 2.0").font(.caption).foregroundStyle(.secondary)
            Text("QuotaBar는 AIUsage의 사용량 조회 코드와 Claude 연결 기능 등을 재사용·수정한 독립 프로젝트입니다.")
                .font(.callout)
            HStack(spacing: 16) {
                Button("라이선스 및 고지 보기") { showingNotices = true }
                Link("원본 프로젝트", destination: URL(string: "https://github.com/sylearn/AIUsage")!)
                    .buttonStyle(.plain).foregroundStyle(AppTheme.accent)
            }
        }.frame(maxWidth: .infinity, alignment: .leading).card()
            .sheet(isPresented: $showingNotices) { OpenSourceNoticesSheet() }
    }
}

private enum OpenSourceDocument: String, CaseIterable, Identifiable {
    case notice = "NOTICE", license = "LICENSE"
    var id: String { rawValue }
    var title: String { self == .notice ? "저작권 및 변경 고지" : "Apache License 2.0 전문" }
    var text: String { Self.contents[self] ?? "고지 파일을 읽을 수 없습니다. 앱을 다시 설치해 주세요." }

    // Package the repository originals into both the main app and Dashboard helper.
    private static let contents: [Self: String] = Dictionary(uniqueKeysWithValues: allCases.compactMap { document in
        guard let url = Bundle.main.url(forResource: document.rawValue, withExtension: nil),
              let text = try? String(contentsOf: url, encoding: .utf8) else { return nil }
        return (document, text)
    })
}

private struct OpenSourceNoticesSheet: View {
    @Environment(\.dismiss) private var dismiss
    @ViewState private var selected: OpenSourceDocument = .notice

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("오픈소스 라이선스").font(.title2.bold())
            Picker("문서", selection: $selected) {
                ForEach(OpenSourceDocument.allCases) { document in
                    Text(document.title).tag(document)
                }
            }.pickerStyle(.segmented).labelsHidden()
            ScrollView {
                Text(verbatim: selected.text)
                    .font(.system(size: 12, design: .monospaced))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(16)
            }
            .id(selected)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(AppTheme.background, in: RoundedRectangle(cornerRadius: 10))
            .accessibilityLabel(selected.title)
            HStack {
                Spacer()
                Button("닫기") { dismiss() }.keyboardShortcut(.cancelAction)
            }
        }.padding(24).frame(width: 720, height: 580)
    }
}
