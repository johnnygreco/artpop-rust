# Benchmark results: ArtPop 0.1.2 vs artpop-rust

Measured on 2026-10-13 with [`validation/benchmark.py`](../validation/benchmark.py) (five reference workflows at 100,000 stars) and [`validation/scale_benchmark.py`](../validation/scale_benchmark.py) (six larger workflows, including one that is compute-bound in Python). Raw samples: [rust-vs-python-benchmark.json](evidence/rust-vs-python-benchmark.json), [rust-vs-python-scale-benchmark.json](evidence/rust-vs-python-scale-benchmark.json). The earlier optimization rounds' reports are kept as [round1-rust-vs-python-benchmark.json](evidence/round1-rust-vs-python-benchmark.json), [round1-rust-vs-python-scale-benchmark.json](evidence/round1-rust-vs-python-scale-benchmark.json), and the `round2-*` counterparts.

## Results

Environment: AMD EPYC 7B13, 4 logical CPUs, Linux 6.8 (x86_64, glibc 2.35, transparent huge pages in `madvise` mode). Python 3.10.12 with NumPy 2.2.6, SciPy 1.15.3, Astropy 6.1.7, fast-histogram 0.14, and ArtPop at `38a8869`. Rust: `artpop-rust 0.1.0` release build (thin LTO), binary SHA-256 `452e8d243685696f9d3b3b0c1cf6c0a1c4bf32ac4b7532f8627abd69e236401c`: the same `cargo build --release` binary for both reports, the Rust test and parity evidence, and the bit-identity check, which found every round-1 artifact ID and exported file reproduced byte for byte. The run started at a 1-minute load of 1.10 (the parity suite had just finished); the load averages below were sampled during the run and mostly reflect the benchmark's own processes. The MIST cutoff case's samples ran while the previous case's processes were still being reaped and both backends measured about 15% slower there than in round 2, so its row should be read as a ratio.

### Five reference workflows (100,000 stars or draws)

| Workflow | Python median (range) | Rust median (range) | Speedup | Python RSS MiB | Rust RSS MiB | Load avg during samples |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Catalog (100k) → count histogram → FITS | 1.266 s (1.254–1.295) | 0.074 s (0.073–0.075) | 17.2× | 136 | 33 | 1.06–1.09 |
| Catalog (100k) → Moffat PSF → ideal image → FITS | 1.302 s (1.269–1.521) | 0.095 s (0.095–0.095) | 13.8× | 140 | 33 | 1.04–1.06 |
| Catalog (100k) → detector noise → three FITS planes | 1.304 s (1.255–1.347) | 0.105 s (0.104–0.105) | 12.4× | 139 | 33 | 0.96–1.24 |
| Isochrone → 100k-star SSP → CSV + photometry | 1.545 s (1.527–1.585) | 0.117 s (0.116–0.127) | 13.2× | 145 | 33 | 1.26–1.52 |
| 7×7 weights → 100k positions → CSV | 1.358 s (1.327–1.375) | 0.073 s (0.073–0.075) | 18.5× | 136 | 26 | 1.34–1.40 |

Medians of 5 samples after 1 warmup per backend. All 50 measured samples (and the warmups) passed `harness.py check`: the Rust histogram is identical to ArtPop's, the ideal image agrees to 7.3e-16 relative L1, and the detector, population, and grid outputs pass their statistical checks (for example, a KS p-value of 0.87 for Rust masses and a χ² p-value of 0.24 for Rust grid counts). A failed check would have aborted the run.

### Workflows at scale

