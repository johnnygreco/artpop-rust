# Reference comparisons and benchmarks

Development tooling that compares the `artpop-rust` binary with pinned ArtPop 0.1.2. The released CLI/MCP never runs Python; these scripts import the sibling `../ArtPop/src` checkout. To set it up next to this repository:

```bash
git clone https://github.com/ArtificialStellarPopulations/ArtPop ../ArtPop
git -C ../ArtPop checkout 38a886906ed175d0205bf48b6dc03f54aeb4a81a
```

A test without `--rust-bin` is skipped, and `--require-rust` turns a missing binary into a configuration error, so skips can never be mistaken for parity.

## Suites

| File | Contents |
| --- | --- |
| [`harness.py`](harness.py) + [`test_e2e.py`](test_e2e.py) | Five end-to-end workflows run by both backends on shared inputs (CSV catalog, isochrone, weights), plus four comparator self-tests that must reject deliberately corrupted outputs. |
| [`test_extended_parity.py`](test_extended_parity.py) | 27 checks of deterministic functions value-by-value and samplers statistically (see below). |
| [`benchmark.py`](benchmark.py) | Fresh-process timing of the five workflows at 100,000 stars, recording RSS and system load per sample. |
| [`scale_cases.py`](scale_cases.py) + [`scale_benchmark.py`](scale_benchmark.py) | Larger equivalent workflows (1M-star SSP, 1M-star Sérsic ideal image, 2047² detector image, 10M-star histogram, MIST mass-normalized cutoff population, and a fully sampled 1e7 Msun MIST population of about 23 million stars) with statistical cross-checks. |

The five workflows:

| Case | Path | Comparison |
| --- | --- | --- |
| `histogram` | CSV positions → unit-weight histogram (legacy bins) → FITS | Exact array equality |
| `ideal` | Catalog → source → Moffat PSF → ideal image (wrap) → FITS | Relative L1 ≤ 1e-9, per-pixel rtol 1e-9; flux conservation |
| `detector` | Catalog → instrumental imager → sky, Poisson, read noise → three FITS planes | Exact sky/calibration; standardized-residual moments |
| `population` | Isochrone CSV → count-normalized SSP → stellar CSV + photometry | Rust masses re-interpolated by ArtPop; remnant mass and flux/SBF accounting; two-sample KS on masses |
| `grid` | 7×7 asymmetric weights → probability grid → positions | Bounds, integer centers, zero-weight exclusion, chi-square |

Extended checks cover:

- IMF integrals and weights for all presets plus a custom broken power law, and SSP magnitudes, SBF, colors, mean mass, and surviving mass. Both are compared against ArtPop and against an independent 50-digit mpmath oracle.
- Magnitude limits, all-roots magnitude inversion, and every SciPy `interp1d` kind.
- All four PSF modes for Gaussian and Moffat profiles, and wrap/fill convolution (direct and FFT) against `astropy.convolution.convolve_fft`.
- Sérsic and Plummer smooth amplitudes; physical LSST counts, calibration, and sky at two pixel scales; the instrumental-zero-point sky correction (D04).
- All 164 filter properties and every AB/Vega/ST zero-point offset; custom filter curves against `FilterSystem`.
- Sample fractions, stars per pixel, MIST selection (bit-for-bit, on- and off-grid [Fe/H]), and Sérsic/Plummer position distributions (KS).
- Mag-limited population total and SBF magnitudes across seeds.

## Running

From `artpop-rust`, with the reference environment from [`requirements.txt`](requirements.txt):

```bash
cargo build --release
export MIST_PATH=/path/to/mist            # optional: enables the MIST comparisons
export ARTPOP_PARITY_DATA_DIR=$(mktemp -d)
python -m pytest validation -q --require-rust --rust-bin ./target/release/artpop-rust
python validation/benchmark.py --backend both --rust-bin ./target/release/artpop-rust --work-dir /tmp/bench --report bench.json --count 100000 --repeat 5 --warmup 1
python validation/scale_benchmark.py --rust-bin ./target/release/artpop-rust --work-dir /tmp/scale --report scale.json --repeat 3 --warmup 1
```

Results from 2026-10-12 are in [docs/evidence/](../docs/evidence/): [`e2e-rust-parity.txt`](../docs/evidence/e2e-rust-parity.txt), [`rust-vs-python-benchmark.json`](../docs/evidence/rust-vs-python-benchmark.json), and [`rust-vs-python-scale-benchmark.json`](../docs/evidence/rust-vs-python-scale-benchmark.json); summarized in [BENCHMARK_RESULTS.md](../docs/BENCHMARK_RESULTS.md).

## Rust adapter

`RustCli` in [`harness.py`](harness.py) runs `artpop-rust invoke OPERATION --input request.json --output-dir artifacts` and requires a single `status="completed"` envelope with exactly one artifact of the expected kind. Request files and raw stdout/stderr are kept for every call. Numeric comparisons always use exported FITS/CSV products. The adapter never falls back to Python.

Statistical thresholds are fixed in advance and conservative (p > 1e-8 for single tests, 6σ for scale realizations). Stochastic outputs are never compared sample-by-sample, because rust-v1 deliberately uses its own random streams.

The `mist_full_1e7` scale case is the compute-bound reference: without a magnitude cutoff ArtPop draws about 2.3e7 stars in mass batches and interpolates nine columns for each, which takes it about 20 s on the analysis VM, so the comparison measures NumPy/SciPy work rather than interpreter start-up. Both backends overshoot the target mass by up to one whole batch (5%), so its check compares the mean flux per star against the realization scatter predicted from the MIST isochrone's flux moments (about 0.4% in g and 0.6% in i at that size) rather than total magnitudes.
