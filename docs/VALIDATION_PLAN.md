# Validation and delivery plan

**Status (2026-10-12):** the Rust port has been built and validated against this plan: 85 Rust tests (including the §4 deviation suite and §5 workflows) and 36 Python reference comparisons pass against the release binary. Results: [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md), [BENCHMARK_RESULTS.md](BENCHMARK_RESULTS.md). Remaining gaps relative to §7–§8 (other platforms, Streamable HTTP, enforced memory budgets, non-LSST MIST data) are listed there.

The target is complete scientific capability coverage through Rust, CLI, and MCP. An early working renderer is a milestone, not evidence of a complete port. Each delivery stage below has an objective exit gate. Native C dependencies are allowed under the user's clarification; grid construction and histogram accumulation remain Rust implementations.

## 1. Baseline evidence from this analysis

| Evidence | Result | Limit |
| --- | --- | --- |
| Non-MIST upstream tests | 15 tests and 17 subtests passed | Nominal Python behavior only |
| MIST upstream tests | 3 tests passed with downloaded LSST v1.2 rotation-0.4 data | Other systems/rotation not exercised |
| API inventory | 172 declarations mapped to 49 service operation IDs | Mapping is specification coverage, not implementation coverage |
| Additional probes | 18 recorded probes including data inventory and edge/failure behavior | Probes are observations, not a comprehensive assertion suite |
| Asset inventory | Five bundled files hashed; 164 curve names match filter metadata | External model redistribution/provenance still needs a manifest |
| Rust build/performance | Not run; no Rust implementation in this pass | No performance or numerical-port claim is made |

The user's subsequent request for executable end-to-end tests and benchmarks is addressed by [validation/](../validation/README.md). It implements five reference workflows, strict Rust CLI request fixtures, numerical/statistical comparators, comparator-failure tests, and a fresh-process benchmark runner. [E2E results](evidence/e2e-tests.txt) and [Python baseline measurements](evidence/python-benchmark.json) record execution in this pass. Rust comparisons are explicitly skipped until a target binary exists; the benchmark reports no speedup in a Python-only run.

Evidence files live in [evidence/](evidence/). The executable analysis scripts live in [../analysis/](../analysis/). The upstream checkout is unmodified.

## 2. Acceptance philosophy

Use three complementary oracles:

1. **Pinned upstream outputs** for established nominal behavior and explicitly selected compatibility algorithms.
2. **Independent analytic invariants** for scientific correctness, especially where upstream behavior is defective.
3. **Statistical tests** for stochastic results when Rust intentionally uses a different RNG/distribution algorithm.

Do not loosen tolerances until a mismatch passes, and do not make same-seed equality with NumPy a condition for the default Rust RNG. If exact NumPy replay is ever requested, treat MT19937 seeding, uniform conversion, weighted-choice ordering, Poisson/Normal algorithms, caching, and draw order as a separate large compatibility feature.

Store every golden fixture with upstream commit, Python/dependency versions, input request, dataset digest, output dtype/shape/units, and allowed comparison. Fixture generation and differential/benchmark tooling may use Python in a separate development/reference environment. Normal Rust builds, tests using checked-in fixtures, CLI, and MCP must not depend on Python.

## 3. Numerical test matrix

