// Adapted from sylearn/AIUsage DashboardView+Heatmap.swift (Apache-2.0),
// revision e3502ea77b79c2ef5b88f33e36231f156243a86b. Uses Lite's bounded local report.
import SwiftUI
import AppKit
import UsageCore

private typealias ActivityState<Value> = SwiftUI.State<Value>

struct ActivitySelection {
    let provider: ProviderKind
    let day: UsageActivity.Day
    let anchor: CGRect
}

struct ActivityView: View, Equatable {
    let report: LocalUsageReport
    let providers: [ProviderKind]
    let onSelection: (ProviderKind, ActivitySelection?) -> Void
    static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.report == rhs.report && lhs.providers == rhs.providers
    }

    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 300, maximum: 560), spacing: 16, alignment: .top)],
                  alignment: .leading, spacing: 16) {
            ForEach(providers) { provider in
                ActivityCard(snapshot: UsageActivity(report: report, provider: provider,
                    now: report.scannedAt ?? Date()), onSelection: onSelection)
            }
        }
    }
}

private enum ActivityStyle {
    static func cell(future: Bool) -> Color {
        future ? Color.primary.opacity(0.025) : AppTheme.trackBackground
    }
    static func opacity(_ level: Int, _ scheme: ColorScheme) -> Double {
        let levels = scheme == .dark ? [0, 0.44, 0.62, 0.80, 0.98] : [0, 0.38, 0.56, 0.74, 0.92]
        return levels[level]
    }
    static func number(_ value: Int) -> String {
        for (divisor, suffix) in [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")] where value >= divisor {
            return String(format: "%.1f%@", Double(value) / Double(divisor), suffix)
        }
        return value.formatted()
    }
}