| Workflow | Python median (range) | Rust median (range) | Speedup | Python RSS MiB | Rust RSS MiB | Load avg during samples |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1M-star SSP → photometry | 1.695 s (1.664–1.732) | 0.154 s (0.152–0.178) | 11.0× | 196 | 56 | 1.31–1.33 |
| 1M-star SSP → Sérsic source → 801² ideal image (41² PSF) → FITS | 2.109 s (2.030–2.184) | 0.311 s (0.299–0.318) | 6.8× | 328 | 91 | 1.28–1.50 |
| 1M-star SSP → Plummer source → 2047² detector image (41² PSF) → 3 FITS planes | 3.937 s (3.822–4.209) | 0.661 s (0.642–0.683) | 6.0× | 915 | 225 | 1.81–1.96 |
| 10M uniform positions → 2047² histogram → FITS | 1.840 s (1.729–1.841) | 0.510 s (0.466–0.512) | 3.6× | 433 | 295 | 1.69–1.75 |
| MIST LSST 10 Gyr, 1e7 Msun, cutoff 29 mag → photometry | 2.302 s (1.969–2.527) | 0.185 s (0.147–0.200) | 12.4× | 278 | 63 | 1.95–2.04 |
| MIST LSST 10 Gyr, 1e7 Msun fully sampled (2.3e7 stars) → photometry | 21.356 s (19.906–22.188) | 3.300 s (2.988–6.302) | 6.5× | 2883 | 2267 | 2.21–3.07 |

Medians of 3 samples after 1 warmup per backend. The geometric-mean speedup over all 11 workflows is **9.9×** (10.4× over the ten workflows that were also measured in round 1, up from 7.7×; round 2 reached 9.5×). Peak RSS was lower for Rust in every workflow.

The last workflow is the compute-bound reference. Without a magnitude cutoff, `MISTSSP(total_mass=1e7)` draws about 2.3e7 stars in mass batches and interpolates nine columns (six LSST bands, EEP, log L, log Teff) for every star, so ArtPop spends 19 s in NumPy and SciPy after its 1.2 s of imports. Rust spends 2.2 s in `population.build` (1.1 s sampling and interpolating, 1.1 s content-hashing and writing the 2.2 GB of population columns) and 0.6 s in `population.photometry` (reading and verifying two 190 MB band columns in parallel and evaluating 4.9e7 powers).

