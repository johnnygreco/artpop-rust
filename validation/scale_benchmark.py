"""Fresh-process timing of larger equivalent workflows (scale_cases.py).

Same methodology as benchmark.py (process-tree RSS sampling, warmups, raw
samples), but with workloads large enough that computation rather than
interpreter/process startup dominates. MIST data for the Rust backend is
registered with `data.import` before timing.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys

from benchmark import measure
from scale_cases import CASES, MIST_SYSTEM_DIR, check


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--rust-bin", type=Path, required=True)
    p.add_argument("--work-dir", type=Path, required=True)
    p.add_argument("--report", type=Path, required=True)
    p.add_argument("--repeat", type=int, default=3)
    p.add_argument("--warmup", type=int, default=1)
    p.add_argument("--cases", nargs="+", choices=CASES, default=list(CASES))
    a = p.parse_args()
    a.work_dir = a.work_dir.resolve()
    if a.work_dir.exists() and any(a.work_dir.iterdir()):
        p.error("Use an empty work directory")
    a.work_dir.mkdir(parents=True)
    binary = a.rust_bin.resolve()
    data_dir = a.work_dir / "rust-data"
    if "mist_mass_cutoff" in a.cases or "mist_full_1e7" in a.cases:
        mist = Path(os.environ["MIST_PATH"]) / MIST_SYSTEM_DIR
        r = subprocess.run([str(binary), "--data-dir", str(data_dir), "--artifact-dir", str(a.work_dir / "setup"), "data", "import", "--set", f"path={mist}"], capture_output=True, text=True)
        assert json.loads(r.stdout)["status"] == "completed", r.stdout
    script = Path(__file__).with_name("scale_cases.py")
    report = {"schema_version": "1", "measurement": "fresh-process end-to-end workflows at scale; Rust times include the Python adapter process that drives the CLI",
              "python": sys.version, "platform": platform.platform(), "logical_cpus": os.cpu_count(), "rust_binary": str(binary),
              "rust_version": subprocess.check_output([str(binary), "--version"], text=True).strip(), "warmup": a.warmup, "repeat": a.repeat, "cases": {}, "speedups": {}}
    for line in Path("/proc/cpuinfo").read_text().splitlines():
        if line.startswith("model name"):
            report["cpu"] = line.split(":", 1)[1].strip()
            break
    def run_sample(case, backend, label):
        out = a.work_dir / case / backend / label
        cmd = [sys.executable, str(script), "--backend", backend, "--case", case, "--output", str(out)]
        if backend == "rust":
            cmd += ["--rust-bin", str(binary), "--data-dir", str(data_dir)]
        return measure(cmd, out, timeout=1800)
    report["sample_order"] = "interleaved: warmups per backend, then alternating backend order per repetition"
    for case in a.cases:
        report["cases"][case] = {}
        for backend in ["python", "rust"]:
            for w in range(a.warmup):
                run_sample(case, backend, f"warmup{w}")
        samples = {"python": [], "rust": []}
        for i in range(a.repeat):
            for backend in (["python", "rust"] if i % 2 == 0 else ["rust", "python"]):
                samples[backend].append(run_sample(case, backend, f"sample{i}"))
        for backend in ["python", "rust"]:
            t = [s["wall_seconds"] for s in samples[backend]]
            rss = [s["sampled_peak_tree_rss_bytes"] for s in samples[backend] if s["sampled_peak_tree_rss_bytes"]]
            report["cases"][case][backend] = {"median_seconds": statistics.median(t), "min_seconds": min(t), "max_seconds": max(t), "median_sampled_peak_tree_rss_bytes": statistics.median(rss) if rss else None, "samples": samples[backend]}
            print(f"{case}/{backend}: median {statistics.median(t):.3f}s", flush=True)
        report["cases"][case]["consistency"] = check(case, a.work_dir / case / "rust" / "sample0", a.work_dir / case / "python" / "sample0")
        report["speedups"][case] = report["cases"][case]["python"]["median_seconds"] / report["cases"][case]["rust"]["median_seconds"]
    a.report.parent.mkdir(parents=True, exist_ok=True)
    a.report.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