| Area | Required cases | Acceptance |
| --- | --- | --- |
| Units/schema | pc/kpc/Mpc, arcsec/radian/degree, seconds/minutes, meters/cm, wavelength units; conflicting/missing units | Exact canonical conversions where representable; explicit dimension errors |
| IMF formulas | All presets; arbitrary slopes/breaks; slope 0, −1, −2 and nearby values; split intervals; raw/number/mass/sum normalization | Analytic integration identities, continuity at breaks, nonnegative physical weights |
| IMF inversion | Tabulated CDF at exact nodes/low tail; analytic segment boundaries; tiny count and broad mass ranges | Deterministic quantile fixture tests plus distribution tests |
| Mass stopping | Target below one draw, batch sizes 1 and large, high-mass final batch | Documented stopping/overshoot policy; bounded termination and correct mass accounting |
| Parsers | Real `.iso`/`.iso.cmd`, PARSEC, generic CSV; multiple ages; malformed headers/truncated rows/duplicate columns | Exact metadata/row counts and typed parse errors with locations |
| MIST selection | Age endpoints/ties, on/off-grid metallicity, both rotations, two systems with equal/mismatched EEPs | Pinned data identity, correct requested/resolved parameters, alignment checks |
| Interpolation | Linear, nearest/nearest-up, step directions, quadratic/cubic/integer order, range/fill/extrapolate, duplicates | Method-specific fixtures and monotonic/endpoint checks where applicable |
| Magnitude inversion | Multiple crossings, exact nodes, flat adjacent magnitudes, boundary and missing band | All valid roots; no monotonicity assumption |
| Isochrone integrals | Existing LSST fixture plus synthetic analytically integrable table | Magnitude/color/SBF/mean mass/remnant fractions within fixed tolerances |
| Populations | Count/mass paths; all discrete/partially unresolved/all unresolved; current-vs-initial mass | First/second moments and mass accounting preserved |
| Extinction/distance | Per-band attenuation, uniform attenuation, distance ×10, update without resampling | Total and SBF mags +A, color changes A_blue−A_red, distance mags +5 |
| Composition | discrete+discrete, discrete+unresolved in both orders, unresolved+unresolved, nested components | Flux/moment/mass additivity, stable labels, no input mutation |
| Phase masks | Every exact EEP threshold and neighbors; aggregate groups; absent EEP | Correct intervals and typed missing-data error |
| Grid construction | One hot pixel, asymmetric/non-square supplied grids, zeros, negative/NaN/infinite/extreme weights | Correct normalization/axes, rejected invalid distributions |
| Grid sampling | Flat CDF/alias/tiled, sample size smaller/equal/larger than image, fractional offsets, even/odd dimensions | Correct discrete probabilities and always-applied outside policy |
| Spatial profiles | Sérsic concentrated/extended/rotated/elliptical; Plummer radial CDF; uniform field | Distribution-specific analytic or numerical-reference checks |
| Histogram | Empty arrays, repeats, masks, NaN policy, all four frame edges, nextafter near bin edges, W≠H, 1-pixel dimension | Exact accepted counts; weighted sum matches accepted weights |
| Legacy histogram | 5×5 probe plus 201×101 and random fractional positions | Matches fast-histogram reference within floating reduction tolerance |
| PSF | Gaussian/Moffat, rectangular odd shapes, all four discretization modes, narrow/broad widths | Unit sum, symmetry, centroid, known 3×3 upstream arrays |
| Convolution | Center and edge impulses, constant field, off-center empirical kernels, wrap/fill, kernel bigger than image | Direct vs FFT parity, correct origin/crop, expected edge loss |
| Counts/calibration | AB physical model, instrumental zero point, sky at two pixel scales, custom curves | Counts scale with exposure/area/efficiency; sky scales with pixel area; calibration invertibility |
| Noise | Poisson at zero/small/large means, Normal read noise, background-only/source-only fields | Correct mean/variance, no negative expected variance, negative observed pixels allowed |
| Filters | Every bundled filter, AB/Vega/ST offsets, irregular curve spacing, invalid/empty curves | Metadata completeness, reversible conversions, analytic curve integrals |
| FITS/artifacts | f64 and integer images, non-square orientation, multiple planes, scaling/masks/headers, malformed lengths | External FITS reader round-trip, byte integrity, metadata/shape checks |
| Preview/plots | NaNs, equal-valued image, reversed gray, lower origin, percentiles, RGB channel mapping | Bounded output, recognizable orientation; science data remains unchanged |

Key deterministic upstream reference values from the existing fixture include LSST i SSP magnitude `4.7117543`, SBF magnitude `-1.4292711`, u−y color `2.2320949`, and Salpeter surviving fractions `0.7014041` with remnants and `0.5809369` without. Preserve the source fixture hashes and normalization settings when using these values.

### Tolerance policy

Initial engineering targets, to be frozen with fixture definitions:

