#!/usr/bin/env python3
"""Run paired release-app benchmarks; workloads execute inside each native app.

No browser/OS UI automation. The macOS reader attributes WebKit XPC processes by
responsible PID, not their names, and measures cumulative CPU with libproc.
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import select
import signal
import subprocess
import tempfile
import time


def percentile(values, quantile):
    """Nearest-rank percentile: explicit and shared by every reported metric."""
    ordered = sorted(v for v in values if math.isfinite(v))
    if not ordered:
        return None
    return ordered[max(0, math.ceil(quantile * len(ordered)) - 1)]


def summary(values):
    return {"samples": len(values), "p50": percentile(values, 0.50),
            "p95": percentile(values, 0.95), "min": min(values) if values else None,
            "max": max(values) if values else None}


def mach_ticks_to_ns(ticks, numer, denom):
    """rusage CPU counters are Mach absolute ticks, including on Apple Silicon."""
    return ticks * numer // denom


class MacProcesses:
    """Capability-tested macOS responsible-PID and rusage reader.

    responsibility_* symbols are undocumented platform APIs. We fail explicitly
    when absent instead of guessing attribution from process names/launch time.
    """

    FIELDS = (
        "user_time system_time pkg_idle_wkups interrupt_wkups pageins wired_size "
        "resident_size phys_footprint proc_start_abstime proc_exit_abstime "
        "child_user_time child_system_time child_pkg_idle_wkups child_interrupt_wkups "
        "child_pageins child_elapsed_abstime diskio_bytesread diskio_byteswritten "
        "cpu_time_qos_default cpu_time_qos_maintenance cpu_time_qos_background "
        "cpu_time_qos_utility cpu_time_qos_legacy cpu_time_qos_user_initiated "
        "cpu_time_qos_user_interactive billed_system_time serviced_system_time "
        "logical_writes lifetime_max_phys_footprint instructions cycles billed_energy "
        "serviced_energy interval_max_phys_footprint runnable_time"
    ).split()

    def __init__(self):
        self.lib = ctypes.CDLL("/usr/lib/libSystem.B.dylib", use_errno=True)
        class MachTimebase(ctypes.Structure):
            _fields_ = [("numer", ctypes.c_uint32), ("denom", ctypes.c_uint32)]
        info = MachTimebase()
        timebase = self.lib.mach_timebase_info
        timebase.argtypes = [ctypes.POINTER(MachTimebase)]
        timebase.restype = ctypes.c_int
        if timebase(ctypes.byref(info)) != 0 or not info.numer or not info.denom:
            raise RuntimeError("Cannot determine Mach CPU-counter timebase")
        self.cpu_timebase = {"numer": info.numer, "denom": info.denom}
        self.responsible = self.lib.responsibility_get_pid_responsible_for_pid
        self.responsible.argtypes = [ctypes.c_int]
        self.responsible.restype = ctypes.c_int
        self.usage_type = type("RusageV4", (ctypes.Structure,), {
            "_fields_": [("uuid", ctypes.c_uint8 * 16)]
                + [(name, ctypes.c_uint64) for name in self.FIELDS]})
        self.rusage = self.lib.proc_pid_rusage
        self.rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]
        self.rusage.restype = ctypes.c_int

    def spawn(self, argv, env):
        """Launch a new foreground bundle through NSWorkspace; never infer its PID."""
        bundle = Path(argv[0]).parents[2]
        if bundle.suffix != ".app":
            raise RuntimeError("macOS benchmarking requires an executable inside an application bundle")
        self.launch_process = subprocess.Popen([str(self.launch_helper), str(bundle), *argv[1:]],
            env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0)
        # The helper bounds launch completion to 20s. Publish the exact PID before
        # activation settles, so resource observation covers that waiting period.
        announcement = self.launch_process.stdout.readline(4096)
        try:
            value = json.loads(announcement)
            pid = int(value["pid"])
            if value.get("status") != "launched" or pid <= 1:
                raise ValueError()
        except (ValueError, KeyError, TypeError):
            self.stop_launch_helper()
            raise RuntimeError("LaunchServices did not return a new application PID") from None
        self.launch_pid = pid
        self.launch_metadata = {"active": False}
        self.launch_buffer = b""
        if self.responsible(pid) != pid:
            os.kill(pid, signal.SIGTERM)
            self.stop_launch_helper()
            raise RuntimeError("The benchmark application did not get its own responsible PID")
        return pid

    def poll_launch(self):
        pipe = self.launch_process.stdout
        if select.select([pipe], [], [], 0)[0]:
            self.launch_buffer += os.read(pipe.fileno(), 8192)
            if len(self.launch_buffer) > 8192:
                raise RuntimeError("The launch metadata exceeds its bound")
            while b"\n" in self.launch_buffer:
                line, self.launch_buffer = self.launch_buffer.split(b"\n", 1)
                event = json.loads(line)
                if event.get("pid") != self.launch_pid or event.get("status") != "active":
                    raise RuntimeError("Unexpected launch activation metadata")
                self.launch_metadata = {"active": True, "window": event["window"]}
        status = self.launch_process.poll()
        if status not in (None, 0):
            raise RuntimeError("LaunchServices did not activate the benchmark application")

    def stop_launch_helper(self):
        if self.launch_process.poll() is None:
            self.launch_process.terminate()
        try:
            self.launch_process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.launch_process.kill()
            self.launch_process.wait()
        self.launch_process.stdout.close()

    def exit_status(self, pid):
        # LaunchServices children belong to launchd, not this runner. Its exit
        # status is unavailable; require process disappearance and an atomic,
        # validated successful application report instead.
        try:
            os.kill(pid, 0)
            return None
        except ProcessLookupError:
            return 0

    def snapshot(self, root_pid):
        listing = subprocess.check_output(["ps", "-axo", "pid=,ppid=,comm="], text=True)
        result = {}
        for line in listing.splitlines():
            fields = line.strip().split(None, 2)
            if len(fields) < 3:
                continue
            pid = int(fields[0])
            if pid != root_pid and self.responsible(pid) != root_pid:
                continue
            usage = self.usage_type()
            if self.rusage(pid, 4, ctypes.byref(usage)) != 0:
                continue
            role = "app"
            if "com.apple.WebKit." in fields[2]:
                role = "webview"
            result[pid] = {"cpu_ns": mach_ticks_to_ns(usage.user_time + usage.system_time,
                    self.cpu_timebase["numer"], self.cpu_timebase["denom"]),
                "resident_bytes": usage.resident_size, "footprint_bytes": usage.phys_footprint,
                "role": role}
        return result


class LinuxProcesses:
    def __init__(self):
        self.tick_ns = 1_000_000_000 / os.sysconf("SC_CLK_TCK")
        self.page = os.sysconf("SC_PAGE_SIZE")

    def spawn(self, argv, env):
        self.process = subprocess.Popen(argv, env=env, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL, start_new_session=True)
        self.process_group = self.process.pid
        self._check_group(self.process.pid)
        return self.process.pid

    def _check_group(self, pid):
        # waitid(WNOWAIT) keeps our own leader unreaped until cleanup. Its PID
        # cannot be reused while we signal this dedicated session/process group.
        if (pid != self.process.pid or os.getpgid(pid) != self.process_group
                or os.getsid(pid) != pid or self.process_group != pid):
            raise RuntimeError("The benchmark process lost its dedicated session")

    def exit_status(self, pid):
        self._check_group(pid)
        result = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        if result is None:
            return None
        return result.si_status if result.si_code == os.CLD_EXITED else -result.si_status

    def _group_running(self):
        for path in Path("/proc").glob("[0-9]*/stat"):
            try:
                text = path.read_text()
                fields = text[text.rindex(")") + 2:].split()
                if (int(fields[2]) == self.process_group
                        and int(fields[3]) == self.process_group and fields[0] != "Z"):
                    return True
            except (OSError, ValueError, IndexError):
                continue
        return False

    def cleanup(self, pid):
        self._check_group(pid)
        os.killpg(self.process_group, signal.SIGTERM)
        term_deadline = time.monotonic() + 0.5
        while self._group_running() and time.monotonic() < term_deadline:
            time.sleep(0.01)
        if self._group_running():
            # The unreaped leader still reserves this PGID even when the app
            # exited successfully before its child processes stopped.
            self._check_group(pid)
            os.killpg(self.process_group, signal.SIGKILL)
        kill_deadline = time.monotonic() + 2
        while self._group_running() and time.monotonic() < kill_deadline:
            time.sleep(0.01)
        running = self._group_running()
        try:
            self.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            raise RuntimeError("The benchmark process did not stop after SIGKILL") from None
        if running:
            raise RuntimeError("Benchmark descendants did not stop after SIGKILL")

    def snapshot(self, root_pid):
        records = {}
        for path in Path("/proc").glob("[0-9]*/stat"):
            try:
                text = path.read_text()
                end = text.rindex(")")
                fields = text[end + 2:].split()
                pid = int(path.parent.name)
                records[pid] = {"ppid": int(fields[1]),
                    "cpu_ns": (int(fields[11]) + int(fields[12])) * self.tick_ns,
                    "resident_bytes": int(fields[21]) * self.page,
                    "footprint_bytes": None, "role": "app" if pid == root_pid else "webview"}
            except (OSError, ValueError, IndexError):
                continue
        owned = {root_pid}
        for _ in range(10):
            added = {pid for pid, record in records.items() if record["ppid"] in owned} - owned
            if not added:
                break
            owned |= added
        return {pid: records[pid] for pid in owned if pid in records}


def phase_metrics(samples):
    phases = {}
    previous = None
    for sample in samples:
        phase = sample["phase"]
        phase_samples = phases.setdefault(phase, {"resident": [], "footprint": [],
            "cpu_ns": 0, "wall_ns": 0, "phase_start": sample["time"]})
        # Stable idle excludes the first 500ms to let async/preload tails settle.
        if phase != "idle" or sample["time"] - phase_samples["phase_start"] >= 0.5:
            phase_samples["resident"].append(sample["resident_mib"])
            if sample["footprint_mib"] is not None:
                phase_samples["footprint"].append(sample["footprint_mib"])
            if (previous and previous["phase"] == phase
                    and (phase != "idle" or previous["time"] - phase_samples["phase_start"] >= 0.5)):
                phase_samples["cpu_ns"] += max(0, sample["cpu_ns"] - previous["cpu_ns"])
                phase_samples["wall_ns"] += (sample["time"] - previous["time"]) * 1_000_000_000
        previous = sample
    return {phase: {"resident_mib": summary(data["resident"]),
        "physical_footprint_mib": summary(data["footprint"]),
        "cpu_percent_one_core": 100 * data["cpu_ns"] / data["wall_ns"] if data["wall_ns"] else None,
        "observed_duration_ms": data["wall_ns"] / 1_000_000}
        for phase, data in phases.items()}


def run_application(reader, implementation, executable, repository, cache, output, args):
    env = dict(os.environ)
    env.update(MANGROVE_BENCH_OUTPUT=str(output), MANGROVE_BENCH_CACHE=str(cache),
        MANGROVE_BENCH_REPO=str(repository), MANGROVE_BENCH_SAMPLES=str(args.interaction_samples))
    # Opening a repository records a recent entry even in benchmark mode. Keep
    # those preferences in the temporary run directory, away from the user's app.
    env["MANGROVE_GPUI_PREFERENCES"] = str(output.with_suffix(".preferences.json"))
    argv = [str(executable)] + ([str(repository)] if implementation == "gpui" else [])
    env["MANGROVE_BENCH_LAUNCH_UNIX_MS"] = str(time.time_ns() / 1_000_000)
    pid = reader.spawn(argv, env)
    began = time.monotonic()
    phase_path = output.with_suffix(".phase")
    observations, seen, cpu_by_pid = [], {}, {}
    exit_status = None
    try:
        while time.monotonic() - began < args.timeout:
            if isinstance(reader, MacProcesses):
                reader.poll_launch()
            processes = reader.snapshot(pid)
            seen.update({process: record["role"] for process, record in processes.items()})
            for process, record in processes.items():
                cpu_by_pid[process] = record["cpu_ns"]
            try:
                phase = phase_path.read_text().strip()
            except OSError:
                phase = "launch"
            footprints = [record["footprint_bytes"] for record in processes.values()]
            observations.append({"time": time.monotonic(), "phase": phase,
                "resident_mib": sum(record["resident_bytes"] for record in processes.values()) / 1_048_576,
                "footprint_mib": sum(footprints) / 1_048_576 if footprints and all(v is not None for v in footprints) else None,
                "cpu_ns": sum(cpu_by_pid.values())})
            exit_status = reader.exit_status(pid)
            if exit_status is not None:
                break
            time.sleep(args.poll_ms / 1_000)
        if exit_status is None:
            raise RuntimeError(f"{implementation}: benchmark workload timed out")
        if exit_status != 0 or not output.is_file():
            raise RuntimeError(f"{implementation}: app exited without a complete report")
        report = json.loads(output.read_text())
        if isinstance(reader, MacProcesses):
            reader.poll_launch()
            if not reader.launch_metadata.get("active"):
                raise RuntimeError("The benchmark window was never observed active")
            if not reader.launch_metadata.get("window", {}).get("on_main_display"):
                raise RuntimeError(f"The benchmark window is not wholly on the main display: {reader.launch_metadata['window']}")
            report["launch"] = reader.launch_metadata
        if report.get("status") != "ok":
            raise RuntimeError(f"{implementation}: app reported an incomplete workload")
        workload = report.get("workload", {})
        expected_interactions = args.interaction_samples // 2 * 2
        if (workload.get("selection_samples") != expected_interactions
                or workload.get("syntax_samples") != expected_interactions
                or workload.get("history_total", 0) < 10_000
                or workload.get("changed_files") != 46
                or workload.get("heavy_diff_rows") != 2_000
                or workload.get("viewport_width") != 1_440
                or workload.get("viewport_height") != 888):
            raise RuntimeError(f"{implementation}: workload size or viewport differs from the required protocol: {workload}")
        for phase in ("history_scroll", "diff_scroll"):
            if len(report.get("frames", {}).get(phase, {}).get("interval_ms", [])) < 20:
                raise RuntimeError(f"{implementation}: insufficient frame observations for the required scroll workload")
        if implementation == "tauri" and not any(role == "webview" for role in seen.values()):
            raise RuntimeError("Tauri: no attributable WebView process observed; memory comparison rejected")
        report["resources"] = {"scope": "application + responsible-PID XPC processes" if platform.system() == "Darwin" else "application + descendants",
            "poll_ms": args.poll_ms, "observations": len(observations),
            "processes_observed": len(seen), "webview_processes_observed": sum(role == "webview" for role in seen.values()),
            "peak_resident_mib": max(sample["resident_mib"] for sample in observations),
            "peak_physical_footprint_mib": max((sample["footprint_mib"] for sample in observations if sample["footprint_mib"] is not None), default=None),
            "phases": phase_metrics(observations)}
        return report
    finally:
        if isinstance(reader, MacProcesses):
            # Preserve macOS responsible-PID/XPC attribution and cleanup.
            if exit_status is None:
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            for child in seen:
                if child == pid:
                    continue
                try:
                    if reader.responsible(child) == pid:
                        os.kill(child, signal.SIGTERM)
                except ProcessLookupError:
                    pass
            reader.stop_launch_helper()
        else:
            reader.cleanup(pid)


def aggregate(runs):
    result = {}
    for mode in ("cold_app_cache", "warm_app_cache"):
        result[mode] = {}
        for implementation in ("tauri", "gpui"):
            selected = [run for run in runs if run["cache_mode"] == mode and run["implementation"] == implementation]
            metrics = {}
            names = sorted({name for run in selected for name in run["timings_ms"]})
            for name in names:
                # Launches are independent units; within-launch repetitions are correlated.
                metrics[name] = {"per_launch_p50_ms": summary([percentile(run["timings_ms"].get(name, []), .5) for run in selected if run["timings_ms"].get(name)]),
                    "pooled_ms": summary([value for run in selected for value in run["timings_ms"].get(name, [])])}
            resource_metrics = {"peak_resident_mib": summary([run["resources"]["peak_resident_mib"] for run in selected]),
                "peak_physical_footprint_mib": summary([run["resources"]["peak_physical_footprint_mib"] for run in selected if run["resources"]["peak_physical_footprint_mib"] is not None])}
            for phase in ("idle", "history_scroll", "diff_scroll", "selection", "syntax"):
                resource_metrics[phase] = {}
                for name in ("cpu_percent_one_core", "resident_mib", "physical_footprint_mib"):
                    values = []
                    for run in selected:
                        metric = run["resources"]["phases"].get(phase, {}).get(name)
                        value = metric.get("p50") if isinstance(metric, dict) else metric
                        if value is not None:
                            values.append(value)
                    resource_metrics[phase][name] = summary(values)
            frame_metrics = {}
            for phase in ("history_scroll", "diff_scroll"):
                frame_metrics[phase] = {}
                for name in ("interval_ms", "draw_ms", "submit_ms"):
                    lists = [run["frames"].get(phase, {}).get(name, []) for run in selected]
                    frame_metrics[phase][name] = {"per_launch_p95_ms": summary([percentile(values, .95) for values in lists if values]),
                        "pooled_ms": summary([value for values in lists for value in values])}
            result[mode][implementation] = {"launches": len(selected), "timings": metrics,
                "resources": resource_metrics, "frames": frame_metrics}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tauri", type=Path, required=True, help="Packaged release executable inside .app on macOS")
    parser.add_argument("--gpui", type=Path, required=True, help="Packaged release executable inside .app on macOS")
    parser.add_argument("--repository", type=Path, required=True, help="Synthetic fixture only")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=30, help="Independent paired launches per cache condition")
    parser.add_argument("--interaction-samples", type=int, default=30)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--poll-ms", type=int, default=100)
    args = parser.parse_args()
    if not (1 <= args.samples <= 100 and 2 <= args.interaction_samples <= 100 and 25 <= args.poll_ms <= 1_000):
        parser.error("Sample counts or polling interval are outside supported bounds")
    for name in ("tauri", "gpui", "repository", "output"):
        setattr(args, name, getattr(args, name).resolve())
    for executable in (args.tauri, args.gpui):
        if not executable.is_file():
            parser.error("A benchmark release executable is missing")
    if not (args.repository / ".git").exists():
        parser.error("The synthetic fixture is missing")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    reader = MacProcesses() if platform.system() == "Darwin" else LinuxProcesses()
    runs = []
    context = {"os": platform.platform(), "architecture": platform.machine(),
        "logical_cpus": os.cpu_count(), "python_version": platform.python_version(),
        "build_profile": "release; caller must build both release executables",
        "filesystem_cache": "warm/uncontrolled; cold means fresh application SQLite cache only",
        "launch_samples_per_condition": args.samples,
        "interaction_samples_per_launch": args.interaction_samples,
        "display": "same active display; refresh rate not forced; record hardware separately",
        "launch_method": "NSWorkspace LaunchServices new-instance foreground activation; exact PID returned" if platform.system() == "Darwin" else "direct subprocess",
        "termination_validation": "process disappeared + validated atomic successful report; native exit status unavailable" if platform.system() == "Darwin" else "child exit status + successful report",
        "attribution": "verified app responsible PID (capability-tested undocumented macOS API)" if platform.system() == "Darwin" else "PID ancestry",
        "background_load": "existing user applications left running; background workload not neutralized",
        "artifacts": {name: {"sha256": hashlib.sha256(getattr(args, name).read_bytes()).hexdigest(),
            "bytes": getattr(args, name).stat().st_size} for name in ("tauri", "gpui")},
        "runner_source_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
    if isinstance(reader, MacProcesses):
        context["cpu_timebase_normalization"] = {**reader.cpu_timebase, "applied": True,
            "method": "Mach user+system ticks converted to nanoseconds in the process reader before phase deltas",
            "provenance": "mach_timebase_info; Apple XNU recount tests",
            "source_url": "https://github.com/apple-oss-distributions/xnu/blob/main/tests/recount/recount_perf_tests.c#L183-L215"}
    with tempfile.TemporaryDirectory(prefix="mangrove-ui-benchmark-") as temporary:
        root = Path(temporary)
        if isinstance(reader, MacProcesses):
            reader.launch_helper = root / "launch-application"
            subprocess.run(["xcrun", "swiftc", "-O", str(Path(__file__).with_name("benchmark-launch.swift")),
                "-o", str(reader.launch_helper)], check=True)
            context["launch_helper_source_sha256"] = hashlib.sha256(Path(__file__).with_name("benchmark-launch.swift").read_bytes()).hexdigest()
            context["launch_helper_binary_sha256"] = hashlib.sha256(reader.launch_helper.read_bytes()).hexdigest()
        for trial in range(args.samples):
            for mode in ("cold_app_cache", "warm_app_cache"):
                order = ("tauri", "gpui") if trial % 2 == 0 else ("gpui", "tauri")
                for implementation in order:
                    output = root / f"report-{trial}-{mode}-{implementation}.json"
                    cache = root / f"cache-{trial}-{implementation}"
                    cache.mkdir(exist_ok=True)
                    report = run_application(reader, implementation, getattr(args, implementation), args.repository, cache, output, args)
                    report["cache_mode"] = mode
                    report["trial"] = trial + 1
                    paired = [run for run in runs if run["trial"] == trial + 1 and run["cache_mode"] == mode]
                    if paired and report["workload"] != paired[0]["workload"]:
                        raise RuntimeError("Applications completed different workloads; comparison rejected")
                    runs.append(report)
                    payload = {"schema_version": 1, "complete": len(runs) == args.samples * 4,
                        "completed_launches": len(runs), "expected_launches": args.samples * 4,
                        "context": context,
                        "summary": aggregate(runs), "runs": runs}
                    temp_output = args.output.with_suffix(".tmp")
                    temp_output.write_text(json.dumps(payload, indent=2) + "\n")
                    temp_output.replace(args.output)
                    print(f"{trial + 1}/{args.samples} {mode} {implementation}: complete", flush=True)


if __name__ == "__main__":
    main()