private struct ActivityCard: View {
    let snapshot: UsageActivity
    let onSelection: (ProviderKind, ActivitySelection?) -> Void
    @Environment(\.colorScheme) private var scheme
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @ActivityState private var hovered: String?
    @ActivityState private var gridHeight: CGFloat = 108
    private var accent: Color { snapshot.provider.color }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                BrandIcon(provider: snapshot.provider, size: 26).accessibilityHidden(true)
                Text(snapshot.provider.title).font(.subheadline.weight(.semibold))
            }.foregroundStyle(accent)
            if snapshot.activeDays == 0 {
                HStack(spacing: 10) {
                    Image(systemName: "square.grid.3x3").font(.title3).foregroundStyle(.secondary)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("아직 사용 기록이 없습니다").font(.subheadline.weight(.semibold))
                        Text("로컬 로그가 확인되면 일별 토큰이 표시됩니다.").font(.caption2).foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 0)
                }.padding(.vertical, 24).padding(.top, 8)
            } else {
                grid.padding(.top, 8).zIndex(1)
                footer.padding(.top, 10)
            }
        }
        .padding(16).frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 18).fill(AppTheme.cardBackground))
        .overlay(RoundedRectangle(cornerRadius: 18).stroke(AppTheme.cardBorder, lineWidth: 1))
        .onDisappear { clearSelection() }
        .onChange(of: snapshot) { _, _ in clearSelection() }
        .onExitCommand { clearSelection() }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(snapshot.provider.title) 최근 26주 사용 활동")
    }

    private var grid: some View {
        GeometryReader { proxy in
            let spacing: CGFloat = proxy.size.width < 340 ? 2 : 3
            let available = max(0, proxy.size.width - 28)
            let weeks = max(1, min(UsageActivity.weeks, Int((available + spacing) / (7 + spacing))))
            let offset = UsageActivity.weeks - weeks
            let side = max(7, min(16, (available - CGFloat(weeks - 1) * spacing) / CGFloat(weeks)))
            let height = 18 + side * 7 + spacing * 6
            let frame = proxy.frame(in: .named("activity-tooltip-root"))
            VStack(alignment: .leading, spacing: 4) {
                monthLabels(weeks: weeks, offset: offset, pitch: side + spacing).frame(height: 14)
                HStack(alignment: .top, spacing: 0) {
                    VStack(spacing: spacing) {
                        ForEach(0..<7, id: \.self) { row in
                            Text([0: "월", 2: "수", 4: "금"][row] ?? "")
                                .font(.system(size: side < 9 ? 8 : 9, weight: .medium))
                                .foregroundStyle(.secondary).frame(width: 28, height: side, alignment: .leading)
                        }
                    }
                    HStack(alignment: .top, spacing: spacing) {
                        ForEach(0..<weeks, id: \.self) { week in
                            VStack(spacing: spacing) {
                                ForEach(0..<7, id: \.self) { row in
                                    let day = snapshot.days[(week + offset) * 7 + row]
                                    let anchor = CGRect(x: frame.minX + 28 + CGFloat(week) * (side + spacing),
                                        y: frame.minY + 18 + CGFloat(row) * (side + spacing), width: side, height: side)
                                    cell(day, side: side, anchor: anchor)
                                }
                            }
                        }
                    }
                }
            }.preference(key: ActivityGridHeight.self, value: height)
        }
        .frame(height: gridHeight)
        .onPreferenceChange(ActivityGridHeight.self) { height in
            if height > 0, abs(gridHeight - height) > 0.5 { gridHeight = height }
        }
    }

    private func monthLabels(weeks: Int, offset: Int, pitch: CGFloat) -> some View {
        var labels: [(column: Int, month: Int)] = []
        var previousMonth = -1
        for column in 0..<weeks {
            let month = snapshot.days[(column + offset) * 7].month
            if month != previousMonth {
                if labels.last.map({ CGFloat(column - $0.column) * pitch >= 24 }) ?? true {
                    labels.append((column, month))
                }
                previousMonth = month
            }
        }
        return ZStack(alignment: .topLeading) {
            Color.clear
            ForEach(labels, id: \.column) { label in
                Text("\(label.month)월").font(.system(size: 9, weight: .medium)).foregroundStyle(.secondary)
                    .fixedSize().offset(x: 28 + CGFloat(label.column) * pitch)
            }
        }
    }

    private func cell(_ day: UsageActivity.Day, side: CGFloat, anchor: CGRect) -> some View {
        let highlighted = hovered == day.key
        let shape = RoundedRectangle(cornerRadius: max(2, min(4, side * 0.24)), style: .continuous)
        return Button { select(day, anchor: anchor) } label: {
            shape.fill(ActivityStyle.cell(future: day.isFuture))
                .overlay(shape.fill(accent.opacity(ActivityStyle.opacity(snapshot.intensity(day.tokens), scheme))))
                .overlay(shape.stroke(highlighted ? Color.primary.opacity(0.92) : day.isToday ? accent : .clear,
                                      lineWidth: highlighted ? 1.5 : day.isToday ? 1.2 : 0))
                .frame(width: side, height: side)
                .shadow(color: highlighted ? accent.opacity(0.5) : .clear, radius: 3)
                .scaleEffect(highlighted && !reduceMotion ? 1.16 : 1)
        }
        .buttonStyle(.plain).zIndex(highlighted ? 10 : 0)
        .animation(reduceMotion ? nil : .easeOut(duration: 0.1), value: highlighted)
        .onHover { inside in
            if inside { select(day, anchor: anchor) }
            else if hovered == day.key { clearSelection() }
        }
        .accessibilityLabel("\(snapshot.provider.title) \(day.key), \(day.tokens.formatted()) tokens\(day.isToday ? ", 오늘" : "")")
        .accessibilityHint("날짜별 모델 사용량 보기")
        .accessibilityHidden(day.isFuture || (day.tokens == 0 && !day.isToday))
    }

    private var footer: some View {
        let total = Text("총 \(ActivityStyle.number(snapshot.totalTokens)) tokens").font(.caption.weight(.semibold)).lineLimit(1)
        let summary = Text(peakSummary).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
        return ViewThatFits(in: .horizontal) {
            HStack(spacing: 6) { total; Text("·").foregroundStyle(.tertiary); summary; Spacer(minLength: 0) }
            VStack(alignment: .leading, spacing: 3) { total; summary }
        }
    }
    private var peakSummary: String {
        guard let peak = snapshot.peak else { return "활동 \(snapshot.activeDays)일" }
        return "활동 \(snapshot.activeDays)일 · 최고 \(ActivityStyle.number(peak.tokens)) (\(peak.date.formatted(.dateTime.month().day())))"
    }
    private func select(_ day: UsageActivity.Day, anchor: CGRect) {
        hovered = day.key
        onSelection(snapshot.provider, ActivitySelection(provider: snapshot.provider, day: day, anchor: anchor))
    }
    private func clearSelection() { hovered = nil; onSelection(snapshot.provider, nil) }
}

