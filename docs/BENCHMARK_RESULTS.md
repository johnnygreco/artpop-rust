# Benchmark results: ArtPop 0.1.2 vs artpop-rust

Measured on 2026-10-12 with [`validation/benchmark.py`](../validation/benchmark.py) (five reference workflows at 100,000 stars) and [`validation/scale_benchmark.py`](../validation/scale_benchmark.py) (larger workflows). Raw samples: [rust-vs-python-benchmark.json](evidence/rust-vs-python-benchmark.json), [rust-vs-python-scale-benchmark.json](evidence/rust-vs-python-scale-benchmark.json).

## Results

Environment: AMD EPYC 7B13, 4 logical CPUs, Linux 6.8 (x86_64, glibc 2.35). Python 3.10.12 with NumPy 2.2.6, SciPy 1.15.3, Astropy 6.1.7, fast-histogram 0.14, and ArtPop at `38a8869`. Rust: `artpop-rust 0.1.0` release build, binary SHA-256 `fe53d95e0fecc566e208991bf376ce744a5aaf33c6253a12693bf35610ffe0d9`, the same binary for both reports. The final build (`08bfee2126cdd42482f3b61456c34714dff7cd953d516bf904dcef893decb806`) adds only a lint attribute. Its `.text`, `.rodata`, and `.eh_frame` sections are byte-identical to the benchmarked binary; outside the symbol string table, the only difference is one panic-location line number. The parity evidence was recorded on the final build. The machine had been idle (1-minute load 0.03) when the run started. The load averages below were sampled during the run and mostly reflect the benchmark's own processes.

### Five reference workflows (100,000 stars or draws)

| Workflow | Python median (range) | Rust median (range) | Speedup | Python RSS MiB | Rust RSS MiB | Load avg during samples |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Catalog (100k) → count histogram → FITS | 1.308 s (1.295–1.330) | 0.084 s (0.083–0.094) | 15.6× | 136 | 31 | 0.24–0.45 |
| Catalog (100k) → Moffat PSF → ideal image → FITS | 1.330 s (1.275–1.389) | 0.105 s (0.104–0.119) | 12.7× | 140 | 32 | 0.54–1.26 |
| Catalog (100k) → detector noise → three FITS planes | 1.311 s (1.275–1.376) | 0.115 s (0.115–0.127) | 11.4× | 139 | 31 | 1.37–1.66 |
| Isochrone → 100k-star SSP → CSV + photometry | 1.566 s (1.558–1.611) | 0.160 s (0.157–0.171) | 9.8× | 144 | 60 | 1.34–1.51 |
| 7×7 weights → 100k positions → CSV | 1.370 s (1.345–1.394) | 0.095 s (0.094–0.095) | 14.4× | 136 | 39 | 1.32–1.41 |

Medians of 5 samples after 1 warmup per backend. All 50 measured samples (and the warmups) passed `harness.py check`: the Rust histogram is identical to ArtPop's, the ideal image agrees to 7.3e-16 relative L1, and the detector, population, and grid outputs pass their statistical checks (for example, a KS p-value of 0.87 for Rust masses and a χ² p-value of 0.24 for Rust grid counts). A failed check would have aborted the run.

### Workflows at scale

| Workflow | Python median (range) | Rust median (range) | Speedup | Python RSS MiB | Rust RSS MiB | Load avg during samples |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1M-star SSP → photometry | 1.659 s (1.637–1.683) | 0.228 s (0.227–0.229) | 7.3× | 196 | 94 | 1.49–1.53 |
| 1M-star SSP → Sérsic source → 801² ideal image (41² PSF) → FITS | 2.051 s (2.021–2.059) | 0.442 s (0.439–0.446) | 4.6× | 328 | 137 | 1.65–1.77 |
| 1M-star SSP → Plummer source → 2047² detector image (41² PSF) → 3 FITS planes | 3.748 s (3.563–4.109) | 1.194 s (1.168–1.211) | 3.1× | 913 | 270 | 1.51–1.60 |
| 10M uniform positions → 2047² histogram → FITS | 1.939 s (1.909–2.014) | 1.175 s (1.156–1.185) | 1.7× | 429 | 317 | 1.46–1.55 |
| MIST LSST 10 Gyr, 1e7 Msun, cutoff 29 mag → photometry | 1.965 s (1.928–2.097) | 0.157 s (0.147–0.170) | 12.5× | 278 | 63 | 1.39–1.43 |

Medians of 3 samples after 1 warmup per backend. The geometric-mean speedup over all 10 workflows is **7.7×**. Peak RSS was lower for Rust in every workflow.

