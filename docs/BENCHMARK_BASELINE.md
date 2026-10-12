# Python baseline and future Rust comparison

> **Superseded by [BENCHMARK_RESULTS.md](BENCHMARK_RESULTS.md)**, which compares these workflows (and larger ones) against the implemented Rust binary. This page records the original Python-only baseline.

The runnable [benchmark harness](../validation/README.md) was executed on 2026-10-12 against pinned ArtPop 0.1.2. All five workflows passed their correctness checks for every measured sample. **No Rust implementation was measured; no speedup has been established.**

Environment: AMD EPYC 7B13, four logical CPUs exposed, Linux x86_64, Python 3.10.12, NumPy 2.2.6, SciPy 1.15.3, Astropy 6.1.7, fast-histogram 0.14. Complete metadata, input hashes, and samples are in [python-benchmark.json](evidence/python-benchmark.json).

Each case uses 100,000 stars/draws, one warmup and three measured repetitions. Image cases use a non-square 201×101 frame; the grid case uses asymmetric 7×7 weights. The population case uses the upstream 1,467-row LSST isochrone fixture through a shared CSV representation.

| End-to-end workflow | Median wall time | Measured range | Median sampled peak RSS |
| --- | ---: | ---: | ---: |
| Catalog → count histogram → FITS | 1.255 s | 1.247–1.286 s | 135.6 MiB |
| Catalog → Moffat PSF → ideal image → FITS | 1.274 s | 1.274–1.298 s | 139.8 MiB |
| Catalog → noisy detector observation → three FITS planes | 1.303 s | 1.268–1.303 s | 138.4 MiB |
| Isochrone → sampled SSP → stellar CSV and photometry | 1.534 s | 1.532–1.539 s | 143.8 MiB |
| Weighted grid → sampled positions → CSV | 1.334 s | 1.311–1.390 s | 136.0 MiB |

These are **fresh-process workflow measurements**, including startup/import, input parsing, calculation, and export. They are not timings of histogram kernels or evidence that the operations have similar compute costs. Input generation and correctness checks are outside the timed region. The memory metric samples process-tree RSS every 10 ms, can miss short peaks, and can double-count shared pages. Three repetitions provide a preliminary local baseline, not a publication-quality confidence interval.

Once the Rust CLI exists, run `validation/benchmark.py --backend both --rust-bin ...` with a fresh work directory. The harness executes the same workflow inputs, validates deterministic signals and stochastic invariants, retains raw samples, and calculates `median_python_seconds / median_rust_seconds` for each case. Ratios below one must be reported as slowdowns. Do not compare a Python whole workflow against only a Rust inner loop.

For a full performance report, also implement the kernel and persistent-MCP benchmarks specified in [VALIDATION_PLAN.md](VALIDATION_PLAN.md), increase repetitions, interleave backend ordering, record compilation flags and native library versions, and measure multiple catalog/grid/image sizes. Separate improvements due to startup/interface costs, parsing, sampling, histogram accumulation, PSF convolution, and output writing.

The E2E test run recorded [9 passes and 5 explicit Rust skips](evidence/e2e-tests.txt): five Python workflows plus four deliberate-corruption checks. The skipped cases become real Python/Rust comparisons when a target binary is supplied. `--require-rust` prevents those skips from satisfying the eventual release gate.