Scale consistency checks (Rust against Python; the backends draw independent realizations, so the bounds are 6σ of the realization scatter predicted from the isochrone's flux moments):

| Workflow | Metric | Value |
| --- | --- | --- |
| 1M-star SSP | z-score of total flux against the IMF expectation, g / i | Rust −1.46 / −1.50; Python −1.25 / −1.50 |
| Sérsic ideal image | Rust/Python image-sum ratio (σ of a ratio ≈ 0.044) | 0.929 (−1.6σ) |
| 2047² detector image | Rust/Python expected source counts (σ ≈ 0.044); sky counts | 1.106 (+2.4σ); sky equal to 1e-9 |
| 10M histogram | Rust/Python deposited counts | 1.00003 |
| MIST mass cutoff | Rust − Python total magnitude, g / i | −0.004 / −0.013 mag |
| MIST fully sampled | Rust/Python mean flux per star, g / i (σ of a ratio ≈ 0.0055 / 0.0090); realized mass ratio | 1.012 (+2.1σ) / 1.019 (+2.1σ); 1.050 (whole-batch overshoot, bound 1.06) |

### Fixed costs

The Python adapter that drives the Rust CLI (`harness.py`, startup only) takes a median of 0.04 s and is included in every Rust time above. A bare `artpop-rust --version` takes under 5 ms, and a metadata-only operation about 4 ms including process start. Importing ArtPop and its dependencies takes about 1.2 s.

## Method

- **Fresh processes.** Each sample runs a complete user-level workflow in a new process tree, including interpreter or process startup, input parsing, computation, and output files. Python calls ArtPop's public API. Rust is driven through its public CLI by the same Python adapter ([`harness.py`](../validation/harness.py), [`scale_cases.py`](../validation/scale_cases.py)), so Rust times include that adapter process, one `artpop-rust` process per operation, and artifact persistence and digest verification between calls. These measure agent-style workflows, not kernels.
- **Interleaving.** After one warmup per backend, measured samples alternate backend order (Python, Rust; Rust, Python; …), so background load affects both alike. Each sample records the 1-minute system load average before and after, and the load column shows the range observed during each case's samples.
- **Checks.** Every Python and Rust sample of the five reference workflows is validated by `harness.py check` (Rust against the Python reference). Scale cases are cross-checked statistically, because the two backends draw independent realizations. For the fully sampled MIST case both backends overshoot the target mass by up to one batch (5%), so its check compares the mean flux per star with the scatter predicted from the MIST isochrone's flux moments rather than total magnitudes. Reported consistency metrics are in the JSON reports.
- **Memory** is the peak sum of process-tree RSS sampled every 10 ms. It can miss short peaks, counts shared pages more than once, and includes the Python adapter for Rust.
- **Medians** of 5 samples (reference workflows) or 3 samples (scale), with min–max ranges. These are local measurements on one machine, not publication-grade confidence intervals.

## Where the time goes (Rust)

Per-operation `metrics.elapsed_ms` and stage `timings_ms` are reported in every result envelope, so costs can be inspected without a profiler.

### First optimization round

Profiling (`perf`) of the first implementation identified six bottlenecks, each optimized without changing results. Before/after times are for single CLI calls on the scale workloads.

| Change | Effect |
| --- | --- |
| Tiled overlap-save FFT convolution (256²/512² blocks, kernel spectrum computed once, two real tiles per complex FFT, parallel tiles) | 2047² image with 41² PSF: 492 → 84 ms (matches direct summation within 1e-12 relative) |
| Random draws in fixed 65,536-element blocks, each with its own derived stream, generated in parallel | Results independent of thread count; population build 320 → ~100 ms for 1M stars |
| Guide-table search for the tabulated IMF inverse; one bracket search per star shared by all interpolated columns | Bit-identical outputs |
| Stable tile-bucketed parallel histogram | 10M stars into 2047²; output bitwise identical to serial accumulation |
| Parallel member hashing and loading; artifacts take ownership of f64 planes (no serialization copy); identity/constant ID columns implied instead of stored | `observe.artificial` 2047²: 1580 → 380 ms; source and positions artifacts ~25% smaller |
| Metadata-only `population.inspect` | 81 → 0.3 ms for 1M stars |

### Second optimization round

Profiling (`perf`, in-process `metrics.elapsed_ms`, and stage `timings_ms`) of the round-1 binary showed that the remaining time was mostly outside the numerical kernels: SHA-256 hashing of artifact members (inherent to content addressing, about 1.3 GB/s per core with SHA-NI), kernel page faults while first touching large fresh buffers, CFITSIO's buffered writes plus a full re-read of each exported FITS file for its digest, serial text parsing and formatting, random gathers in the tiled histogram, and operations that loaded every column of an artifact when one band was needed. Each change below keeps every artifact ID and exported file byte-identical to round 1 (verified for all ten workflows), so none of them alters results.

| Change | Where |
| --- | --- |
| Transparent huge pages are advised (`madvise(MADV_HUGEPAGE)`, Linux only, a hint) for buffers above 4 MiB; page faults for a 160 MB buffer drop from about 40,000 to about 80. | `artpop_core::mem`, used by image, sampler, interpolation, and store buffers |
| Artifact members are hashed and written in one parallel pass; member reads verify the digest while the next chunk is read. | `store.rs` |
| Populations move their columns into the artifact instead of copying them; positions and sources skip identity index columns entirely. | `codec.rs`, `population.rs`, `spatial.rs` |
| `population.photometry`, `observe.*`, `image.histogram`, and CSV export read only the columns they use (one band instead of all six; no masses for photometry). | `codec::LazyPopulation`, `codec::load_source_with` |
| FITS images are encoded natively (CFITSIO's exact header layout and `%.15G` formatting, tested byte-for-byte against CFITSIO) and hashed from the same buffer; no re-read. | `artpop_io::fits::encode_image` |
| CSV catalogs are parsed in newline-aligned chunks on all cores; CSV exports format typed columns in parallel blocks without intermediate JSON values and are hashed while written. | `artpop_io::csv` |
| The tiled histogram scatters stars into per-(block, tile) windows in parallel and accumulates from sequential sorted arrays instead of random `cells[i]` gathers; the counting sort is parallel per block. Output order per pixel is still input order. | `image::deposit` |
| Pairwise sums split work across threads with the identical addition tree. | `image::pairwise_sum` |
| Overlap-save convolution reuses per-thread FFT buffers, gathers rows by slice copies instead of per-pixel modular indexing, uses a blocked transpose, and writes tiles straight into the output. | `convolve::fft_tiled` |
| Flux/count conversion of stars, finiteness checks, sky addition, expected-count sanitization, and smooth-light evaluation run in parallel (sequential accumulation where the order matters). | `imager.rs`, `observe.rs`, `image.rs` |
| Position samplers fill `x` and `y` directly from the same block streams instead of a tuple array plus unzip. | `spatial.rs`, `rng::par_blocks2` |
| MIST files are digest-verified concurrently with parsing. | `data::read_verified_parse` |
| The PTRS Poisson sampler uses an inline exact floor instead of a library call. | `rng.rs` |

A fat-LTO, single-codegen-unit build was measured against the thin-LTO release profile and was within noise (at most 4% on one operation), so the profile is unchanged.

Per-operation in-process medians (`metrics.elapsed_ms`, the same requests in both rounds; wall times above add process start and the adapter):

| Workflow | Operation | Round 1 (ms) | Round 2 (ms) |
| --- | --- | ---: | ---: |
| detector | source.build | 32.9 | 25.4 |
| detector | observe.artificial | 10.4 | 8.8 |
| detector | artifact.export | 2.1 | 1.6 |
| detector | artifact.export | 2.0 | 1.6 |
| detector | artifact.export | 2.0 | 1.7 |
| detector | observation.inspect | 0.2 | 0.2 |
| detector | **sum of operations** | **49.6** | **39.3** |
| grid | spatial.grid-build | 1.1 | 1.1 |
| grid | spatial.sample | 5.9 | 5.0 |
| grid | artifact.export | 29.4 | 10.9 |
| grid | **sum of operations** | **36.3** | **17.0** |
| histogram | image.histogram | 29.2 | 22.5 |
| histogram | artifact.export | 2.0 | 1.6 |
| histogram | **sum of operations** | **31.3** | **24.1** |
| ideal | source.build | 32.1 | 24.9 |
| ideal | psf.build | 0.9 | 1.0 |
| ideal | observe.ideal | 13.6 | 11.7 |
| ideal | artifact.export | 2.1 | 1.6 |
| ideal | **sum of operations** | **48.6** | **39.2** |
| population | isochrone.import | 1.9 | 2.1 |
| population | population.build | 21.2 | 20.2 |
| population | artifact.export | 63.6 | 31.8 |
| population | population.photometry | 8.0 | 7.0 |
| population | population.inspect | 0.3 | 0.3 |
| population | **sum of operations** | **95.0** | **61.3** |
| artificial_2k | isochrone.import | 3.1 | 3.1 |
| artificial_2k | population.build | 96.1 | 65.6 |
| artificial_2k | source.build | 135.9 | 89.6 |
| artificial_2k | psf.build | 1.0 | 1.1 |
| artificial_2k | observe.artificial | 445.8 | 285.5 |
| artificial_2k | artifact.export | 138.7 | 69.2 |
| artificial_2k | artifact.export | 142.7 | 65.8 |
| artificial_2k | artifact.export | 137.6 | 66.8 |
| artificial_2k | **sum of operations** | **1100.9** | **646.7** |
| mist_mass_cutoff | population.build | 93.4 | 85.9 |
| mist_mass_cutoff | population.photometry | 5.6 | 3.0 |
| mist_mass_cutoff | **sum of operations** | **99.0** | **88.9** |
| population_1m | isochrone.import | 2.8 | 3.0 |
| population_1m | population.build | 97.0 | 66.3 |
| population_1m | population.photometry | 68.0 | 41.1 |
| population_1m | population.inspect | 0.3 | 0.4 |
| population_1m | **sum of operations** | **168.1** | **110.8** |
| sersic_ideal_1m | isochrone.import | 3.0 | 2.9 |
| sersic_ideal_1m | population.build | 96.4 | 64.2 |
| sersic_ideal_1m | source.build | 155.5 | 107.0 |
| sersic_ideal_1m | psf.build | 1.2 | 0.9 |
| sersic_ideal_1m | observe.ideal | 99.5 | 62.3 |
| sersic_ideal_1m | artifact.export | 22.0 | 14.4 |
| sersic_ideal_1m | **sum of operations** | **377.6** | **251.7** |
| uniform_hist_10m | spatial.sample | 358.8 | 154.7 |
| uniform_hist_10m | image.histogram | 615.8 | 264.0 |
| uniform_hist_10m | artifact.export | 148.6 | 68.3 |
| uniform_hist_10m | **sum of operations** | **1123.2** | **487.0** |

### Third optimization round

Stage timings (`timings_ms`, now also reported by `population.build` and `source.build`) and `perf` on the round-2 binary showed three further things worth changing. The VM has two physical cores with SMT (four vCPUs), so CPU-bound stages scale by about 2× at best, and its filesystem writes 8 MB in 7–21 ms even for `dd`, so file output is a shared floor for exports and the artifact store. All ten round-1 workflows still reproduce their artifact IDs and exported files byte for byte.

| Change | Where |
| --- | --- |
| Pixel-centers binning used `floor()` from libm per star (baseline x86-64 has no SSE4.1 rounding instruction); the range check already guarantees a non-negative argument, so a truncating cast gives the identical result. The classification loop is monomorphized per convention and reads block-local slices (no bounds checks); the cell buffer is filled without a memset pass. | `image::deposit` |
| Buffers that several threads then write at random (histogram image and scatter arrays, convolution output) are pre-faulted in one kernel call (`MADV_POPULATE_WRITE`); concurrent first-touch faults on the same huge pages were about ten times slower. Buffers filled sequentially per thread are left to fault on their own, which is faster than a serial prefault. | `mem::prefault`, `image.rs`, `convolve.rs` |
| FITS export streams the plane: each 8 MB chunk is digest-verified, byte-swapped, hashed, and written while the next chunk is read; the file is published only after the member's digest verifies. No 33 MB buffers. | `fits::ImageWriter`, `Loaded::stream_f64_bytes` |
| `population.photometry` resolves every requested band first and reads all of their columns in one parallel pass. | `codec::LazyPopulation::abs_mags_many` |
| Isochrone interpolation bounds each star's bracket search with a guide table over uniform mass bins (same row as a full binary search) and uses one precomputed IEEE slope per row. | `isochrone::MassGuide` |
| Identity star identifiers are implicit (`Catalog::star_id: Option`), never materialized on build or load; mass-normalized sampling pre-sizes its output from the first batch. | `population.rs`, `codec.rs`, `imf::build_to_mass` |
| CSV parsing resolves line numbers from the chunks' own line counts (no separate counting pass) and pre-sizes per-chunk vectors; CSV export formats rows in waves and writes and hashes each wave while the next is formatted. | `artpop_io::csv`, `artifact::write_atomic_streaming` |


Per-operation in-process medians, round 2 against round 3 (the same requests; `mist_mass_cutoff` ran under the elevated load noted above):

| Workflow | Operation | Round 2 (ms) | Round 3 (ms) |
| --- | --- | ---: | ---: |
| detector | source.build | 25.4 | 23.4 |
| detector | observe.artificial | 8.8 | 8.7 |
| detector | artifact.export | 1.6 | 1.6 |
| detector | artifact.export | 1.6 | 1.4 |
| detector | artifact.export | 1.7 | 1.5 |
| detector | observation.inspect | 0.2 | 0.3 |
| detector | **sum of operations** | **39.3** | **36.9** |
| grid | spatial.grid-build | 1.1 | 1.1 |
| grid | spatial.sample | 5.0 | 4.8 |
| grid | artifact.export | 10.9 | 11.8 |
| grid | **sum of operations** | **17.0** | **17.7** |
| histogram | image.histogram | 22.5 | 20.2 |
| histogram | artifact.export | 1.6 | 1.4 |
| histogram | **sum of operations** | **24.1** | **21.6** |
| ideal | source.build | 24.9 | 23.4 |
| ideal | psf.build | 1.0 | 0.9 |
| ideal | observe.ideal | 11.7 | 11.5 |
| ideal | artifact.export | 1.6 | 1.5 |
| ideal | **sum of operations** | **39.2** | **37.2** |
| population | isochrone.import | 2.1 | 1.8 |
| population | population.build | 20.2 | 17.3 |
| population | artifact.export | 31.8 | 26.5 |
| population | population.photometry | 7.0 | 5.4 |
| population | population.inspect | 0.3 | 0.3 |
| population | **sum of operations** | **61.3** | **51.3** |
| artificial_2k | isochrone.import | 3.1 | 2.8 |
| artificial_2k | population.build | 65.6 | 54.7 |
| artificial_2k | source.build | 89.6 | 81.9 |
| artificial_2k | psf.build | 1.1 | 1.0 |
| artificial_2k | observe.artificial | 285.5 | 277.7 |
| artificial_2k | artifact.export | 69.2 | 53.2 |
| artificial_2k | artifact.export | 65.8 | 51.2 |
| artificial_2k | artifact.export | 66.8 | 51.4 |
| artificial_2k | **sum of operations** | **646.7** | **573.9** |
| mist_full_1e7 | population.build | 2668.3 | 2605.6 |
| mist_full_1e7 | population.photometry | 672.9 | 592.6 |
| mist_full_1e7 | **sum of operations** | **3341.2** | **3198.2** |
| mist_mass_cutoff | population.build | 85.9 | 105.2 |
| mist_mass_cutoff | population.photometry | 3.0 | 3.6 |
| mist_mass_cutoff | **sum of operations** | **88.9** | **108.8** |
| population_1m | isochrone.import | 3.0 | 2.7 |
| population_1m | population.build | 66.3 | 57.1 |
| population_1m | population.photometry | 41.1 | 34.9 |
| population_1m | population.inspect | 0.4 | 0.3 |
| population_1m | **sum of operations** | **110.8** | **95.1** |
| sersic_ideal_1m | isochrone.import | 2.9 | 2.9 |
| sersic_ideal_1m | population.build | 64.2 | 55.3 |
| sersic_ideal_1m | source.build | 107.0 | 104.2 |
| sersic_ideal_1m | psf.build | 0.9 | 1.1 |
| sersic_ideal_1m | observe.ideal | 62.3 | 55.9 |
| sersic_ideal_1m | artifact.export | 14.4 | 14.7 |
| sersic_ideal_1m | **sum of operations** | **251.7** | **234.1** |
| uniform_hist_10m | spatial.sample | 154.7 | 143.6 |
| uniform_hist_10m | image.histogram | 264.0 | 243.0 |
| uniform_hist_10m | artifact.export | 68.3 | 50.1 |
| uniform_hist_10m | **sum of operations** | **487.0** | **436.7** |

Where the remaining time goes, for the operations that still dominate: `observe.artificial` at 2047² spends about 100 ms realizing Poisson and read noise for 4.2 million pixels (the PTRS and polar algorithms are frozen by the `rust-v1` profile), about 80 ms hashing and writing its five 33 MB planes, 38 ms in the convolution, and 18 ms in the deposit. Each 33 MB FITS export now streams in about 50 ms, of which most is this VM's file write throughput. The 10 million positions cost 144 ms to draw and store (SHA-256 of 160 MB on two cores sets the floor) and 243 ms to read, verify, and bin. For the fully sampled MIST population, content-hashing and writing 2.2 GB of columns (1.1 s) is the largest single cost, followed by interpolating eleven columns for 24.6 million stars (0.7 s).


## Interpretation

At 100,000 stars, both backends spend most of their time on fixed startup. ArtPop imports NumPy, SciPy, Astropy, and Matplotlib (about 1.2 s); each `artpop-rust` call starts in a few milliseconds, so those ratios largely measure interpreter startup. The scale workflows show compute and I/O costs. Rust is fastest where ArtPop parses text or runs Python-level loops (MIST tables, sampling, population bookkeeping). Its margin is smallest on the 10M-position histogram, whose two-step Rust workflow writes, re-reads, and digest-verifies 170 MB of positions between processes; a single-process `simulate.run` avoids that round trip. The fully sampled 1e7 Msun population is where ArtPop is genuinely slow (20 s of array work) and Rust is bounded by content-addressed storage of a 2.2 GB result; the 6× there is a measure of the engine plus its storage design, not of startup.

No speedup is claimed outside these workflows, this machine, and these sizes.
