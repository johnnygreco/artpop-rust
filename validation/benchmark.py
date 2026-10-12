"""Measure equivalent Python/Rust agent workflows in fresh subprocesses.

Output reports wall time and sampled Linux process-tree RSS. It never infers a
Rust speedup from a Python-only run. Input generation and checking are untimed.
"""
import argparse
import importlib.metadata
import json
import os
from pathlib import Path
import platform
import signal
import statistics
import subprocess
import sys
import time

from harness import CASES, ROOT, dump, prepare


def sampled_tree_rss(pid):
    """Linux RSS sum for the live process and descendants, not allocator bytes.

    Sampling can miss short peaks and sums shared pages more than once. Return
    None on other systems; do not label it an exact high-water mark.
    """
    if sys.platform != "linux": return None
    pending=[pid]
    seen=set()
    total=0
    while pending:
        current=pending.pop()
        if current in seen: continue
        seen.add(current)
        try:
            status=Path(f"/proc/{current}/status").read_text()
            for line in status.splitlines():
                if line.startswith("VmRSS:"):
                    total+=int(line.split()[1])*1024
            for task in Path(f"/proc/{current}/task").iterdir():
                pending.extend(int(x) for x in (task/"children").read_text().split())
        except (FileNotFoundError,ProcessLookupError,PermissionError):
            pass
    return total


def loadavg():
    """1-minute system load average (Linux), recorded to expose contention."""
    try:
        return float(Path("/proc/loadavg").read_text().split()[0])
    except (OSError, ValueError, IndexError):
        return None


def measure(command, output, timeout=300):
    output.mkdir(parents=True,exist_ok=True)
    load_before=loadavg()
    with (output/"worker.stdout").open("w") as stdout, (output/"worker.stderr").open("w") as stderr:
        start=time.perf_counter()
        child=subprocess.Popen(command,stdout=stdout,stderr=stderr,start_new_session=(os.name=="posix"))
        peak=None
        while child.poll() is None:
            rss=sampled_tree_rss(child.pid)
            if rss is not None: peak=max(peak or 0,rss)
            if time.perf_counter()-start>timeout:
                if os.name=="posix": os.killpg(child.pid,signal.SIGKILL)
                else: child.kill()
                child.wait()
                raise TimeoutError(f"Benchmark exceeded {timeout}s: {command}")
            time.sleep(0.01)
        elapsed=time.perf_counter()-start
    if child.returncode:
        raise RuntimeError(f"Worker failed ({child.returncode}): {(output/'worker.stderr').read_text()[-2000:]}")
    return {"wall_seconds":elapsed,"sampled_peak_tree_rss_bytes":peak,"load_average_1m_before":load_before,"load_average_1m_after":loadavg()}


