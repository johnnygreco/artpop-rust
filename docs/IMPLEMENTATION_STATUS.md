# Implementation status

Status date: 2026-10-12. Engine `artpop-rust 0.1.0`, algorithm profile `rust-v1`, reference ArtPop 0.1.2 at [`38a8869`](https://github.com/ArtificialStellarPopulations/ArtPop/commit/38a886906ed175d0205bf48b6dc03f54aeb4a81a).

The Rust port specified in [RUST_PORT_SPEC.md](RUST_PORT_SPEC.md) and [CLI_MCP_CONTRACT.md](CLI_MCP_CONTRACT.md) is implemented. All 68 contract operations are registered once and reachable through the CLI (`artpop-rust invoke <id>` or `artpop-rust <domain> <action>`) and as MCP tools (`artpop_<domain>_<action>`). Every operation is exercised by at least one automated test, listed below. Measured results are in [BENCHMARK_RESULTS.md](BENCHMARK_RESULTS.md) and [evidence/](evidence/).

## Workspace

| Crate | Lines | Responsibility |
| --- | ---: | --- |
| `crates/artpop-core` | ~5,500 | IMFs, interpolation (SciPy `interp1d` kinds), isochrones, populations and unresolved moments, MIST/PARSEC parsing, spatial models and categorical samplers, weighted histograms, PSFs, convolution, detector model, special functions, RNG streams, large-buffer allocation helpers. No I/O, CLI, or MCP. |
| `crates/artpop-io` | ~1,100 | CSV tables (chunk-parallel parse, streaming typed writer), FITS (native 64-bit image encoder and streaming writer with CFITSIO's exact header layout; reading through vendored CFITSIO, `fitsio` crate), PNG previews and RGB composites, SVG plots. |
| `crates/artpop-service` | ~8,200 | Typed request schemas (generated JSON Schema), operation registry, executor, result envelopes, content-addressed artifact store (members hashed and written in one parallel pass, reads verified while streaming), lazy column loading, streaming exports, dataset manifests, jobs. `population.build`, `source.build`, `observe.ideal`, and `observe.artificial` report wall-clock stage timings (`timings_ms`) in their results. |
| `crates/artpop-mcp` | ~200 | MCP server on the official `rmcp` 3.5 SDK (stdio). |
| `crates/artpop-cli` | ~400 | `artpop-rust` binary: `invoke`, domain commands, `batch`, `mcp serve`, `worker run-job`. |

Bundled metadata (164 filters, 12 systems, MIST zero points) is converted from ArtPop's pickles by [`tools/convert_bundled_data.py`](../tools/convert_bundled_data.py) into reviewable JSON under [`data/`](../data/) with source and output hashes in [`data/provenance.json`](../data/provenance.json); the binary embeds it and never unpickles anything. The 1,467-row LSST test isochrone is kept as [`fixtures/lsst_feh_m1.00_10gyr_iso.csv`](../fixtures/lsst_feh_m1.00_10gyr_iso.csv).

### Building

Requirements: Rust 1.99 (pinned by `rust-toolchain.toml`), a C and C++ compiler, and CMake ≥ 3.5 with Make or Ninja. CFITSIO 3.x (with its bundled zlib) is compiled from the source vendored in `fitsio-sys` and linked statically; no system CFITSIO is needed. TLS for `data.fetch` uses rustls with the ring provider and bundled webpki roots. On Linux the core crate also depends on the `libc` bindings crate (no additional native code) to advise transparent huge pages for buffers above 4 MiB; elsewhere that call is a no-op.

```bash
cargo build --release          # target/release/artpop-rust (~17 MB, Linux x86_64)
cargo test --workspace         # 97 tests: core, io, service integration, CLI/MCP
```

Only Linux x86_64 has been built and tested. The release profile uses thin LTO; `cargo test` in the default (debug) profile is slower but exercises the same code.

## Operations and tests

`coverage.rs`, `deviations.rs`, and `workflows.rs` are service integration tests (`crates/artpop-service/tests/`); `cli_mcp.rs` drives the real binary and an MCP client (`crates/artpop-cli/tests/`); `harness.py`, `test_extended_parity.py`, and `scale_cases.py` compare against ArtPop 0.1.2 (`validation/`).

| Operation | MCP tool | Exercised by |
| --- | --- | --- |
| `system.capabilities` | `artpop_system_capabilities` | `coverage.rs` |
| `system.schema` | `artpop_system_schema` | `coverage.rs` |
| `system.validate` | `artpop_system_validate` | `workflows.rs` |
| `system.estimate` | `artpop_system_estimate` | `coverage.rs` |
| `system.citations` | `artpop_system_citations` | `coverage.rs` |
| `data.list` | `artpop_data_list` | `coverage.rs` |
| `data.import` | `artpop_data_import` | `coverage.rs`, `deviations.rs`, `test_extended_parity.py`, `workflows.rs` |
| `data.verify` | `artpop_data_verify` | `workflows.rs` |
| `data.fetch` | `artpop_data_fetch` | `workflows.rs` |
| `filters.list` | `artpop_filters_list` | `test_extended_parity.py`, `workflows.rs` |
| `filters.inspect` | `artpop_filters_inspect` | `coverage.rs` |
| `filters.import` | `artpop_filters_import` | `deviations.rs`, `test_extended_parity.py` |
| `filters.convert` | `artpop_filters_convert` | `test_extended_parity.py` |
| `imf.evaluate` | `artpop_imf_evaluate` | `test_extended_parity.py` |
| `imf.integrate` | `artpop_imf_integrate` | `cli_mcp.rs`, `deviations.rs`, `test_extended_parity.py` |
| `imf.sample` | `artpop_imf_sample` | `cli_mcp.rs` |
| `imf.build` | `artpop_imf_build` | `coverage.rs` |
| `isochrone.import` | `artpop_isochrone_import` | `harness.py`, `scale_cases.py`, `test_extended_parity.py`, `workflows.rs` |
| `isochrone.mist` | `artpop_isochrone_mist` | `cli_mcp.rs`, `deviations.rs`, `test_extended_parity.py`, `workflows.rs` |
| `isochrone.inspect` | `artpop_isochrone_inspect` | `deviations.rs`, `test_extended_parity.py`, `workflows.rs` |
| `isochrone.interpolate` | `artpop_isochrone_interpolate` | `test_extended_parity.py` |
| `isochrone.invert` | `artpop_isochrone_invert` | `test_extended_parity.py` |
| `isochrone.magnitude-limit` | `artpop_isochrone_magnitude_limit` | `test_extended_parity.py` |
| `isochrone.nearest` | `artpop_isochrone_nearest` | `coverage.rs` |
| `isochrone.weights` | `artpop_isochrone_weights` | `test_extended_parity.py` |
| `isochrone.integrated` | `artpop_isochrone_integrated` | `deviations.rs`, `test_extended_parity.py` |
| `population.build` | `artpop_population_build` | `cli_mcp.rs`, `coverage.rs`, `deviations.rs`, `harness.py`, `scale_cases.py`, `test_extended_parity.py`, `workflows.rs` |
| `population.compose` | `artpop_population_compose` | `deviations.rs`, `workflows.rs` |
| `population.inspect` | `artpop_population_inspect` | `harness.py`, `scale_cases.py`, `workflows.rs` |
| `population.query` | `artpop_population_query` | `workflows.rs` |
| `population.photometry` | `artpop_population_photometry` | `deviations.rs`, `harness.py`, `scale_cases.py`, `test_extended_parity.py`, `workflows.rs` |
| `population.update` | `artpop_population_update` | `deviations.rs` |
| `population.sample-fraction` | `artpop_population_sample_fraction` | `test_extended_parity.py` |
| `population.phases` | `artpop_population_phases` | `workflows.rs` |
| `population.surface-density` | `artpop_population_surface_density` | `test_extended_parity.py` |
| `source.build` | `artpop_source_build` | `coverage.rs`, `deviations.rs`, `harness.py`, `scale_cases.py`, `workflows.rs` |
| `source.compose` | `artpop_source_compose` | `deviations.rs` |
| `source.inspect` | `artpop_source_inspect` | `coverage.rs` |
| `source.query` | `artpop_source_query` | `coverage.rs`, `deviations.rs`, `workflows.rs` |
| `source.amplitude` | `artpop_source_amplitude` | `test_extended_parity.py` |
| `spatial.evaluate` | `artpop_spatial_evaluate` | `workflows.rs` |
| `spatial.grid-build` | `artpop_spatial_grid_build` | `deviations.rs`, `harness.py`, `workflows.rs` |
| `spatial.sample` | `artpop_spatial_sample` | `deviations.rs`, `harness.py`, `scale_cases.py`, `test_extended_parity.py`, `workflows.rs` |
| `psf.build` | `artpop_psf_build` | `cli_mcp.rs`, `harness.py`, `scale_cases.py`, `test_extended_parity.py`, `workflows.rs` |
| `psf.import` | `artpop_psf_import` | `workflows.rs` |
| `imager.inspect` | `artpop_imager_inspect` | `deviations.rs` |
| `imager.convert` | `artpop_imager_convert` | `deviations.rs`, `test_extended_parity.py` |
| `image.histogram` | `artpop_image_histogram` | `coverage.rs`, `deviations.rs`, `harness.py`, `scale_cases.py`, `workflows.rs` |
| `image.smooth` | `artpop_image_smooth` | `coverage.rs` |
| `image.convolve` | `artpop_image_convolve` | `test_extended_parity.py`, `workflows.rs` |
| `image.embed` | `artpop_image_embed` | `workflows.rs` |
| `image.combine` | `artpop_image_combine` | `coverage.rs` |
| `image.preview` | `artpop_image_preview` | `workflows.rs` |
| `image.rgb` | `artpop_image_rgb` | `workflows.rs` |
| `plot.render` | `artpop_plot_render` | `coverage.rs` |
| `observe.ideal` | `artpop_observe_ideal` | `coverage.rs`, `harness.py`, `scale_cases.py`, `workflows.rs` |
| `observe.artificial` | `artpop_observe_artificial` | `deviations.rs`, `harness.py`, `scale_cases.py`, `workflows.rs` |
| `observation.inspect` | `artpop_observation_inspect` | `deviations.rs`, `harness.py` |
| `artifact.list` | `artpop_artifact_list` | `cli_mcp.rs` |
| `artifact.inspect` | `artpop_artifact_inspect` | `coverage.rs` |
| `artifact.import` | `artpop_artifact_import` | `coverage.rs`, `test_extended_parity.py` |
| `artifact.export` | `artpop_artifact_export` | `cli_mcp.rs`, `coverage.rs`, `harness.py`, `test_extended_parity.py`, `workflows.rs` |
| `artifact.clone` | `artpop_artifact_clone` | `coverage.rs` |
| `simulate.run` | `artpop_simulate_run` | `coverage.rs` |
| `job.submit` | `artpop_job_submit` | `cli_mcp.rs` |
| `job.status` | `artpop_job_status` | `cli_mcp.rs` |
| `job.cancel` | `artpop_job_cancel` | `cli_mcp.rs` |
| `job.result` | `artpop_job_result` | `cli_mcp.rs` |

## Deliberate differences from ArtPop 0.1.2 (`rust-v1`)

Each corrected behavior has a regression test with an independent expected answer. Comparisons that must match upstream select the documented legacy options (`coordinates: "artpop-0.1.2"`, `frame: "intrinsic_apparent"`).

| ID | Behavior in rust-v1 | ArtPop 0.1.2 | Test |
| --- | --- | --- | --- |
| D01 | Default bins are pixel centers; the final row/column is kept. `artpop-0.1.2` reproduces fast-histogram arithmetic bit for bit. | Range `[0, W-1)`, bins of width `(W-1)/W` | `deviations::final_column_star_is_kept_by_default_and_dropped_by_legacy_bins`; E2E `histogram` exact equality |
| D02 | Coordinates are f64; fractional offsets work; the outside policy is applied after every transform. | Fractional offsets raise; same-size shifted grids escape unmasked | `deviations::fractional_offsets_work_and_outside_policy_always_applies` |
| D03 | IMF slopes −1/−2 integrate with logarithmic branches (`expm1`/`ln_1p` forms). | Division by zero | `deviations::logarithmic_imf_slopes_integrate` |
| D04 | Instrumental-zero-point sky and smooth light are multiplied by the pixel area. | Pixel area omitted | `deviations::instrumental_sky_scales_with_pixel_area`; parity test asserts the 0.04 factor at 0.2″/pix and equality at 1″/pix |
| D05 | Exactly one calibration path per instrument (tagged union). | `phot_system` silently wins over `zpt_inst` | `deviations::conflicting_instrument_configuration_is_rejected` |
| D06 | Custom filter curves build a valid physical instrument. | TypeError | `deviations::custom_filter_curves_make_a_valid_instrument` |
| D07 | Population composition keeps every component's unresolved moments in either order. | Fails for full + partial | `deviations::composition_with_unresolved_light_works_in_both_orders` |
| D08 | Unresolved second moments are attenuated by extinction, so 1 mag of extinction shifts SBF by exactly 1 mag. | SBF shifts by 0.9995 mag | `deviations::one_magnitude_of_extinction_shifts_total_and_sbf_by_one` |
| D09 | Uniform-surface-brightness star counts use the selected IMF. | Always Kroupa | `deviations::uniform_surface_brightness_count_depends_on_the_imf` |
| D10 | Off-grid [Fe/H] interpolation reads both endpoints at the requested rotation. | Rotation dropped | `deviations::off_grid_mist_uses_the_requested_rotation_and_accepts_age_10_3` |
| D11 | Remnant integrals are clipped to the allowed mass interval. | Negative remnant mass for upper limit 2 Msun | `deviations::restricted_remnant_limit_cannot_subtract_mass` |
| D12 | `expected_variance_counts` (always ≥ 0) and its calibrated form; ArtPop's estimator is optional and labeled. | `raw + rn²` can be negative | `deviations::expected_variance_is_nonnegative_even_when_observed_pixels_are_negative` |
| D13 | A row with any non-finite coordinate is rejected (or dropped by request). | Partly masked rows deposited | `deviations::partly_invalid_positions_are_never_deposited` |
| D14 | MIST ages use integer grid indices; literal 10.3 is accepted. | `10.3` fails the float `arange` range check | `deviations::off_grid_mist_uses_the_requested_rotation_and_accepts_age_10_3`; `mist::tests::age_grid_indices` |
| D15 | Scenes keep smooth components, extinction, and component identity. | Smooth light and extinction lost | `deviations::composition_keeps_extinction_labels_and_smooth_light` |
| D16 | Multiple MIST systems are joined only when initial masses and EEPs match (`MODEL_ALIGNMENT` otherwise). | Appended by row position | Implemented; exercised only with LSST data locally |
| D17 | Magnitude cutoffs default to observed magnitudes (with extinction); `intrinsic_apparent` reproduces ArtPop. | Intrinsic only | `test_extended_parity::test_sample_fraction_and_surface_density`, `test_magnitude_limited_population_photometry` |
| D18 | Missing AB/Vega/ST conversions are `MISSING_ZEROPOINT` (or explicitly kept native). | Warning and silent zero | `filters::tests::conversions_follow_zero_point_converter` |
| D19 | No implicit downloads; datasets need a manifest; every read is digest-verified. | Downloads on construction; directory existence is a cache hit | `workflows::missing_mist_data_is_typed_and_recoverable` |
| D20 | Portable Poisson sampler on every platform. | Normal approximation on Windows | `rng::tests::poisson_and_normal_moments` |
| D21 | ChaCha8 streams per (stage, keys, 65,536-element block); no NumPy sequence replay. Stochastic outputs are compared statistically. | MT19937 | E2E `population`/`grid`/`detector`; scale statistics |
| D22 | IMF cell integrals avoid cancellation. On the fixture, Rust matches a 50-digit oracle to 1e-12 (relative) per weight and 1e-12 mag for SSP magnitudes/SBF; ArtPop's own errors reach 3e-2 (relative, tiniest cells) and 2.2e-7 mag (SBF). | Indefinite integrals anchored at 1e-3 Msun | `test_extended_parity::test_isochrone_integrated_quantities` |

## Known limitations

- MCP: stdio transport only (Streamable HTTP is not implemented). MCP progress notifications and per-request cancellation are not wired to operations; the `job.*` operations are the portable mechanism for long work. Protocol revisions 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25 (`initialize`) and 2026-07-28 (`server/discover`) are negotiated by the SDK; tests cover four of them.
- Jobs run one per detached worker process. Cancellation is cooperative (marker file, worker exits without publishing); there are no checkpoints, so interrupted jobs are reported `interrupted` and must be resubmitted. There is no queue or concurrency limit. The submitter records the worker PID in a separate `worker.pid` file: writing it into `state.json` after the spawn raced with a fast-starting worker's own `running` write and could leave a running job reported as `queued` (found by the worker-kill test under release-build timing; fixed).
- Limits: star counts, grid cells, image pixels, page sizes, and inline sizes are enforced. `execution.max_memory_bytes` is validated and recorded but not enforced as an allocator budget, and request deadlines are not implemented.
- Data: only the MIST v1.2 LSST rotation-0.4 grid has been exercised with real files. It was fetched live from mist.science with `data.fetch` (85,410,956-byte archive, 15 files byte-identical to the analysis download; [evidence](evidence/data-fetch-lsst.json)), and its archive SHA-256 is now pinned. Other grids are fetched unpinned with an `UNPINNED_CHECKSUM` warning unless `expected_sha256` is supplied.
- Interpolation accepts nondecreasing initial masses with `numpy.interp` semantics for linear kinds; spline kinds require strictly increasing x. There is no separate duplicate-handling option.
- Previews use polynomial approximations of viridis/magma/inferno; `plot.render` produces plain SVG without ArtPop's `mpl_style`/`jpg_style` settings.
- FITS support covers 2D image HDUs (any CFITSIO-readable pixel type, scaled to f64). Images are written by a native encoder whose bytes are tested to be identical to CFITSIO's output (header layout, `%.15G` value formatting, padding); reading still goes through CFITSIO. WCS is copied, never interpreted; tables and compressed images are untested.
- Arrow/Parquet export is not implemented (CSV, FITS, JSON, PNG, SVG, and portable bundles are).