Scale consistency checks (Rust against Python; the backends draw independent realizations, so the bounds are 6σ of the realization scatter predicted from the isochrone's flux moments):

| Workflow | Metric | Value |
| --- | --- | --- |
| 1M-star SSP | z-score of total flux against the IMF expectation, g / i | Rust −1.46 / −1.50; Python −1.25 / −1.50 |
| Sérsic ideal image | Rust/Python image-sum ratio (σ of a ratio ≈ 0.044) | 0.929 (−1.6σ) |
| 2047² detector image | Rust/Python expected source counts (σ ≈ 0.044); sky counts | 1.106 (+2.4σ); sky equal to 1e-9 |
| 10M histogram | Rust/Python deposited counts | 1.00003 |
| MIST mass cutoff | Rust − Python total magnitude, g / i | −0.004 / −0.013 mag |

### Fixed costs

The Python adapter that drives the Rust CLI (`harness.py`, startup only) takes a median of 0.04 s and is included in every Rust time above. A bare `artpop-rust --version` takes under 5 ms. Importing ArtPop and its dependencies takes about 1.2 s.

## Method

- **Fresh processes.** Each sample runs a complete user-level workflow in a new process tree, including interpreter or process startup, input parsing, computation, and output files. Python calls ArtPop's public API. Rust is driven through its public CLI by the same Python adapter ([`harness.py`](../validation/harness.py), [`scale_cases.py`](../validation/scale_cases.py)), so Rust times include that adapter process, one `artpop-rust` process per operation, and artifact persistence and digest verification between calls. These measure agent-style workflows, not kernels.
- **Interleaving.** After one warmup per backend, measured samples alternate backend order (Python, Rust; Rust, Python; …), so background load affects both alike. Each sample records the 1-minute system load average before and after, and the load column shows the range observed during each case's samples. An earlier run was disturbed by an unrelated build on this shared VM, so the reported run was started only after the machine had been idle for two minutes.
- **Checks.** Every Python and Rust sample of the five reference workflows is validated by `harness.py check` (Rust against the Python reference). Scale cases are cross-checked statistically, because the two backends draw independent realizations. Reported consistency metrics are in the JSON reports.
- **Memory** is the peak sum of process-tree RSS sampled every 10 ms. It can miss short peaks, counts shared pages more than once, and includes the Python adapter for Rust.
- **Medians** of 5 samples (reference workflows) or 3 samples (scale), with min–max ranges. These are local measurements on one machine, not publication-grade confidence intervals.

## Where the time goes (Rust)

Per-operation `metrics.elapsed_ms` and stage `timings_ms` are reported in every result envelope, so costs can be inspected without a profiler. Profiling (`perf`) of the first implementation identified six bottlenecks, each optimized without changing results. Before/after times are for single CLI calls on the scale workloads.

| Change | Effect |
| --- | --- |
| Tiled overlap-save FFT convolution (256²/512² blocks, kernel spectrum computed once, two real tiles per complex FFT, parallel tiles) | 2047² image with 41² PSF: 492 → 84 ms (matches direct summation within 1e-12 relative) |
| Random draws in fixed 65,536-element blocks, each with its own derived stream, generated in parallel | Results independent of thread count; population build 320 → ~100 ms for 1M stars |
| Guide-table search for the tabulated IMF inverse; one bracket search per star shared by all interpolated columns | Bit-identical outputs |
| Stable tile-bucketed parallel histogram | 10M stars into 2047²; output bitwise identical to serial accumulation |
| Parallel member hashing and loading; artifacts take ownership of f64 planes (no serialization copy); identity/constant ID columns implied instead of stored | `observe.artificial` 2047²: 1580 → 380 ms; source and positions artifacts ~25% smaller |
| Metadata-only `population.inspect` | 81 → 0.3 ms for 1M stars |

## Interpretation

At 100,000 stars, both backends spend most of their time on fixed startup. ArtPop imports NumPy, SciPy, Astropy, and Matplotlib (about 1.2 s); each `artpop-rust` call starts in a few milliseconds, so those ratios largely measure interpreter startup. The scale workflows show compute and I/O costs. Rust is fastest where ArtPop parses text or runs Python-level loops (MIST tables, sampling, population bookkeeping). Its margin is smallest on the 10M-position histogram, whose two-step Rust workflow writes, re-reads, and digest-verifies 170 MB of positions between processes. A single-process `simulate.run` avoids that round trip.

No speedup is claimed outside these workflows, this machine, and these sizes.
