import importlib.util
import os
from pathlib import Path
import platform
import signal
import subprocess
import tempfile
import textwrap
import time
from types import SimpleNamespace
import unittest
import sys

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("benchmark_ui", Path(__file__).with_name("benchmark-ui.py"))
benchmark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchmark)


class BenchmarkStatisticsTests(unittest.TestCase):
    def test_nearest_rank_handles_small_and_even_samples(self):
        self.assertEqual(benchmark.percentile(list(range(1, 31)), .95), 29)
        self.assertEqual(benchmark.percentile([2, 1], .5), 1)
        self.assertIsNone(benchmark.percentile([], .5))

    def test_idle_excludes_settle_time_and_uses_cpu_deltas(self):
        samples = [{"time": t, "phase": "idle", "resident_mib": memory,
                    "footprint_mib": memory / 2, "cpu_ns": cpu}
                   for t, memory, cpu in [(0, 100, 0), (.25, 100, 50_000_000),
                       (.5, 20, 100_000_000), (.75, 20, 105_000_000)]]
        metrics = benchmark.phase_metrics(samples)["idle"]
        self.assertEqual(metrics["resident_mib"]["p50"], 20)
        self.assertAlmostEqual(metrics["cpu_percent_one_core"], 2)

    def test_mach_timebase_produces_one_core_cpu_percentage(self):
        # Apple Silicon's 24 MHz counter is not a nanosecond counter. A full
        # CPU-second over a wall-second must report 100%, rather than 2.4%.
        samples = [{"time": t, "phase": "selection", "resident_mib": 20,
                    "footprint_mib": 10,
                    "cpu_ns": benchmark.mach_ticks_to_ns(ticks, 125, 3)}
                   for t, ticks in [(0, 0), (1, 24_000_000)]]
        self.assertEqual(benchmark.phase_metrics(samples)["selection"]["cpu_percent_one_core"], 100)


@unittest.skipUnless(platform.system() == "Linux", "Linux process-session integration tests")
class LinuxCleanupTests(unittest.TestCase):
    CHILD = """import os, signal, sys, time
from pathlib import Path
signal.signal(signal.SIGTERM, signal.SIG_IGN)
Path(sys.argv[1]).write_text(str(os.getpid()))
time.sleep(60)
"""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.child_pid_path = self.root / "child.pid"
        self.reader = benchmark.LinuxProcesses()
        self.unrelated = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"],
            start_new_session=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.addCleanup(self.directory.cleanup)
        self.addCleanup(self.stop_unrelated)

    def stop_unrelated(self):
        if self.unrelated.poll() is None:
            self.unrelated.terminate()
        try:
            self.unrelated.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.unrelated.kill()
            self.unrelated.wait(timeout=2)

    def executable(self, mode):
        executable = self.root / "synthetic-app"
        executable.write_text("#!/usr/bin/env python3\n" + textwrap.dedent(f"""
            import json, os, signal, subprocess, sys, time
            from pathlib import Path
            if {mode!r} == 'ignore':
                signal.signal(signal.SIGTERM, signal.SIG_IGN)
            child_path = Path({str(self.child_pid_path)!r})
            subprocess.Popen([sys.executable, '-c', {self.CHILD!r}, str(child_path)])
            while not child_path.exists():
                time.sleep(0.005)
            if {mode!r} == 'exit':
                report = {{'status': 'ok', 'workload': {{'selection_samples': 2,
                    'syntax_samples': 2, 'history_total': 10000, 'changed_files': 46,
                    'heavy_diff_rows': 2000, 'viewport_width': 1440, 'viewport_height': 888}},
                    'frames': {{phase: {{'interval_ms': [16] * 20}}
                        for phase in ['history_scroll', 'diff_scroll']}}}}
                Path(os.environ['MANGROVE_BENCH_OUTPUT']).write_text(json.dumps(report))
                sys.exit(0)
            time.sleep(60)
            """))
        executable.chmod(0o755)
        return executable

    def assert_disposed(self):
        self.assertIsNotNone(self.reader.process.returncode, "root was not reaped")
        child = int(self.child_pid_path.read_text())
        path = Path(f"/proc/{child}/stat")
        def state():
            try:
                text = path.read_text()
                return text[text.rindex(")") + 2:].split()[0]
            except FileNotFoundError:
                return None
        deadline = time.monotonic() + 2
        while state() not in (None, "Z") and time.monotonic() < deadline:
            time.sleep(0.01)
        # A zombie is stopped; only its init reaper owns waitpid.
        self.assertIn(state(), (None, "Z"), "child still running")
        self.assertIsNone(self.unrelated.poll(), "cleanup signalled an unrelated process")

    def run_synthetic(self, mode):
        args = SimpleNamespace(interaction_samples=2, timeout=0.3, poll_ms=10)
        return benchmark.run_application(self.reader, "gpui", self.executable(mode),
            self.root, self.root / "cache", self.root / "report.json", args)

    def test_timeout_stops_root_and_child_which_both_ignore_sigterm(self):
        began = time.monotonic()
        with self.assertRaisesRegex(RuntimeError, "workload timed out"):
            self.run_synthetic("ignore")
        self.assertLess(time.monotonic() - began, 5)
        self.assertEqual(self.reader.process.returncode, -signal.SIGKILL)
        self.assert_disposed()

    def test_successful_root_exit_still_stops_orphaned_child(self):
        report = self.run_synthetic("exit")
        self.assertEqual(report["status"], "ok")
        self.assertEqual(self.reader.process.returncode, 0)
        self.assert_disposed()

    def test_cleanup_rejects_an_unrelated_pid_before_signalling(self):
        executable = self.executable("ignore")
        pid = self.reader.spawn([str(executable)], dict(os.environ))
        deadline = time.monotonic() + 2
        while not self.child_pid_path.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        try:
            with self.assertRaisesRegex(RuntimeError, "dedicated session"):
                self.reader.cleanup(self.unrelated.pid)
            self.assertIsNone(self.unrelated.poll())
        finally:
            self.reader.cleanup(pid)
        self.assert_disposed()


if __name__ == "__main__":
    unittest.main()
