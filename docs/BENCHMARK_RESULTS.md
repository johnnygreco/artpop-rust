# Benchmark results: ArtPop 0.1.2 vs artpop-rust

Measured on 2026-10-12 with [`validation/benchmark.py`](../validation/benchmark.py) (five reference workflows at 100,000 stars) and [`validation/scale_benchmark.py`](../validation/scale_benchmark.py) (six larger workflows, including one that is compute-bound in Python). Raw samples: [rust-vs-python-benchmark.json](evidence/rust-vs-python-benchmark.json), [rust-vs-python-scale-benchmark.json](evidence/rust-vs-python-scale-benchmark.json). The first optimization round's reports are kept as [round1-rust-vs-python-benchmark.json](evidence/round1-rust-vs-python-benchmark.json) and [round1-rust-vs-python-scale-benchmark.json](evidence/round1-rust-vs-python-scale-benchmark.json).

## Results

Environment: AMD EPYC 7B13, 4 logical CPUs, Linux 6.8 (x86_64, glibc 2.35, transparent huge pages in `madvise` mode). Python 3.10.12 with NumPy 2.2.6, SciPy 1.15.3, Astropy 6.1.7, fast-histogram 0.14, and ArtPop at `38a8869`. Rust: `artpop-rust 0.1.0` release build (thin LTO), binary SHA-256 `2acdfcb0d94d127c7d6beaf129da75f2fe742a7fdc237286275558b8fbc6398c`, the same binary for both reports. The final build (`2bd6a1f7d0f3e1f61b47be396ba2c80f9759e65f54f02a7b37d6cd0eeb0f0a2a`) differs from it only by the job-submission race fix in `jobs.rs` (see [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md#known-limitations)); the Rust test, parity, and bit-identity evidence were recorded on the final build, which reproduces every round-1 artifact ID and exported file byte for byte. (`cargo test --release` re-links that same source with different crate metadata; the plain `cargo build --release` output hashes to `b549543787e8a6bbe431967dbec5c8a79132056ab272f4e971fc3f5e3f73404e` and was also verified to reproduce all round-1 outputs.) The run started at a 1-minute load of 0.65 on the otherwise idle VM; the load averages below were sampled during the run and mostly reflect the benchmark's own processes.

### Five reference workflows (100,000 stars or draws)

| Workflow | Python median (range) | Rust median (range) | Speedup | Python RSS MiB | Rust RSS MiB | Load avg during samples |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Catalog (100k) → count histogram → FITS | 1.269 s (1.247–1.318) | 0.075 s (0.073–0.086) | 17.0× | 136 | 32 | 0.79–0.83 |
| Catalog (100k) → Moffat PSF → ideal image → FITS | 1.337 s (1.283–1.341) | 0.097 s (0.094–0.106) | 13.8× | 140 | 33 | 1.06–1.08 |
| Catalog (100k) → detector noise → three FITS planes | 1.294 s (1.293–1.314) | 0.107 s (0.106–0.109) | 12.1× | 139 | 32 | 1.53–1.86 |
| Isochrone → 100k-star SSP → CSV + photometry | 1.622 s (1.576–1.725) | 0.129 s (0.128–0.139) | 12.6× | 144 | 35 | 1.65–1.91 |
| 7×7 weights → 100k positions → CSV | 1.384 s (1.334–1.430) | 0.074 s (0.073–0.084) | 18.6× | 136 | 26 | 1.50–1.70 |

Medians of 5 samples after 1 warmup per backend. All 50 measured samples (and the warmups) passed `harness.py check`: the Rust histogram is identical to ArtPop's, the ideal image agrees to 7.3e-16 relative L1, and the detector, population, and grid outputs pass their statistical checks (for example, a KS p-value of 0.87 for Rust masses and a χ² p-value of 0.24 for Rust grid counts). A failed check would have aborted the run.

### Workflows at scale

| Workflow | Python median (range) | Rust median (range) | Speedup | Python RSS MiB | Rust RSS MiB | Load avg during samples |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1M-star SSP → photometry | 1.677 s (1.622–1.798) | 0.175 s (0.163–0.177) | 9.6× | 195 | 64 | 1.58–1.61 |
| 1M-star SSP → Sérsic source → 801² ideal image (41² PSF) → FITS | 2.066 s (2.065–2.097) | 0.318 s (0.318–0.324) | 6.5× | 329 | 105 | 1.52–1.61 |
| 1M-star SSP → Plummer source → 2047² detector image (41² PSF) → 3 FITS planes | 3.656 s (3.565–4.148) | 0.727 s (0.726–0.728) | 5.0× | 913 | 225 | 1.40–1.48 |
| 10M uniform positions → 2047² histogram → FITS | 1.828 s (1.799–1.833) | 0.539 s (0.484–0.575) | 3.4× | 433 | 288 | 1.56–1.61 |
| MIST LSST 10 Gyr, 1e7 Msun, cutoff 29 mag → photometry | 1.945 s (1.940–1.972) | 0.148 s (0.148–0.149) | 13.2× | 278 | 64 | 1.52–1.56 |
| MIST LSST 10 Gyr, 1e7 Msun fully sampled (2.3e7 stars) → photometry | 20.570 s (19.770–20.761) | 3.402 s (3.178–5.552) | 6.0× | 2881 | 2295 | 1.36–1.93 |

Medians of 3 samples after 1 warmup per backend. The geometric-mean speedup over all 11 workflows is **9.5×** (9.9× over the ten workflows that were also measured in round 1, up from 7.7×). Peak RSS was lower for Rust in every workflow.

The last workflow is the compute-bound reference. Without a magnitude cutoff, `MISTSSP(total_mass=1e7)` draws about 2.3e7 stars in mass batches and interpolates nine columns (six LSST bands, EEP, log L, log Teff) for every star, so ArtPop spends 19 s in NumPy and SciPy after its 1.2 s of imports. Rust spends 2.4 s in `population.build` (sampling, interpolation, and content-hashing plus writing the 2.2 GB of population columns) and 0.7 s in `population.photometry` (reading and verifying two 190 MB band columns and evaluating 4.9e7 powers).

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

Where the remaining time goes, for the operations that still dominate: `observe.artificial` at 2047² spends about 100 ms realizing Poisson and read noise for 4.2 million pixels (the PTRS and polar algorithms are frozen by the `rust-v1` profile), 90–115 ms hashing and writing its five 33 MB planes, 35 ms in the convolution, and 25 ms in the deposit. Each 33 MB FITS export reads and verifies the plane (about 35 ms), encodes it, and writes and hashes it concurrently (about 30 ms). The 10 million positions cost 155 ms to draw and store (SHA-256 of 160 MB on two cores sets the floor) and 265 ms to read, verify, and bin. For the fully sampled MIST population, content-hashing and writing 2.2 GB of columns is the largest single cost.

## Interpretation

At 100,000 stars, both backends spend most of their time on fixed startup. ArtPop imports NumPy, SciPy, Astropy, and Matplotlib (about 1.2 s); each `artpop-rust` call starts in a few milliseconds, so those ratios largely measure interpreter startup. The scale workflows show compute and I/O costs. Rust is fastest where ArtPop parses text or runs Python-level loops (MIST tables, sampling, population bookkeeping). Its margin is smallest on the 10M-position histogram, whose two-step Rust workflow writes, re-reads, and digest-verifies 170 MB of positions between processes; a single-process `simulate.run` avoids that round trip. The fully sampled 1e7 Msun population is where ArtPop is genuinely slow (20 s of array work) and Rust is bounded by content-addressed storage of a 2.2 GB result; the 6× there is a measure of the engine plus its storage design, not of startup.

No speedup is claimed outside these workflows, this machine, and these sizes.