- Indexing, metadata, masks, counts, and fixed artifact bytes: exact equality.
- Straightforward f64 transforms/integrals/interpolation: `abs <= 1e-12 + 1e-10*scale`, with `scale` explicitly defined per quantity and correct units.
- Magnitudes/SBF derived from the nominal LSST fixture: absolute error ≤1e-7 mag to match existing reference precision; more precise stored fixtures can tighten this.
- Normalized PSFs: sum error ≤1e-12 for well-conditioned fixtures; pixel comparison ≤1e-10 absolute plus relative allowance against a converged integration oracle.
- Convolution: total L1 residual ≤1e-9 times a defined signal norm for well-conditioned test images, plus per-pixel error checks near bright sources. Near-zero pixels need an absolute floor; relative-only comparisons are inappropriate.
- Inverse gamma and numerical PSF integration: test a stated ordinary operating range (initially Sérsic n 0.2–8 plus stress cases outside it) against high-accuracy reference values. Solver tolerances and actual supported extreme range are reported explicitly.

These are acceptance targets, not measured Rust performance or guaranteed precision in every pathological regime. If a method cannot meet a target, investigate the formula/conditioning and document a justified domain/tolerance change before release.

Statistical tests use fixed suites of multiple seeds and thresholds chosen for controlled false positives. Use expected bin counts large enough for chi-square tests, KS/CDF tests for one-dimensional masses/radii, and confidence bounds for moments. Aggregate planned tests with a family-wise false-failure target such as 1e-6 rather than relying on one arbitrary seeded image or a p>0.05 rule. Retain samples/statistics needed to investigate failures.

## 4. Explicit deviation regression suite

Create one named regression test for each verified upstream issue, with an independent expected answer:

- The default histogram accepts a pixel-center star on the final column; legacy convention excludes its documented upper edge.
- `dx=0.5` works and any shifted outside star receives the selected outside treatment.
- Custom slopes −1 and −2 yield logarithmic integrals, not exceptions.
- Doubling pixel scale quadruples instrumental sky counts for constant surface brightness.
- Conflicting instrument configuration is rejected instead of silently applying precedence.
- A custom FilterSystem produces a valid instrument.
- Population composition with unresolved light succeeds in either operand order and retains moments.
- One magnitude of extinction increases total and SBF magnitudes by exactly one within tolerance.
- Uniform-SB normalization depends on the selected IMF.
- Off-grid MIST interpolation keeps the selected rotation at both endpoints.
- Adding physically permitted remnants cannot subtract mass due to inverted integration limits.
- Expected variance is nonnegative even if the observed signal is negative.
- A partly invalid position cannot be deposited as a valid star.
- Literal age endpoint 10.3 is handled according to the exact model grid.
- Component extinction, labels, and smooth light survive composition.

All deliberate upstream differences are recorded by stable ID in release notes/manifests where relevant. Do not silently mix upstream coordinate rules with corrected noise or selection rules under a label claiming exact legacy parity.

## 5. Agent interface tests

Generate test cases from the operation registry and run through both CLI and an actual MCP client. Check equal scientific content IDs and summaries for identical resolved requests. Compare semantic results, excluding request IDs/timing/transport formatting.

Protocol tests cover tool listing and schemas, negotiated supported revisions, resource retrieval, outputSchema validation, application error results, malformed messages, cancellation, client disconnect, multiple concurrent requests, and stderr-only diagnostics. Exercise both older supported lifecycle negotiation and the current revision through the SDK. Optional HTTP transport must pass its own tests before being advertised.

Workflow tests cover:

1. Fresh offline installation: list bundled filters, import a generic isochrone, build a population, sample a grid, observe, and export FITS/PNG.
2. Missing MIST data: produce a concrete typed missing-data response, explicitly fetch/import, and retry successfully.
3. Reproduce quickstart in three bands with one retained population/source and separate deterministic noise streams.
4. Build young/old populations, query phases and colors, compose them, and preserve component labels.
5. Render a custom ring-weight grid and an imported empirical PSF.
6. Embed mock images into a supplied FITS background with correct edge clipping and retained headers.
7. Run a large job, inspect progress, cancel, and verify no partial artifact is marked completed.
8. Retry the same idempotency key after disconnect; receive the same job/result, not a duplicate simulation.
9. Restart the worker and report interrupted jobs accurately; only resume supported checkpoints.
10. Read a 20-row catalog preview without loading the full catalog into the agent's response.

