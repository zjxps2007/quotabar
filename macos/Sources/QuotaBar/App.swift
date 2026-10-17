import AppKit
import Combine
import SwiftUI
import UsageCore

/// Keep only the native status item alive while the dashboard and popover are closed.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate, NSPopoverDelegate {
    private var store: MonitorStore!
    private var statusItem: NSStatusItem!
    private var dashboard: NSWindow?
    private var popover: NSPopover?
    private var observation: AnyCancellable?
    private var statusUpdatePending = false
    private var displayedMode: MenuBarDisplayMode?
    private var displayedTitle: String?
    private var dashboardHost: DashboardHost?
    private var dashboardClient: DashboardClient?
    private var dashboardStateReady = false
    private let isDashboardHelper = CommandLine.arguments.contains("--dashboard-helper")

    func applicationDidFinishLaunching(_ notification: Notification) {
        if isDashboardHelper {
            let client = DashboardClient()
            dashboardClient = client
            store = MonitorStore(remote: client)
            NSApp.setActivationPolicy(.accessory)
            installMainMenu()
            client.onState = { [weak self] in
                self?.store.apply($0)
                self?.dashboardStateReady = true
            }
            client.onShow = { [weak self] in self?.showDashboard(route: $0) }
            client.start()
            return
        }
        store = MonitorStore(managesDashboard: false)
        let host = DashboardHost(store: store)
        host.onPopover = { [weak self] in self?.togglePopover() }
        dashboardHost = host
        installMainMenu()
        if let url = Bundle.main.url(forResource: "AppDockIcon", withExtension: "png"),
           let icon = NSImage(contentsOf: url) {
            icon.size = NSSize(width: 256, height: 256)
            NSApp.applicationIconImage = icon
        }
        store.applyDockIconVisibility()
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        statusItem.button?.target = self
        statusItem.button?.action = #selector(togglePopover)
        statusItem.button?.imageScaling = .scaleNone
        updateStatusItem()
        observation = store.objectWillChange.sink { [weak self] _ in
            self?.scheduleStatusUpdate()
        }
        if !CommandLine.arguments.contains("--menu-bar-only") { showDashboard() }
        recordWindows("ui-launched")
        if CommandLine.arguments.contains("--memory-probe"), CommandLine.arguments.contains("--diagnostics") {
            runMemoryProbe()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationWillTerminate(_ notification: Notification) {
        dashboardHost?.stop()
        dashboardClient?.disconnect()
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !hasVisibleWindows { showDashboard() }
        return true
    }

    func applicationDidHide(_ notification: Notification) {
        dashboard?.contentView = nil
        store.setVisible(false)
    }

    func applicationDidUnhide(_ notification: Notification) {
        if let dashboard, dashboard.isVisible, !dashboard.isMiniaturized {
            attachDashboard(to: dashboard)
        }
    }

    private func scheduleStatusUpdate() {
        guard !statusUpdatePending else { return }
        statusUpdatePending = true
        // Published sends before its value changes. Coalesce the completed mutations.
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.statusUpdatePending = false
            self.updateStatusItem()
        }
    }

    private func updateStatusItem() {
        guard let button = statusItem.button else { return }
        let mode = store.settings.selectedMenuBarDisplayMode
        let title = mode == .remainingOnly ? store.menuBarRemaining : store.menuBarTitle
        if displayedMode != mode || displayedTitle != title {
            switch mode {
            case .providerAndRemaining:
                button.title = ""
                button.image = MenuBarTitleImage.image(for: title)
                button.imagePosition = .imageOnly
            case .remainingOnly:
                button.image = nil
                button.title = title
                button.font = MenuBarTitleImage.font
                button.imagePosition = .noImage
            case .iconOnly:
                button.title = ""
                button.image = MenuBarTitleImage.icon
                button.imagePosition = .imageOnly
            }
            displayedMode = mode
            displayedTitle = title
        }
        let help = store.menuBarHelp
        if button.toolTip != help { button.toolTip = help }
        button.setAccessibilityLabel("남은 사용량 · \(store.menuBarTitle)")
    }

    @objc private func togglePopover() {
        if let dashboardClient { dashboardClient.send(.showPopover); return }
        if let popover, popover.isShown { popover.performClose(nil); return }
        guard let button = statusItem.button else { return }
        let controller = NSHostingController(rootView: MenuPopoverRoot(store: store) { [weak self] route in
            self?.showDashboard(route: route)
        })
        controller.sizingOptions = .preferredContentSize
        let panel = NSPopover()
        panel.behavior = .transient
        panel.delegate = self
        panel.contentViewController = controller
        controller.view.layoutSubtreeIfNeeded()
        panel.contentSize = controller.view.fittingSize
        popover = panel
        NSApp.activate(ignoringOtherApps: true)
        panel.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        recordWindows("popover-opened")
    }

    func popoverDidClose(_ notification: Notification) {
        guard let closed = notification.object as? NSPopover else { return }
        closed.contentViewController = nil
        if popover === closed { popover = nil }
        store.reclaimIdleMemory()
        recordWindows("popover-closed")
    }

    private func showDashboard(route: String? = nil) {
        popover?.performClose(nil)
        if let dashboardHost { dashboardHost.show(route: route); return }
        guard !isDashboardHelper || dashboardStateReady else { return }
        if let route { store.route = route }
        let window: NSWindow
        if let existing = dashboard {
            window = existing
        } else {
            window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1120, height: 810),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                              backing: .buffered, defer: true)
            window.title = "QuotaBar"
            window.identifier = NSUserInterfaceItemIdentifier("dashboard")
            window.titleVisibility = .hidden
            window.titlebarAppearsTransparent = true
            window.isReleasedWhenClosed = false
            window.contentMinSize = NSSize(width: 900, height: 660)
            window.delegate = self
            if !window.setFrameUsingName("DashboardWindow") { window.center() }
            window.setFrameAutosaveName("DashboardWindow")
            dashboard = window
        }
        attachDashboard(to: window)
        if window.isMiniaturized { window.deminiaturize(nil) }
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        recordWindows("dashboard-opened")
    }

    private func attachDashboard(to window: NSWindow) {
        store.setVisible(true)
        guard !(window.contentView is NSHostingView<DashboardRoot>) else { return }
        let host = NSHostingView(rootView: DashboardRoot(store: store))
        host.sizingOptions = []
        window.contentView = host
    }

    func windowWillClose(_ notification: Notification) {
        guard let window = notification.object as? NSWindow, dashboard === window else { return }
        window.contentView = nil
        dashboard = nil
        store.setVisible(false)
        recordWindows("dashboard-closed")
        if isDashboardHelper { NSApp.terminate(nil) }
    }

    func windowDidMiniaturize(_ notification: Notification) {
        guard let window = notification.object as? NSWindow, dashboard === window else { return }
        window.contentView = nil
        store.setVisible(false)
    }

    func windowDidDeminiaturize(_ notification: Notification) {
        guard let window = notification.object as? NSWindow, dashboard === window else { return }
        attachDashboard(to: window)
    }

    @objc private func openDashboard() { showDashboard(route: "dashboard") }
    @objc private func openSettings() { showDashboard(route: "settings") }
    @objc private func refresh() { store.refreshAll() }
    @objc private func quitApplication() {
        if let dashboardClient { dashboardClient.send(.quit) }
        else { NSApp.terminate(nil) }
    }

    private func recordWindows(_ event: String) {
        store.diagnostic(event, details: ["dashboardRetained": dashboard != nil || dashboardHost?.isRunning == true,
            "popoverRetained": popover != nil, "popoverVisible": popover?.isShown == true])
    }

    /// Exercise real views without a screen-capture stream altering the measurement.
    private func runMemoryProbe() {
        Task { @MainActor [weak self] in
            guard let self else { return }
            for _ in 0..<2 {
                if !CommandLine.arguments.contains("--menu-bar-only") {
                    for route in ["dashboard", "accounts", "settings"] {
                        self.showDashboard(route: route)
                        try? await Task.sleep(for: .seconds(route == "dashboard" ? 5 : 2))
                    }
                    self.dashboardHost?.close()
                    try? await Task.sleep(for: .seconds(2))
                }
                self.togglePopover()
                try? await Task.sleep(for: .seconds(2))
                self.popover?.performClose(nil)
                try? await Task.sleep(for: .seconds(1))
            }
            self.recordWindows("memory-probe-idle")
        }
    }

    private func installMainMenu() {
        let main = NSMenu()
        func menu(_ title: String) -> NSMenu {
            let item = NSMenuItem(title: title, action: nil, keyEquivalent: "")
            let submenu = NSMenu(title: title)
            item.submenu = submenu
            main.addItem(item)
            return submenu
        }
        func add(_ menu: NSMenu, _ title: String, _ action: Selector, _ key: String = "", target: AnyObject? = nil) {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
            item.target = target
            menu.addItem(item)
        }
        let app = menu("QuotaBar")
        add(app, "QuotaBar 정보", #selector(NSApplication.orderFrontStandardAboutPanel(_:)))
        app.addItem(.separator())
        add(app, "설정…", #selector(openSettings), ",", target: self)
        app.addItem(.separator())
        let services = NSMenu(title: "서비스")
        let servicesItem = NSMenuItem(title: "서비스", action: nil, keyEquivalent: "")
        servicesItem.submenu = services
        app.addItem(servicesItem)
        NSApp.servicesMenu = services
        app.addItem(.separator())
        add(app, "QuotaBar 가리기", #selector(NSApplication.hide(_:)), "h")
        add(app, "다른 항목 가리기", #selector(NSApplication.hideOtherApplications(_:)), "h")
        app.items.last?.keyEquivalentModifierMask = [.command, .option]
        add(app, "모두 보기", #selector(NSApplication.unhideAllApplications(_:)))
        app.addItem(.separator())
        add(app, "QuotaBar 종료", #selector(quitApplication), "q", target: self)
        let edit = menu("Edit")
        add(edit, "실행 취소", Selector(("undo:")), "z")
        add(edit, "다시 실행", Selector(("redo:")), "Z")
        edit.addItem(.separator())
        for (title, action, key) in [("잘라내기", "cut:", "x"), ("복사", "copy:", "c"),
                                     ("붙여넣기", "paste:", "v"), ("모두 선택", "selectAll:", "a")] {
            add(edit, title, Selector(action), key)
        }
        let view = menu("View")
        add(view, "전체 화면 전환", #selector(NSWindow.toggleFullScreen(_:)), "f")
        view.items.last?.keyEquivalentModifierMask = [.command, .control]
        let monitoring = menu("모니터링")
        add(monitoring, "새로고침", #selector(refresh), "r", target: self)
        add(monitoring, "Dashboard 열기", #selector(openDashboard), "1", target: self)
        add(monitoring, "상단바 사용량 보기", #selector(togglePopover), "2", target: self)
        let windows = menu("Window")
        add(windows, "닫기", #selector(NSWindow.performClose(_:)), "w")
        add(windows, "최소화", #selector(NSWindow.performMiniaturize(_:)), "m")
        add(windows, "확대/축소", #selector(NSWindow.performZoom(_:)))
        add(windows, "모두 앞으로 가져오기", #selector(NSApplication.arrangeInFront(_:)))
        NSApp.windowsMenu = windows
        NSApp.mainMenu = main
    }
}

private struct DashboardRoot: View {
    @ObservedObject var store: MonitorStore
    var body: some View {
        RootView().environmentObject(store)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(AppTheme.background)
            .preferredColorScheme(AppTheme.colorScheme(for: store.settings.selectedAppearance))
    }
}

private struct MenuPopoverRoot: View {
    @ObservedObject var store: MonitorStore
    let openDashboard: (String) -> Void
    var body: some View {
        MenuPanel(openDashboard: openDashboard).environmentObject(store)
            .preferredColorScheme(AppTheme.colorScheme(for: store.settings.selectedAppearance))
    }
}

struct MenuBarLabel: View {
    @ObservedObject var store: MonitorStore
    var body: some View {
        switch store.settings.selectedMenuBarDisplayMode {
        case .providerAndRemaining: MenuBarTitle(title: store.menuBarTitle)
        case .remainingOnly: Text(store.menuBarRemaining).monospacedDigit()
        case .iconOnly: AppMark()
        }
    }
}
