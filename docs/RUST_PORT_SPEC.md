# artpop-rust implementation specification

Status: design specification, now **implemented** (2026-10-12). See [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md) for what was built, the deviation ledger, and known limitations; where the implementation refined this design (block-parallel RNG streams, tiled overlap-save convolution, CFITSIO 3.x vendored build), that document is authoritative. Upstream behavior and evidence are in [UPSTREAM_ANALYSIS.md](UPSTREAM_ANALYSIS.md). Every mapped capability must be available through both interfaces described in [CLI_MCP_CONTRACT.md](CLI_MCP_CONTRACT.md).

## 1. Required outcomes and compatibility contract

1. Implement the scientific pipeline in Rust, with no Python runtime, Python subprocess, or NumPy/SciPy bridge. This includes constructing spatial weights/CDFs, sampling stars, accumulating count/flux histograms, special functions, PSFs, FFT convolution, random noise, and table import. Established C libraries may support well-defined infrastructure/file-format operations behind Rust interfaces, as authorized in the user's clarification.
2. Preserve all ArtPop scientific capabilities: custom and MIST/PARSEC isochrones, all IMFs and normalizations, theoretical and sampled photometry, unresolved light, remnants, extinction, phase selection, compositions, all spatial profiles and arbitrary supplied grids, both imagers, filter conversions, PSF modes, persistence, and visualization equivalents.
3. Expose a stable library API, a CLI executable named `artpop-rust`, and `artpop-rust mcp serve`. They use the same validated request types and executor. No scientific operation is implemented only inside a CLI or MCP handler.
4. Treat large catalogs/images as artifacts with bounded previews. Agents receive concise results, explicit units, reproducible seeds, useful error paths, and discoverable schemas.
5. Ship an offline-capable core. Network access is an explicit data-management operation, never a constructor side effect.

The default numerical contract is **`rust-v1`**: scientifically equivalent nominal behavior, corrected defects listed in the upstream analysis, and versioned numerical choices. It does not promise identical random sequences for the same Python integer seed. Expose upstream discretized IMF inversion and `artpop-0.1.2` histogram bounds as named options for comparison. Do not advertise a blanket legacy mode until all its behaviors have actually been specified and tested.

The user subsequently clarified that C code is acceptable where needed. The resulting boundary is: Rust owns the library, scientific orchestration, numerical algorithms, CLI/MCP, and particularly spatial sampling and histogram construction. Narrow, maintained C dependencies are allowed for FITS, TLS, or compression. Python and shell-outs to scientific executables remain excluded. Prefer RustFFT and native Rust math to avoid unnecessary numerical FFI; any later native numerical backend must pass the same accuracy, determinism, packaging, and interface tests. Document native versions and build/link choices.

Language-specific API compatibility is replaced by capability compatibility: Python callables become a Rust model trait or a supplied numeric grid; pickle becomes a documented portable artifact; Matplotlib figures become native preview/plot files. These replacements are public, not hidden exclusions. Third-party arbitrary object methods are not inherited into the service contract.

## 2. Workspace and dependency direction

```text
artpop-rust/
  Cargo.toml                 workspace and release profiles
  Cargo.lock                 pinned executable dependencies
  rust-toolchain.toml        tested toolchain/MSRV
  crates/
    artpop-core/              scientific types and pure computation
    artpop-data/              model parsers, catalogs, cache manifests, fetch
    artpop-io/                portable bundles, FITS, CSV, PNG/SVG
    artpop-service/           requests, validation, operations, jobs, artifacts
    artpop-cli/               clap adapter and executable entry point
    artpop-mcp/               rmcp adapter
  data/                      converted bundled metadata and provenance
  schemas/                   generated JSON Schema and operation manifest
  fixtures/                  versioned numerical references
  examples/                  executable JSON requests and CLI workflows
  tests/                     cross-interface and end-to-end acceptance
  benches/                   numerical, memory, parser, and workflow benchmarks
  docs/                      specifications and usage
```

`core` cannot depend on service, CLI, MCP, a network client, or a filesystem. `data` and `io` depend on core types. `service` owns orchestration and the artifact store. CLI and MCP depend on service. A synchronous or streaming core API accepts cancellation/progress callbacks without depending on an async runtime. The service uses bounded blocking workers for CPU work and leaves protocol I/O responsive.