private struct ActivityGridHeight: PreferenceKey {
    static var defaultValue: CGFloat = 108
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}

struct ActivityTooltipLayer: View {
    let selection: ActivitySelection
    let container: CGSize
    @Environment(\.colorScheme) private var scheme
    @ActivityState private var measuredHeight: CGFloat = 0
    private var accent: Color { selection.provider.color }
    var body: some View {
        let width = min(304, max(220, container.width - 24))
        let height = measuredHeight > 0 ? measuredHeight : 90 + CGFloat(min(4, selection.day.models.count)) * 32
        let x = min(max(selection.anchor.midX - width / 2, 12), max(12, container.width - width - 12))
        let above = selection.anchor.minY - height - 10
        let y = above >= 12 ? above : min(selection.anchor.maxY + 10, max(12, container.height - height - 12))
        card.frame(width: width).fixedSize(horizontal: false, vertical: true)
            .background(GeometryReader { proxy in Color.clear.preference(key: ActivityTooltipHeight.self, value: proxy.size.height) })
            .offset(x: x, y: y)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .allowsHitTesting(false)
            .onPreferenceChange(ActivityTooltipHeight.self) { measuredHeight = $0 }
            .onChange(of: selection.day.key) { _, _ in measuredHeight = 0 }
    }
    private var card: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 12) {
                Text(selection.day.date.formatted(.dateTime.year().month().day().weekday()))
                    .font(.system(size: 12, weight: .semibold)).lineLimit(1)
                Spacer(minLength: 8)
                if selection.day.isFuture {
                    Text("아직 도래하지 않은 날짜").font(.system(size: 9)).foregroundStyle(.secondary)
                } else {
                    VStack(alignment: .trailing, spacing: 1) {
                        Text("Tokens").font(.system(size: 8.5)).foregroundStyle(.secondary)
                        Text(selection.day.tokens.formatted()).font(.system(size: 13.5, weight: .semibold)).monospacedDigit()
                    }
                }
            }
            if !selection.day.models.isEmpty {
                Divider().padding(.vertical, 7)
                VStack(alignment: .leading, spacing: 6) {
                    Text("모델 \(selection.day.models.count)").font(.system(size: 9, weight: .semibold)).foregroundStyle(.secondary)
                    ForEach(selection.day.models.prefix(4)) { row in
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            Circle().fill(accent).frame(width: 6, height: 6)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(row.model).font(.system(size: 10.5, weight: .medium)).lineLimit(1).truncationMode(.middle)
                                Text("입력 \(ActivityStyle.number(row.counts.input)) · 출력 \(ActivityStyle.number(row.counts.output)) · 캐시 \(ActivityStyle.number(row.counts.cached))")
                                    .font(.system(size: 8.5)).monospacedDigit().foregroundStyle(.secondary).lineLimit(1)
                            }
                            Spacer(minLength: 6)
                            Text(ActivityStyle.number(row.counts.total)).font(.system(size: 10, weight: .medium)).monospacedDigit()
                        }
                    }
                    if selection.day.models.count > 4 {
                        Text("그 외 \(selection.day.models.count - 4)개 모델").font(.system(size: 8.5)).foregroundStyle(.secondary)
                    }
                }
            }
        }
        .padding(.horizontal, 13).padding(.vertical, 11)
        .background(RoundedRectangle(cornerRadius: 12).fill(AppTheme.cardBackground)
            .shadow(color: .black.opacity(scheme == .dark ? 0.4 : 0.14), radius: 22, y: 9))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(AppTheme.cardBorder, lineWidth: 1))
        .overlay(alignment: .leading) { Capsule().fill(accent).frame(width: 3).padding(.vertical, 11).padding(.leading, 1) }
    }
}

private struct ActivityTooltipHeight: PreferenceKey {
    static var defaultValue: CGFloat = 0
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}
