import CQuotaBar
import Darwin

let status = MainActor.assumeIsolated {
    let service = MonitorDataSource()
    return withExtendedLifetime(service) {
        qb_mac_monitor_run(Unmanaged.passUnretained(service).toOpaque()) { context, handle, event, value, bytes, length in
            guard let context, let handle else { return -1 }
            return MainActor.assumeIsolated {
                Unmanaged<MonitorDataSource>.fromOpaque(context).takeUnretainedValue().event(handle, event, value, bytes, length)
            }
        }
    }
}
exit(status)