Start with these boundaries even if several crates are initially small. They prevent protocol concerns from becoming numerical dependencies and make no-network library use straightforward.

## 3. Core types and invariants

Use f64 for physical values, weights, flux moments, coordinates, and science images. Use checked u64 counts and checked `usize` conversions for allocation. Use newtypes for `MassSolar`, `DistancePc`, `AngleRad`, `PixelScaleArcsec`, `ExposureSeconds`, `WavelengthAngstrom`, and validated finite scalars. Public JSON uses explicit unit-bearing objects or unambiguous unit suffixes; internal units are canonical.

| Type | Required contents / invariants |
| --- | --- |
| `Image<T>` | Positive width/height, row-major `data[y*width+x]`, checked length; optional validity plane |
| `ImageGeometry` | Dimensions, coordinate convention, pixel scale, source center, optional external FITS metadata |
| `FilterId` / `FilterCatalog` | Stable upstream names, system identity, units, magnitude reference, provenance |
| `FilterCurve` | Strictly increasing positive wavelength; finite nonnegative transmission; positive integral |
| `MagnitudeSystem` | AB, Vega, ST, or named instrumental calibration; no implicit interchange |
| `Imf` | Named preset or continuous piecewise power law; slopes, ordered positive breaks, finite mass domain |
| `Isochrone` | Equal-length initial/current mass columns and ordered filter columns; optional EEP/logL/logTeff/extra columns; source identity |
| `StarCatalog` | Structure-of-arrays: stable star ID, initial/current mass, per-band absolute magnitudes, optional evolutionary columns, component ID |
| `PopulationComponent` | Isochrone reference, IMF, distance, per-band extinction, requested and realized mass/count bookkeeping |
| `UnresolvedMoments` | Expected count, current mass, and per-band first/second **absolute flux moments**; no fabricated individual stars |
| `Population` | One or more components, discrete catalogs, unresolved moments, construction parameters |
| `SpatialModel` | Sérsic, Plummer, constant, ring, supplied grid, or supported model-expression tree |
| `SampledPositions` | f64 x/y, source star IDs, in-frame validity, generated/retained counts |
| `Source` / `Scene` | Discrete positions plus explicit smooth components, labels, geometry and population provenance |
| `Psf` | Finite odd-sized kernel, normalization, origin, FWHM/mode metadata |
| `Instrument` | Exactly one of physical filter properties/curves or instrumental zero points; read noise and calibration |
| `Observation` | Image planes, units, band/system, exposure, zero point, expected sky, calibration, construction provenance |
| `ArtifactRef` | Kind, schema version, SHA-256 content digest, URI, shape/row count, media type, bounded summary |

Keep column order stable. Use nullable optional columns with explicit validity rather than inventing zeros for missing EEP or phases. Reject non-finite physical inputs. Zero-flux magnitude outputs are represented as a defined `no_flux` status plus null numeric value in JSON, rather than nonstandard JSON infinity.

Isochrone initial masses may be nondecreasing because upstream permits repeats. Define duplicate handling per operation: preserve rows for IMF cell integration; require an explicit duplicate policy for interpolation; allow identical x/y repeats to collapse; reject conflicting duplicates by default. Do not silently sort a magnitude–mass relation before finding its crossings.

Composition is immutable. It preserves input component IDs (renumber through a documented stable mapping if needed), extinction, unresolved moments, and optional columns. Population composition initially requires equal distances and the same ordered filter set for upstream-equivalent semantics. Scenes may hold separate populations at different distances because apparent flux is already well defined, but all sources must share compatible image geometry and calibrated band identities.

## 4. Numerical algorithms

### 4.1 IMF evaluation and integration

Represent each segment as `C_i*m^a_i` with continuity constants calculated from adjacent breaks. Named presets retain upstream raw normalization as well as normalized weights. Avoid the upstream formula involving division by the high-mass slope; zero slopes are valid.

For each interval, integrate `C*m^p` as `C*(hi^(p+1)-lo^(p+1))/(p+1)`, using `C*ln(hi/lo)` when `p=-1`. Number integrals use `p=a`; mass integrals use `p=a+1`. Split at breakpoints, use stable log/expm1 forms near limiting exponents, and use compensated summation across segments. Reject reversed bounds unless a specifically signed mathematical integral is requested; physical intervals are intersected with the domain and empty intervals contribute zero.

Required sampling algorithms:

- `tabulated_inverse`: upstream logarithmic-edge/midpoint/CDF procedure, configurable edge count, explicit low-tail behavior. This is the initial default for comparison with ArtPop.
- `analytic_inverse`: select a segment by integrated probability and invert its power-law integral, including logarithmic branches. Record this method in provenance.

`imf.sample` supports exact requested count. `imf.build` supports whole-batch stopping matching ArtPop and an explicit nearest-target policy. Population construction continues to account for current mass separately from initial mass. Return requested mass, realized mass, overshoot, draw count, and stopping policy. `mass_tolerance` is a batch-sizing input in the upstream policy, not a promised error bound.

### 4.2 Table parsing and interpolation

Implement streaming Rust parsers for MIST `.iso` and `.iso.cmd`, including headers, age blocks, abundance/rotation metadata, EEP integer columns, and all available numeric columns. Retain selected-age indexing so repeated lookups avoid reparsing every age block. Parse PARSEC CMD text by column name with optional age/Zini selection and explicit filter names/count overrides. Generic isochrones import from CSV or the portable columnar bundle.

MIST v1.2 selection uses exact integer age-grid indices and nearest-age tie-breaking toward the first/lower grid entry. Return requested and selected ages. For metallicity interpolation, preserve the upstream row/tail algorithm as the initial method, forward rotation and model version to both endpoints, validate EEP correspondence, and record any extrapolated tail. A later EEP-aligned alternative must be a distinct method; do not silently substitute it.

Join multiple systems only when initial masses, EEPs, age, rotation, and composition are compatible within recorded tolerances. A mismatch is a `MODEL_ALIGNMENT` error, not an implicit row zip.

Named-column interpolation must support the useful `interp1d` capabilities exposed upstream: linear/slinear, nearest/nearest-up, previous/next/zero, quadratic/cubic, and validated integer spline order. Use native piecewise polynomial/B-spline routines with explicitly specified knots and endpoint conditions; high-order spline equivalence needs dedicated SciPy fixtures. Inputs remain one-dimensional columns; selections are index ranges/steps or explicit row masks. Bounds behavior is `error`, `fill` (including separate left/right values), or `extrapolate`. Public requests do not accept arbitrary executable keyword arguments.

Magnitude inversion scans every adjacent pair and returns all crossings, exact-node matches deduplicated by row/mass identity, with explicit treatment of flat segments. It must not assume globally monotonic luminosity. Fraction-to-magnitude inversion supports number or mass fraction (exactly one), reports the mass limit and band/system, and specifies the interpolation method.

### 4.3 Theoretical and sampled photometry

Preserve IMF bin weights and reference outputs for total magnitude, color, SBF, mean current mass, and surviving mass. Integrate remnant contributions only over the overlap with the chosen initial-mass limits and the nonliving interval. Return separate live, remnant, and initial-mass normalization quantities.

For a magnitude cutoff, store the actual sampled stars and the unresolved first/second moments, including evolved faint sampled stars transferred to that component. Define a cutoff in observed apparent magnitude by default, applying distance and extinction. Expose `cutoff_frame=intrinsic_apparent` to reproduce the upstream selection convention. A requested limit brighter than all stars yields an entirely unresolved population; fainter than all stars yields entirely discrete. Do not silently turn an impossible bright cutoff into full sampling.

With a distance modulus `DM` and extinction `A_b`, first moments scale by `10^(-0.4*(DM+A_b))`; second moments scale by its square. Then:

```text
total_mag = -2.5 log10(F_discrete + F_unresolved)
mean_mag  = -2.5 log10((F_discrete + F_unresolved)/(N_discrete + N_unresolved))
sbf_mag   = -2.5 log10((Q_discrete + Q_unresolved)/(F_discrete + F_unresolved))
color     = -2.5 log10(F_blue/F_red)
```

The unresolved expected number may be fractional internally; reports distinguish expected count from discrete integer count. An upstream count-rounding option can reproduce integer truncation where necessary. Selection requests state whether integrated light is excluded (default for individual-star selections) or which components' unresolved moments are included.

Distance/extinction updates create a new artifact. They do not resample masses. They also do not automatically change a source's stored positions; rebuilding physical geometry is an explicit source operation. Changing a magnitude cutoff requires rebuilding the population, not just editing metadata.

### 4.4 Constructing spatial probability grids entirely in Rust