## 6. Performance plan

First benchmark the pinned Python reference and unoptimized-correct Rust baseline on the same hardware/data. Then optimize measured bottlenecks. Record CPU, compiler flags, worker count, algorithm choices, data digests, wall time, peak resident memory, and artifact sizes. Separate cold downloads/parsing from warm scientific computation.

| Benchmark | Parameters | Primary metric |
| --- | --- | --- |
| IMF inverse construction/draws | 100k edges; 1e5/1e6/1e7 samples | Preprocessing time, draws/sec, allocations |
| Grid evaluation/CDF | 201², 1001², 4097²; flat/elliptical Sérsic | Peak bytes, evaluations/sec, preprocessing time |
| Spatial draws | 1e5–1e7 stars; sparse/concentrated/wide weights | Samples/sec and method-memory tradeoff |
| Histogram | 1e5–1e7 stars; 201², 2048², non-square; uniform/clustered positions | Deposits/sec, memory, determinism |
| PSFs | 41² and 101², all modes | Accuracy vs evaluation time |
| Seeing | 701², 2048², 4096²; 41²/101² kernels | Direct/FFT crossover and padded peak memory |
| Population build | Count/mass and cutoff paths; 1/6/20 bands | Stars/sec, bytes/star, integrated-light overhead |
| MIST parser/cache | Real LSST plus larger HST data | Cold parse time, warm query time, memory |
| End-to-end | Quickstart plus large/low-surface-brightness scenes | Time/peak memory and transport overhead |

No fixed “10× faster” promise is justified before benchmarking. Performance acceptance requires bounded resource use, no unintended O(N²) concatenation, no coordinate mesh allocations in the native sampler, and correct behavior at configured limits. Speed improvements may not change declared distributions, units, cutoff semantics, or deterministic results invisibly.

Estimate memory from actual planned buffers. Catalog lower bound is roughly `N * (8*(4+B) + 16)` bytes for initial/current mass, x/y, B magnitudes, and two u64 IDs, before optional columns/masks/capacity. A 1e7-star six-band catalog is thus already about 960 MB decimal before EEP/logL/logTeff. Image-only execution and magnitude cuts can reduce retained output, but they must preserve requested scientific summaries and be explicit in the request.

## 7. Delivery sequence and exit gates

| Milestone | Deliverable | Exit gate |
| --- | --- | --- |
| M0 — Contract and reference assets | Workspace, types, operation registry, schema conventions, converted bundled metadata, pinned fixture manifest, dependency decisions | Small reference data usable without Python; coverage ledger imported into an implementation tracker |
| M1 — Native numerical foundation | Units, IMF/integrals/inverse sampling, model evaluation, weighted-grid builder, categorical sampler, Rust histogram | Analytic/edge tests pass; CLI and MCP can sample masses/positions and create count/flux maps |
| M2 — Data and isochrones | Explicit cache/fetch/import, MIST/PARSEC parsers, interpolation, filter integration/conversions, theoretical photometry | All 12 metadata systems covered; both rotations/metallicity paths represented in fixtures; corruption/offline tests pass |
| M3 — Populations | Count/mass construction, remnants, cutoffs, unresolved moments, photometry, phases, updates/composition | Mass/flux/SBF invariants and upstream nominal references pass; all population operations callable |
| M4 — Sources and scenes | Sérsic/Plummer/uniform/custom-grid source builders, masks/labels, smooth amplitudes/composition | Geometry/selection/truncation tests pass; no smooth light or extinction lost |
| M5 — Complete imaging | PSF modes, seeing, physical/instrumental conversion, ideal/artificial observing, noise/variance | PSF/convolution/count/noise gates pass; full observation planes returned |
| M6 — Artifacts and agent workflows | CFITSIO-backed FITS, portable bundles, catalog pages, previews/RGB/plots, jobs, progress/cancellation/idempotency | Six tutorial-equivalent workflows plus failure/retry workflows run through both adapters |
| M7 — Full parity and releases | Completed coverage ledger, system matrix, release binaries/docs/examples, performance report | No required catalog entry unimplemented; package/runtime/native dependencies documented; release acceptance checklist passes |

