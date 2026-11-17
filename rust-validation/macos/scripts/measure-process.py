#!/usr/bin/env python3
"""Read macOS process counters without activating the app or sampling its stacks."""
import argparse
import ctypes
import datetime
import json
import statistics
import time
from pathlib import Path


class RUsage(ctypes.Structure):
    # sys/resource.h: rusage_info_v0, proc_pid_rusage(..., RUSAGE_INFO_V0, ...).
    _fields_ = [("uuid", ctypes.c_uint8 * 16)] + [
        (name, ctypes.c_uint64) for name in (
            "user_time", "system_time", "pkg_idle_wkups", "interrupt_wkups",
            "pageins", "wired_size", "resident_size", "phys_footprint",
            "proc_start_abstime", "proc_exit_abstime",
        )
    ]


class Timebase(ctypes.Structure):
    _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=int)
    parser.add_argument("--seconds", type=float, default=90)
    parser.add_argument("--interval", type=float, default=2)
    parser.add_argument("--settle", type=float, default=60, help="idle settling seconds before sampling")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.seconds <= 0 or args.interval <= 0 or args.settle < 0:
        parser.error("seconds and interval must be positive; settle must be nonnegative")
    lib = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    lib.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
    lib.proc_pid_rusage.restype = ctypes.c_int
    timebase = Timebase()
    system = ctypes.CDLL("/usr/lib/libSystem.B.dylib")
    system.mach_timebase_info.argtypes = [ctypes.POINTER(Timebase)]
    if system.mach_timebase_info(ctypes.byref(timebase)) or not timebase.denom:
        raise RuntimeError("mach_timebase_info failed")
    initial = RUsage()
    if lib.proc_pid_rusage(args.pid, 0, ctypes.byref(initial)):
        raise OSError(ctypes.get_errno(), "proc_pid_rusage failed")
    if args.settle:
        time.sleep(args.settle)
    samples = []
    start = time.monotonic()
    date = datetime.datetime.now(datetime.timezone.utc).isoformat()
    while True:
        info = RUsage()
        if lib.proc_pid_rusage(args.pid, 0, ctypes.byref(info)):
            raise OSError(ctypes.get_errno(), "proc_pid_rusage failed")
        sample = {name: getattr(info, name) for name, _ in RUsage._fields_[1:]}
        sample["elapsed_seconds"] = time.monotonic() - start
        if info.proc_start_abstime != initial.proc_start_abstime:
            raise RuntimeError("PID was reused during measurement")
        samples.append(sample)
        remaining = args.seconds - sample["elapsed_seconds"]
        if remaining <= 0:
            break
        time.sleep(min(args.interval, remaining))
    first, last = samples[0], samples[-1]
    elapsed = last["elapsed_seconds"] - first["elapsed_seconds"]
    # rusage CPU times use Mach ticks (not nanoseconds on Apple Silicon).
    cpu_seconds = sum(last[k] - first[k] for k in ("user_time", "system_time")) * timebase.numer / timebase.denom / 1e9
    summary = {
        "duration_seconds": elapsed,
        "cpu_seconds": cpu_seconds,
        "cpu_percent_one_core": 100 * cpu_seconds / elapsed,
        "interrupt_wakeups": last["interrupt_wkups"] - first["interrupt_wkups"],
        "package_idle_wakeups": last["pkg_idle_wkups"] - first["pkg_idle_wkups"],
    }
    for key in ("phys_footprint", "resident_size"):
        values = [s[key] / 1024**2 for s in samples]
        summary[key + "_mib"] = {
            "min": min(values), "median": statistics.median(values), "max": max(values),
        }
    result = {"pid": args.pid, "started_utc": date, "settle_seconds": args.settle,
              "mach_timebase": {"numer": timebase.numer, "denom": timebase.denom},
              "summary": summary, "samples": samples}
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