Provide both `SpatialModel::evaluate(x,y)` for library use and serializable built-ins/grid assets for CLI/MCP. A user-supplied grid preserves arbitrary model weights at the same finite grid resolution as ArtPop. Include a ring built-in for the documented custom-model workflow. An optional arithmetic expression AST may combine built-ins and constants with bounded operations; it cannot call a shell, Python, or the filesystem.

The grid builder must:

1. Validate dimensions, checked byte estimates, model parameters, and sampling geometry before allocation.
2. Evaluate each integer pixel center directly from its index; allocate no x/y mesh arrays.
3. Reject NaN, infinite, or negative weights. All-zero weights are an error. Scale large weights by a finite maximum or use log-weight evaluation to avoid overflow for concentrated Sérsic profiles.
4. Accumulate a stable total and build a CDF or alias table. Preserve zero-probability cells. Final cumulative mass must cover the final positive cell.
5. Store dimensions, origin, model identity, total normalization, method, and source digest as an immutable grid artifact.
6. Sample independently with replacement, convert indices to x/y, apply offsets, then apply the requested outside policy every time.

Baseline CDF construction is O(P), storage approximately `8P` bytes when the weights are reused in place, and sampling O(N log P). Alias sampling is O(P) preprocessing/O(N) draws but needs additional probability and index arrays and consumes a different random stream. Expose the method; never select it invisibly based on machine load.

For large grids, add a hierarchical/tiled exact categorical sampler: compute tile masses in a first pass; choose tiles; evaluate/reuse tile-local CDFs within a fixed memory budget. The resulting probability for a pixel is `(tile_mass/total)*(pixel_weight/tile_mass)`. This preserves the finite-grid distribution, although it differs from a flat-CDF draw sequence. Report recomputation costs and tile-cache memory. Do not simply crop to the image, because that would condition away stars outside the frame.

Sérsic uses `I(R)=I_e*exp(-b_n*((R/r_e)^(1/n)-1))`, with `P(2n,b_n)=0.5`. Use a checked gamma implementation plus a bracketed inverse solver (Newton steps only inside a valid bracket). Calculate normalizations in log space. Positive n is the mathematical domain; extreme values that exceed validated numeric range return a documented error rather than overflow silently.

The initial Sérsic sampler retains ArtPop's square size `2*ceil(r_pix*num_r_eff)+1`. Record that truncation and its finite-grid normalization. Analytic elliptical Sérsic sampling via a gamma-distributed radius is an optional later method, not a replacement for arbitrary grid support or exact square-grid parity.

Plummer offers the upstream 3D-radius/projection algorithm and, optionally, direct projected-radius inversion. Uniform and Plummer continuous positions use the chosen coordinate convention consistently. Model evaluation and sampling are independently callable.

### 4.5 Coordinates and native weighted histograms

Default convention `pixel-centers-v1`:

- Integer `(x,y)` denotes a pixel center; origin is lower-left in scientific coordinates.
- Valid frame is `[-0.5,W-0.5) × [-0.5,H-0.5)`.
- Pixel indices are `floor(x+0.5)` and `floor(y+0.5)`, **after finite/range checks**.
- Buffer layout is `image[y*W+x]`, shape `[H,W]`; FITS `NAXIS1=W`, `NAXIS2=H`.
- Continuous uniform sampling uses those frame bounds. All source models center on `((W-1)/2,(H-1)/2)` plus offsets.
- A sampling square of side S is centered into the image using offsets `(S-W)/2` and `(S-H)/2` as f64; even image dimensions are allowed. Legacy presets may require odd dimensions.

Selectable convention `artpop-0.1.2` for histogram parity:

```text
require W > 1 and H > 1
accept 0 <= x < W-1 and 0 <= y < H-1
ix = floor(x * W/(W-1))
iy = floor(y * H/(H-1))
image[iy*W+ix] += weight
```

Match the reference implementation's arithmetic order near bin edges in oracle tests. This setting records its non-unit bins and exact upper-edge exclusion. It does not activate unrelated upstream bugs.

`image.histogram` takes positions, optional selection, and either supplied f64 weights, unit weights for a count map, or an explicitly requested magnitude-to-flux conversion. It supports non-square images, empty catalogs, repeated positions, and masks. Sampling is a separate operation: a histogram represents accumulated stars, whereas a probability grid defines where future stars may be drawn.