def main():
    p=argparse.ArgumentParser()
    p.add_argument("--backend",choices=["python","rust","both"],default="python")
    p.add_argument("--rust-bin",type=Path)
    p.add_argument("--work-dir",type=Path,required=True)
    p.add_argument("--report",type=Path,required=True)
    p.add_argument("--count",type=int,default=100000)
    p.add_argument("--repeat",type=int,default=5)
    p.add_argument("--warmup",type=int,default=1)
    p.add_argument("--cases",nargs="+",choices=CASES,default=list(CASES))
    args=p.parse_args()
    if args.count<10000 or args.repeat<2 or args.warmup<0: p.error("Require count>=10000, repeat>=2, warmup>=0")
    if args.backend in ["rust","both"] and (not args.rust_bin or not args.rust_bin.is_file()):
        p.error("Rust measurement requires --rust-bin pointing to a real implementation")
    args.work_dir=args.work_dir.resolve()
    if args.work_dir.exists() and any(args.work_dir.iterdir()): p.error("Use an empty work directory to avoid reusing artifacts/cache hits")
    args.work_dir.mkdir(parents=True,exist_ok=True)
    inputs=prepare(args.work_dir/"inputs",args.count)
    harness=Path(__file__).with_name("harness.py")
    backends=["python","rust"] if args.backend=="both" else [args.backend]
    report={"schema_version":"1","measurement":"fresh-process end-to-end workflows including input parsing, initialization, compute, and output export; input generation and validation excluded", "memory_method":"10ms Linux process-tree RSS sampling; shared pages can be double-counted; short peaks can be missed; null on other platforms", "python":sys.version,"platform":platform.platform(),"machine":platform.machine(),"cpu":platform.processor(),"logical_cpus":os.cpu_count(),"upstream_commit":subprocess.check_output(["git","-C",str(ROOT/"ArtPop"),"rev-parse","HEAD"],text=True).strip(),"packages":{x:importlib.metadata.version(x) for x in ["numpy","scipy","astropy","fast-histogram"]},"count":args.count,"warmup":args.warmup,"repeat":args.repeat,"input_manifest":json.loads((inputs/"manifest.json").read_text()),"rust_binary":str(args.rust_bin.resolve()) if args.rust_bin else None,"cases":{},"speedups":None}
    if sys.platform=="linux":
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            if line.startswith("model name"):
                report["cpu"]=line.split(":",1)[1].strip(); break
    if args.rust_bin:
        import hashlib
        report["rust_binary_sha256"]=hashlib.sha256(args.rust_bin.read_bytes()).hexdigest()
        report["rust_version"]=subprocess.check_output([str(args.rust_bin.resolve()),"--version"],text=True).strip()
    def run_sample(case, backend, label):
        run_dir=args.work_dir/case/backend/label
        command=[sys.executable,str(harness),"run","--backend",backend,"--case",case,"--inputs",str(inputs),"--output",str(run_dir)]
        if args.rust_bin: command.extend(["--rust-bin",str(args.rust_bin.resolve())])
        result=measure(command,run_dir)
        check_command=[sys.executable,str(harness),"check","--case",case,"--inputs",str(inputs),"--output",str(run_dir)]
        if backend=="rust" and "python" in backends:
            # Warmups validate against the Python warmup; samples against Python sample0.
            ref="sample0" if label.startswith("sample") else "warmup0"
            check_command.extend(["--reference",str(args.work_dir/case/"python"/ref)])
        check=subprocess.run(check_command,capture_output=True,text=True,timeout=300)
        if check.returncode: raise RuntimeError(f"Validation failed for {case}/{backend}: {check.stderr}")
        result["validation"]=json.loads(check.stdout)
        result["output_bytes"]=sum(x.stat().st_size for x in run_dir.rglob("*") if x.is_file())
        return result
    # Warmups first, then measured samples with alternating backend order
    # (python, rust, rust, python, ...) so background load affects both alike.
    report["sample_order"]="interleaved: warmups per backend, then alternating backend order per repetition"
    for case in args.cases:
        report["cases"][case]={}
        for backend in backends:
            for w in range(args.warmup):
                run_sample(case,backend,f"warmup{w}")
        samples={b:[] for b in backends}
        if "python" in backends and "rust" in backends:
            # Python sample0 is the reference for Rust checks, so it runs first.
            samples["python"].append(run_sample(case,"python","sample0"))
            samples["rust"].append(run_sample(case,"rust","sample0"))
            for i in range(1,args.repeat):
                order=backends if i%2==0 else list(reversed(backends))
                for backend in order:
                    samples[backend].append(run_sample(case,backend,f"sample{i}"))
        else:
            for i in range(args.repeat):
                for backend in backends:
                    samples[backend].append(run_sample(case,backend,f"sample{i}"))
        for backend in backends:
            times=[s["wall_seconds"] for s in samples[backend]]
            rss=[s["sampled_peak_tree_rss_bytes"] for s in samples[backend] if s["sampled_peak_tree_rss_bytes"] is not None]
            report["cases"][case][backend]={"median_seconds":statistics.median(times),"min_seconds":min(times),"max_seconds":max(times),"median_sampled_peak_tree_rss_bytes":statistics.median(rss) if rss else None,"samples":samples[backend]}
            print(f"{case}/{backend}: median {statistics.median(times):.3f}s",flush=True)
    if args.backend=="both":
        report["speedups"]={case: report["cases"][case]["python"]["median_seconds"]/report["cases"][case]["rust"]["median_seconds"] for case in args.cases}
    args.report.parent.mkdir(parents=True,exist_ok=True)
    dump(args.report,report)


if __name__=="__main__":main()
