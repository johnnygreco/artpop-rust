"""Render markdown tables from validation/benchmark.py and scale_benchmark.py reports.

Usage: python tools/benchmark_tables.py BASE_REPORT.json SCALE_REPORT.json
Prints a README summary table and the detailed BENCHMARK_RESULTS tables.
"""
import json
import statistics
import sys

LABELS = {
    "histogram": "Catalog (100k) → count histogram → FITS",
    "ideal": "Catalog (100k) → Moffat PSF → ideal image → FITS",
    "detector": "Catalog (100k) → detector noise → three FITS planes",
    "population": "Isochrone → 100k-star SSP → CSV + photometry",
    "grid": "7×7 weights → 100k positions → CSV",
    "population_1m": "1M-star SSP → photometry",
    "sersic_ideal_1m": "1M-star SSP → Sérsic source → 801² ideal image (41² PSF) → FITS",
    "artificial_2k": "1M-star SSP → Plummer source → 2047² detector image (41² PSF) → 3 FITS planes",
    "uniform_hist_10m": "10M uniform positions → 2047² histogram → FITS",
    "mist_mass_cutoff": "MIST LSST 10 Gyr, 1e7 Msun, cutoff 29 mag → photometry",
    "mist_full_1e7": "MIST LSST 10 Gyr, 1e7 Msun fully sampled (2.3e7 stars) → photometry",
}


def mib(v):
    return f"{v / 2**20:.0f}" if v else "n/a"


def loads(case):
    vals = [s.get(k) for b in ("python", "rust") for s in case[b]["samples"] for k in ("load_average_1m_before", "load_average_1m_after")]
    vals = [v for v in vals if v is not None]
    return (min(vals), max(vals)) if vals else (None, None)


def rows(report):
    out = []
    for name, c in report["cases"].items():
        p, r = c["python"], c["rust"]
        lo, hi = loads(c)
        out.append((name, p, r, p["median_seconds"] / r["median_seconds"], lo, hi))
    return out


def main():
    base, scale = (json.load(open(p)) for p in sys.argv[1:3])
    print("<!-- README summary -->")
    print("| Workflow | ArtPop 0.1.2 (Python) | artpop-rust | Speedup |")
    print("| --- | ---: | ---: | ---: |")
    for rep in (base, scale):
        for name, p, r, s, _, _ in rows(rep):
            print(f"| {LABELS[name]} | {p['median_seconds']:.2f} s | {r['median_seconds']:.3f} s | {s:.1f}× |")
    for title, rep in (("Five reference workflows (100,000 stars or draws)", base), ("Workflows at scale", scale)):
        print(f"\n### {title}\n")
        print("| Workflow | Python median (range) | Rust median (range) | Speedup | Python RSS MiB | Rust RSS MiB | Load avg during samples |")
        print("| --- | ---: | ---: | ---: | ---: | ---: | ---: |")
        for name, p, r, s, lo, hi in rows(rep):
            load = f"{lo:.2f}–{hi:.2f}" if lo is not None else "n/a"
            print(f"| {LABELS[name]} | {p['median_seconds']:.3f} s ({p['min_seconds']:.3f}–{p['max_seconds']:.3f}) | {r['median_seconds']:.3f} s ({r['min_seconds']:.3f}–{r['max_seconds']:.3f}) | {s:.1f}× | {mib(p['median_sampled_peak_tree_rss_bytes'])} | {mib(r['median_sampled_peak_tree_rss_bytes'])} | {load} |")
    gm = statistics.geometric_mean([x[3] for x in rows(base) + rows(scale)])
    print(f"\nGeometric-mean speedup over {len(rows(base)) + len(rows(scale))} workflows: {gm:.1f}×")
    for name, c in scale["cases"].items():
        print(f"- {name} consistency: {json.dumps(c.get('consistency'))}")


if __name__ == "__main__":
    main()