Reject non-finite coordinates/weights by default; a documented `invalid=drop` option returns reason-specific counts. Signed weights may be used only in generic image arithmetic, not probability distributions or physical expected counts. Out-of-frame policy is `drop`, `retain_marked`, or `error`; image accumulation always excludes outside positions and reports their lost signal. It must never redraw them unless the request explicitly asks for sampling conditioned on the frame.

Baseline deposition is a safe Rust loop, O(N+W*H). Use no floating-point atomics. Deterministic parallelization partitions stars into stable logical blocks, groups by output tile without reordering equal-tile stars, and accumulates each tile in a fixed order. A simpler serial path remains the oracle. Per-worker full images can cost O(workers*W*H) and must not be the unconditional strategy.

Required diagnostics: generated, selected, invalid, outside, and deposited star counts; selected/deposited/outside signal sums; shape, units, bin convention, and deterministic-reduction mode. Flux conservation compares against accepted stars, not every input star.

### 4.6 Smooth light, PSFs, and seeing

Sérsic/Plummer smooth components retain their intrinsic infinite-profile normalization by default; uniform uses frame area. Record captured in-frame flux separately from intrinsic total flux. Discrete Sérsic sampling is conditional on a finite square, so do not claim its sampled total has identical aperture normalization to the analytic component. Provide a clearly selected `finite_grid` normalization when users need both components normalized over identical support.

Keep every smooth component in a scene until rendering. A star mask affects only discrete stars unless component selection says otherwise. Combining already-rendered images must check units, zero points, pixel scale, geometry, and band identity.

PSF formulas: Gaussian sigma = FWHM/sqrt(8 ln 2); Moffat scale = FWHM/(2 sqrt(2^(1/alpha)-1)). Require alpha>1 for a normalizable infinite Moffat profile in the default physical mode; a finite-kernel-only mode can explicitly allow alpha>0. Normalize the sampled finite kernel and record pre-normalization flux. Implement all four upstream modes:

- `center`: evaluate at pixel centers.
- `linear_interp`: average corner values as the upstream discretizer does.
- `oversample`: evaluate a regular factor×factor subgrid and average.
- `integrate`: analytic separable erf integration for Gaussian; checked adaptive 2D or nested 1D quadrature for Moffat with exposed tolerance and iteration limits.

`image.convolve` accepts a normalized generated or imported PSF, with explicit kernel origin and finite checks. Implement direct convolution for small kernels/oracle use and RustFFT for large cases. Zero-fill needs padding for linear convolution and exact cropping; wrap needs circular convolution with correctly shifted kernel origin. Default `wrap` preserves ArtPop's operational default, and appears in every resolved request. Normalize inverse FFT by the transformed pixel count. Tiny negative residuals may be clamped only under an explicit relative/absolute tolerance; material negative expected counts are an error.

An imported PSF may contain signed values; allow that for generic image convolution with a finite nonzero sum. Before physical noise sampling, validate the resulting expected counts. Do not conflate generic kernels with nonnegative probability grids.

### 4.7 Instrument conversion and observations

For physical photometric properties:

```text
f_nu = 10^(-0.4*(m_AB + 48.6))              [erg s^-1 cm^-2 Hz^-1]
area = pi * (diameter_cm/2)^2
counts = f_nu/h * (dlam/lam_eff) * area * exposure_s * efficiency
sky_counts_per_pixel = counts(sky_mag_per_arcsec2) * pixel_scale_arcsec^2
```

For instrumental zero points:

```text
counts = 10^(0.4*(zpt_inst - magnitude)) * exposure_s
sky_counts_per_pixel = 10^(0.4*(zpt_inst - sky_sb)) * exposure_s * pixel_scale_arcsec^2
calibration = 10^(0.4*(output_zpt-zpt_inst)) / exposure_s
```

The input magnitude system must match the zero-point definition. Physical conversion requires AB or an explicit validated conversion to AB. Instrumental mode does not additionally apply diameter/efficiency; mutually exclusive configuration prevents silent precedence conflicts. Pin the physical constants used for reference conversion.

Artificial observing builds expected source counts (discrete and smooth), applies PSF, adds expected sky, samples Poisson, adds independent Gaussian read noise, and creates the sky-subtracted calibrated image. Preserve the raw realization separately. Return:

