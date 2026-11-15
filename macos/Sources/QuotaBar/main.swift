import CQuotaBar
import Darwin

let status = MainActor.assumeIsolated {
    let service = MonitorApplication()
    return withExtendedLifetime(service) {
        qb_mac_monitor_run(Unmanaged.passUnretained(service).toOpaque()) { context, event in
            guard let context else { return }
            MainActor.assumeIsolated {
                let service = Unmanaged<MonitorApplication>.fromOpaque(context).takeUnretainedValue()
                switch event {
                case 0: service.launch()
                case 1: service.reopen()
                case 2: service.terminate()
                default: break
                }
            }
        }
    }
}
exit(status)