Every milestone includes both CLI and MCP exposure for the operations it introduces. A minimal adapter/artifact layer is scaffolded in M0/M1; M6 completes durability and richer output handling. Do not postpone MCP until all scientific code is done, because typed contracts and artifact sizes affect core data design.

The critical path is M0 → M1 → M2/M3 → M4/M5 → M6/M7. Treat a mass-sampling demo or ideal-image renderer as an explicitly partial release. Reserve time for edge validation and native-library packaging, not just translation of the roughly 5k upstream lines. Detailed calendar estimates should follow M1/M2 measurements and the number of contributors; no unsupported delivery date is assumed here.

## 8. Release definition of done

- Every required operation and every upstream ledger row has implementation status, tests, and a documented replacement/deviation where applicable.
- Every operation works through both CLI and MCP with identical scientific semantics and schema versions.
- All stochastic outputs record algorithm/seed/data provenance; thread/job/band ordering tests pass.
- Native Rust grid construction and histogram accumulation pass boundary, orientation, mask, distribution, and memory checks.
- Populations retain discrete and unresolved first/second moments, mass components, phase labels, extinction, and composition identity.
- All filters/systems, both permitted MIST rotations, PARSEC/custom imports, PSF modes, imagers, and observation planes are covered.
- Offline use works from shipped/imported data; explicit fetch verifies and atomically publishes cache entries.
- Portable artifact and FITS round-trips pass external-reader checks; no runtime Python or shell scientific executable is needed.
- Linux/macOS/Windows binaries build and execute acceptance workflows; allowed native libraries are pinned, packaged, and attributed.
- Memory limits, cancellation, failure/retry paths, and clean machine output pass integration tests.
- Benchmarks and known numerical domains/limits are published without unsupported performance claims.

## 9. Reproducing the current analysis

The following commands use an isolated reference environment and write only analysis evidence. Dependency versions used for this run are in `evidence/probes.json`; pin those for exact reproduction.

```bash
UV_CACHE_DIR=/tmp/artpop-analysis-uv-cache uv venv /tmp/artpop-analysis-venv --python /usr/bin/python3
UV_CACHE_DIR=/tmp/artpop-analysis-uv-cache uv pip install --python /tmp/artpop-analysis-venv/bin/python numpy scipy astropy matplotlib fast-histogram pytest requests
mkdir -p /tmp/artpop-analysis-mist /tmp/artpop-analysis-mpl
MIST_PATH=/tmp/artpop-analysis-mist MPLCONFIGDIR=/tmp/artpop-analysis-mpl PYTHONPATH=../ArtPop/src /tmp/artpop-analysis-venv/bin/python -m pytest ../ArtPop/tests --ignore=../ArtPop/tests/test_mist.py -q
MIST_PATH=/tmp/artpop-analysis-mist MPLCONFIGDIR=/tmp/artpop-analysis-mpl PYTHONPATH=../ArtPop/src /tmp/artpop-analysis-venv/bin/python -m pytest ../ArtPop/tests/test_mist.py -q
MIST_PATH=/tmp/artpop-analysis-mist MPLCONFIGDIR=/tmp/artpop-analysis-mpl PYTHONPATH=../ArtPop/src /tmp/artpop-analysis-venv/bin/python analysis/probe_upstream.py
python3 analysis/inventory.py
```

Run from the `artpop-rust` directory. The MIST tests may download the LSST grid when the temporary cache is empty. The fixture pickles read by the probes are the pinned upstream repository assets, not arbitrary user uploads. These Python reference scripts are intentionally separate from the proposed Rust build/runtime.