- `source_counts`, `sky_counts_per_pixel`, `raw_counts`, `calibrated_image`;
- `expected_variance_counts = source_counts + sky_counts + read_noise²`;
- `expected_variance_calibrated = expected_variance_counts * calibration²`;
- optional `observed_variance_estimator` for comparison with the upstream formula, clearly labeled and not guaranteed positive;
- optional per-star upstream-style magnitude-error estimate with its assumptions.

Poisson sampling must use a native checked implementation at small and large rates on every supported platform. Validate the count range for the chosen algorithm; never silently substitute a Gaussian on one OS. Skies and expected counts must be nonnegative; read noise may produce negative observed pixels legitimately. Do not clip calibrated observations to zero.

### 4.8 Filter integration and magnitude systems

Port the actual trapezoidal expressions over lambda or ln(lambda) from `FilterSystem`, retaining their numerical definitions. Tests must include irregularly spaced wavelengths and a simple analytic curve. Convert the bundled metadata once into reviewable JSON/CSV with original hashes. The released application must not need a Python unpickler.

Support AB/Vega/ST magnitude and color offsets, native-vs-target system metadata, and custom calibration tables. Missing conversions return `MISSING_ZEROPOINT`, with filter and available systems. Physical imager inputs must not silently interpret Vega as AB.

## 5. Reproducibility and resource management

Require an explicit seed for stochastic service requests. The CLI may offer `--random-seed`, but its chosen seed must appear in the response and stored manifest. Represent seeds as fixed-width hex strings to avoid JSON number precision loss. Use a named native generator such as ChaCha8 with a pinned algorithm/distribution version.

Derive streams from the master seed plus stable component ID, stage (`mass`, `position`, `shot_noise`, `read_noise`), and band ID where applicable. Use fixed logical blocks or indexed streams independent of worker scheduling; the implementation block size is part of the algorithm version, not a performance flag. Adding a band must not change sampled masses/positions. Reordering independent jobs must not change outputs. User-visible chunk sizes must not alter the random result under the same algorithm version.

Promise exact replay for the same engine build/algorithm version/platform profile and deterministic settings. Portable RNG bytes do not alone imply bit-identical transcendental functions or SIMD FFTs across architectures. Cross-platform floating-point scientific outputs use tested tolerances. A strict scalar mode can provide a narrower reproducibility envelope if needed.

Cache identities include dataset hashes, requested/resolved parameters, constants, algorithm versions, magnitude system, seed/stream policy, and all relevant numerical options. Seeds alone are insufficient. Do not include incidental output paths or timestamps in scientific content IDs. Avoid time-dependent metadata in deterministic artifact payloads.

`system.estimate` must calculate checked worst-case allocations for star columns, magnitudes by band, grid/CDF/alias data, PSFs, output planes, and padded FFT workspaces. Report a range when mass-based sample count is stochastic, together with a hard maximum. A 4096² f64 image alone uses 128 MiB; multiple count/variance images and complex FFT buffers dominate memory quickly. Resource limits are applied before allocations and checked at chunk boundaries.

Support maximum stars, maximum grid pixels, memory budget, output bytes, and deadline. A request exceeding limits returns a structured estimate and actionable alternatives (such as a brighter explicit magnitude limit). Never silently reduce star count, crop morphology, change PSF integration mode, or substitute approximate noise to fit a budget.

## 6. Data cache and portable artifacts

A data manifest identifies provider, release, rotation, abundance, photometric systems, source URL, archive/expanded sizes, SHA-256, acquisition time, and citation information. Keep scientific identity independent of a URL that may redirect. Fetch to a temporary path, check status and size, verify a pinned checksum, extract with traversal/link/expansion limits, validate expected members, then atomically publish a completion manifest. Concurrent fetches use a per-dataset lock. An existing incomplete directory is not a valid cache hit.

Use a dedicated cache path (`ARTPOP_DATA_DIR` or explicit service config); support existing MIST directories through `data.import`, with validation and hashes. `data.fetch` is the explicit online action. `isochrone.mist` with missing data returns `DATA_MISSING` and a concrete fetch request, never starts an unrequested multi-hundred-megabyte download.

The initial portable artifact is a versioned bundle directory: `manifest.json` plus typed little-endian column/image files and validity bitmaps, each with dtype, shape, units, length and SHA-256. Restrict manifest paths to relative local members. Readers validate checked lengths and digests before interpretation. Arrays are not serialized as enormous JSON lists. Write to a staging directory and atomically publish. CSV imports/exports support small catalog interchange; add Arrow/Parquet only if measured workflow needs justify their dependency cost.

FITS is required for image interoperability. Use the Rust `fitsio` interface to **CFITSIO**, now permitted by the user's clarification. Place it behind `artpop-io` so core types do not depend on FFI handles. Support primary and IMAGE extension HDUs, integer and f32/f64 image input, BSCALE/BZERO and BLANK handling, typed header values, and explicit units/provenance; output science images as f64 by default. Preserve imported header cards when embedding into an existing image, without claiming WCS reprojection. Test the application's supported compressed-image/table subset and return capability errors outside it. FITS image export covers ArtPop's own FITS use; a complete replacement for every Astropy FITS feature is not required. A pure-Rust reader/writer can remain an optional future backend, but is not required for this port.

Preview generation is Rust-native percentile clipping, stretch, colormap, origin handling, and PNG output. Provide SVG plots for CMD/HR/spatial diagnostics and native RGB composition for notebook workflows. These display files never replace the f64 scientific image. Styles analogous to `mpl_style`/`jpg_style` map to explicit palette/axis/font settings; TeX execution and Python GUI object semantics are excluded.

## 7. Dependency choices and implementation risks

Exact compatible versions must be resolved and pinned at implementation time. The following candidates were checked against their own documentation on the analysis date; none were compiled here.

| Need | Candidate / decision | Qualification |
| --- | --- | --- |
| Arrays / tables | `Vec<f64>` and typed columns; optional ndarray views | No BLAS dependency; simple ownership and predictable memory |
| FFT | `rustfft` | Documented pure Rust; own padding, normalization and origin handling are still required |
| RNG | `rand_chacha`, checked Rust distribution routines | Freeze generator and distribution algorithms separately |
| Gamma functions | `statrs` with minimal features; own checked inverse solver | Its gamma module exposes regularized gamma and log gamma, not a ready inverse-gamma API to assume |
| JSON/schema | serde/serde_json + schemars | Strict tagged unions; generate schemas from service types |
| CLI | clap | Explicit JSON input/output contract layered over parser behavior |
| MCP | official `rmcp` SDK, tokio | Pin supported protocol revisions and test negotiation; SDK alone is not application conformance |
| XZ / tar | `lzma-rust2` + Rust tar parser; liblzma wrapper allowed if justified | Keep bounded extraction in Rust; no shell `tar`/`xz` |
| FITS | `fitsio` + CFITSIO behind `artpop-io` | Pin/build native dependency and test portability, array ordering and ownership |
| Images / plots | Rust PNG/image encoding and SVG generation | Audit compression/font feature graph; no Matplotlib runtime |
| Parallel work | rayon or bounded service worker pool | Deterministic reduction/random streams must be designed separately |
| Hashing / errors / logging | Rust SHA-256, thiserror, tracing | Protocol stdout remains clean |
| HTTP/TLS | Rust HTTP client + production rustls provider | C/assembly backend is allowed; select supported configuration and package it explicitly |

Sources: [RustFFT](https://docs.rs/rustfft/latest/rustfft/), [rand_chacha](https://docs.rs/rand_chacha/latest/rand_chacha/), [statrs gamma functions](https://docs.rs/statrs/latest/statrs/function/gamma/index.html), [Schemars](https://docs.rs/schemars/latest/schemars/), [clap](https://docs.rs/clap/latest/clap/), [official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk), [lzma-rust2](https://docs.rs/lzma-rust2/latest/lzma_rust2/), [fitsrs](https://docs.rs/fitsrs/latest/fitsrs/), [fitsio](https://docs.rs/fitsio/latest/fitsio/).

Use a Rust HTTP client with rustls and one explicitly selected supported crypto provider, such as aws-lc-rs or ring. Record the backend and native versions in the build manifest and test HTTPS redirects/certificate validation against the model hosts. [rustls provider interface](https://docs.rs/rustls/latest/rustls/crypto/index.html)

Native dependency packaging is still a release gate: test CFITSIO/TLS builds on all supported targets, ensure library ownership/threading rules are respected, and include dependency notices. Prefer bundled builds for reproducible distributable binaries where licensing/build support permits; document a supported system-library mode. The main scientific risks remain interpolation, unresolved moments, coordinate conventions, and detector variance. Validation gates are defined in [VALIDATION_PLAN.md](VALIDATION_PLAN.md).
