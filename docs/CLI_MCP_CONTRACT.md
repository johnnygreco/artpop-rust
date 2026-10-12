# CLI and MCP service contract

**Implemented (2026-10-12):** all 68 operations below are available through `artpop-rust` and `artpop-rust mcp serve`; see [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md). Implementation notes: `--output-dir` is an alias of `--artifact-dir` (the artifact store root); `--get <pointer>` prints one envelope field; `--set key=value` adds request fields absent from `--input`; jobs run in detached `artpop-rust worker run-job` processes rather than a separate `worker serve`; only the stdio MCP transport exists. The original specification follows.

This document specified proposed interfaces, not commands available in this analysis directory at the time. The scientific semantics are defined in [RUST_PORT_SPEC.md](RUST_PORT_SPEC.md); the source-linked completeness ledger is [API_COVERAGE.md](API_COVERAGE.md).

## 1. One operation registry, two adapters

Each operation has a stable dotted ID, Rust request/result types, input/output JSON Schema, description, side-effect annotation, cost class, and executor. For example:

```text
operation: population.build
CLI:       artpop-rust population build --input population.json
MCP tool:  artpop_population_build
```

The CLI also accepts `artpop-rust invoke population.build --input population.json`, so every registered operation is automatically reachable even before ergonomic flag wrappers are added. For MCP, dots and hyphens become underscores and names receive `artpop_`. Tool definitions use each operation's real schema; do not expose only an opaque `execute` tool taking arbitrary JSON.

Generate the registry, schema catalog, MCP definitions, CLI dispatch table, and coverage report from shared Rust definitions. CI checks that every scientific registry entry is callable through both adapters. Some methods are parameters of a larger operation: AB/Vega/ST conversion is one tagged request, not six nearly identical tools.

All operations return the common result envelope, including failures. Transport failures remain distinct. High-level simulation composes the same primitives exposed individually, so an agent can start with one request and later inspect intermediate artifacts without repeating the entire run.

## 2. Request and result rules

Every request includes `schema_version: "1"`. Scientific producing requests accept `execution` containing algorithm profile, seed when stochastic, resource limits, and optional idempotency key. Defaults must be discoverable and copied into `resolved_request` in the result; omitted options must not depend on process-global scientific state.

Common types:

| Type | Contract |
| --- | --- |
| `ArtifactInput` | Exactly one immutable `artifact_id` or import descriptor; references are checked for expected kind |
| `Quantity` | `{ "value": finite_number, "unit": supported_unit }`; dimension checked before conversion |
| `CountOrMass` | Tagged union `{"kind":"count","value":positive_integer}` or `{"kind":"current_mass","value":positive_number,"unit":"Msun"}` |
| `ImfSpec` | Named preset or tagged piecewise power law; bounds and normalization explicit |
| `Selection` | All, label/component/phase membership, row range, or validated mask artifact; no executable code |
| `ArrayInput` | Small inline values or artifact reference plus column/plane; maximum inline size enforced |
| `Seed` | 16-digit hexadecimal master seed string; service derives algorithm-specific streams |
| `Page` | Opaque cursor and limit; default 100, maximum 1,000 catalog rows |
| `OutputSpec` | Requested planes/formats and destination within configured artifact/output roots |
| `ExecutionSpec` | Profile, stochastic algorithm options, seed, memory/star/grid/output/deadline limits, idempotency key |

Unknown fields are rejected. Optional and nullable mean different things. Distances must be positive; pixel scales/exposures/apertures positive; extinction finite (negative values require an explicit mathematical rather than physical mode); read noise nonnegative; fractions within their specified closed/open endpoints. Exactly one population normalization and one instrumental calibration path are required. Filter names and magnitude systems are validated before large work begins.

JSON results must not contain NaN/infinity or scientific arrays of unbounded size. Define per-field validity/status for undefined magnitudes or missing columns. Return bounded summaries and artifact references instead of dumping million-row tables.

Proposed result envelope (the hash/value placeholders below are illustrative):

```json
{
  "schema_version": "1",
  "status": "completed",
  "operation": "observe.artificial",
  "request_id": "req-example",
  "result": {
    "observation_id": "sha256:example",
    "width": 701,
    "height": 701,
    "bandpass": "LSST_i",
    "deposited_stars": 932814,
    "outside_stars": 67186
  },
  "artifacts": [
    {
      "artifact_id": "sha256:example",
      "kind": "observation",
      "uri": "artpop://artifacts/sha256:example/manifest",
      "media_type": "application/json"
    }
  ],
  "warnings": [],
  "provenance": {
    "engine_version": "0.1.0",
    "algorithm_profile": "rust-v1",
    "seed": "0000000000000064",
    "data_manifest_ids": ["sha256:example-data"]
  },
  "resolved_request": {},
  "metrics": {"elapsed_ms": 1250, "peak_working_bytes": 734003200}
}
```

`resolved_request` contains the actual fully defaulted request in a real result. Timing/memory in this example are invented to illustrate fields, not measured performance. Artifact digests are full validated SHA-256 values in real responses. Result schemas discriminate `completed`, `accepted`, and `error`; `accepted` includes a durable job ID.

## 3. Complete operation surface

Each row is required through CLI and MCP. This catalog contains 68 operations; 49 are referenced by the upstream declaration ledger and the others supply agent orchestration and notebook workflow equivalents. A producing operation returns an artifact ID plus compact metadata; query operations page or return small scalar results. A [machine-readable catalog](evidence/operation-catalog.json) records both adapter names and specification status.

### Discovery, validation, and data

| Operation | Inputs | Outputs / behavior |
| --- | --- | --- |
| `system.capabilities` | Optional domain filter | Engine/schema/protocol versions, operations, algorithms, limits, format support, available datasets |
| `system.schema` | Operation ID, input/output selector | Versioned JSON Schema and examples |
| `system.validate` | Operation ID plus request, or typed unit/RNG/geometry value | Errors/warnings and resolved defaults; no expensive simulation or network |
| `system.estimate` | Operation ID plus validated request | Memory, stars, grid/FFT shapes, output-size bounds, missing data, cost category |
| `system.citations` | Optional population/observation/data IDs | Scientific/code references and provenance actually used |
| `data.list` | Provider/version/system/cache filters, cursor | Available model manifests and local verified/completion state |
| `data.fetch` | Dataset identity or approved manifest ID, explicit destination/cache policy | Verified local dataset, hashes, download/extraction metrics; online side effect |
| `data.import` | Local MIST directory/archive or dataset manifest | Validated indexed cache entry without implicit download |
| `data.verify` | Dataset ID, full-vs-metadata verification | Missing/corrupt members and digest results |

### Filters and IMF

| Operation | Inputs | Outputs / behavior |
| --- | --- | --- |
| `filters.list` | Optional system(s), name lookup, cursor | System/filter names, native magnitude reference, supported conversions |
| `filters.inspect` | Filter/system/custom curve IDs, requested properties | Width, effective/pivot wavelength, throughput, curve reference, units |
| `filters.import` | Curve files/inline arrays, column mapping, units, names, optional zero points | Validated custom filter catalog artifact |
| `filters.convert` | Magnitude or color values, source/target system, filter(s), calibration table | Offsets and converted values; missing conversion is explicit |
| `imf.evaluate` | Preset/custom slopes, masses, output `dN_dm`/`m_dN_dm`, normalization and bounds | Values, continuity/normalization metadata; named preset parameters available |
| `imf.integrate` | IMF, mass limits, moment `number`/`mass`, normalization bool/value | Integral and normalization used |
| `imf.sample` | IMF, count, mass bounds, algorithm, seed | Sampled initial-mass artifact and statistics |
| `imf.build` | IMF, target initial mass, batch/stopping policy, bounds, seed | Mass catalog, realized mass, overshoot, draw count |

### Isochrones

| Operation | Inputs | Outputs / behavior |
| --- | --- | --- |
| `isochrone.import` | Generic columns, CSV, PARSEC, MIST `.iso`/`.iso.cmd`; column/age/Zini selectors | Validated isochrone or indexed grid artifact, parser/model metadata |
| `isochrone.mist` | Version, rotation, abundance, log age, [Fe/H], system(s), target magnitude system, data ID | Selected/interpolated isochrone, requested/resolved grid positions, interpolation provenance |
| `isochrone.inspect` | Isochrone/grid ID, summary or page/columns, optional nearest-age request | Full table access, mass domain, filters, evolutionary columns, age-block index |
| `isochrone.interpolate` | ID, x/y columns, query x, selection, interpolation kind/order, bounds/fill policy | Interpolated vector with method and valid domain |
| `isochrone.invert` | ID, magnitude(s), band, system | All mass crossings, row/segment identities, flat-segment status |
| `isochrone.magnitude-limit` | ID, IMF, band, number-or-mass fraction, distance, cutoff frame | Magnitude and mass threshold, sampled fraction |
| `isochrone.nearest` | ID, initial mass values | Nearest masses and row indices, stable tie rule |
| `isochrone.weights` | ID, IMF, normalization type/bounds | Per-row IMF weights and integrals |
| `isochrone.integrated` | ID, IMF, requested bands/colors/metrics, remnant model/bounds | SSP magnitude/color/SBF, mean current mass, live/remnant fractions |

### Populations and sources

| Operation | Inputs | Outputs / behavior |
| --- | --- | --- |
| `population.build` | Isochrone reference or MIST request, count/current mass, IMF, distance, extinction, cutoff, remnant/stopping options, seed | Population artifact, sampled/unresolved accounting, realized mass/flux statistics |
| `population.compose` | Ordered population IDs, component labels | Composite artifact, remapped IDs, per-component fractions and moments |
| `population.inspect` | ID, metric/metadata selector | Mass definitions, distance modulus, count/fractions, component metadata, cutoff and provenance |
| `population.query` | ID, columns/bands, absolute/apparent mode, selection, page | Catalog rows/column artifact; component and original star IDs retained |
| `population.photometry` | ID, bands/colors/metrics, selection, unresolved inclusion policy | Total/mean/integrated-component/SBF magnitude, colors, moments and validity |
| `population.update` | ID, new distance and/or scalar/per-band extinction | New population artifact without resampling; changed cutoff consistency reported |
| `population.sample-fraction` | Population/isochrone ID, IMF, magnitude limit/band/frame | Mass cutoff and sampled number/mass fractions without construction |
| `population.phases` | ID or EEP array, phase/list/label mode | Available phases, selection-mask artifact, counts or paged labels |
| `population.surface-density` | Surface brightness, mean absolute magnitude, distance, pixel scale | Expected stars per square arcsec and per pixel |
| `source.build` | Catalog+positions or population+spatial model; alternatively uniform-SB+isochrone; geometry, labels, seed | Source artifact, retained IDs, smooth components, outside diagnostics; covers all seven exported source constructors |
| `source.compose` | Source IDs, geometry/band checks, `retain_smooth` default true | Scene artifact with preserved component hierarchy; explicit discrete-only mode |
| `source.inspect` | Source ID | Geometry, counts, labels, smooth components, population references, provenance |
| `source.query` | Source ID, coordinate/magnitude/label columns, selection, page | Paged catalog or mask artifact; supports upstream label selection |
| `source.amplitude` | Model parameters/source ID, total magnitude, zero point, pixel scale, normalization | Surface brightness and image amplitude plus parameter name/units |

### Spatial sampling and imaging

| Operation | Inputs | Outputs / behavior |
| --- | --- | --- |
| `spatial.evaluate` | Built-in/model AST and x/y arrays or grid geometry | Model values; covers Constant2D, Plummer2D, Sérsic and ring |
| `spatial.grid-build` | Model or supplied weights, grid geometry, sampler kind, memory budget | Validated reusable weighted-grid/CDF/alias artifact and normalization |
| `spatial.sample` | Grid or Sérsic/Plummer/uniform request, count, physical geometry, offsets, outside policy, seed | Position artifact, generated/retained counts and mask; optional count histogram |
| `psf.build` | Gaussian/Moffat, FWHM+units, pixel scale, odd shape, discretization mode/factor/tolerance | Normalized PSF artifact and diagnostics |
| `psf.import` | Image artifact/FITS plane, origin, normalization policy | Validated empirical PSF artifact |
| `imager.inspect` | Tagged instrument spec or stored observation | Filter set, aperture area, calibration mode, units and validation |
| `imager.convert` | Instrument, mode `mag_counts`/`sky_counts`/`calibration`/`ab_fnu`, values, band, exposure, pixel scale/zpt | Converted values, constants, units |
| `image.histogram` | x/y/weight arrays or catalog, geometry, selection, bin convention | Weighted image or count map, accepted/outside/invalid totals and flux diagnostics |
| `image.smooth` | Image/empty geometry, smooth source components, band, flux/count calibration | Image with integrated light, component amplitudes and captured flux |
| `image.convolve` | Image, PSF, wrap/fill, direct/FFT method | Convolved image with normalization and edge diagnostics |
| `observe.ideal` | Source/scene, band, PSF optional, output zero point, star/component selection, histogram/seeing options | Ideal observation with image and construction metadata |
| `observe.artificial` | Source/scene, instrument, band, exposure, sky, PSF, output zero point, selections, noise seed | Full observation planes and calibrated/expected-variance products |
| `observation.inspect` | ID, metadata/plane/statistics selector, optional bounded cutout | Observation metadata, plane refs, units, min/max/sums and masks |
| `image.embed` | Destination/source images or shapes, center, add/replace/slices-only, clipping policy | New image or exact overlap slices, overlap/lost-flux diagnostics; no WCS reprojection |
| `image.combine` | Image IDs, coefficients, explicit calibration conversion if needed | Checked arithmetic composite; preserves unit/geometry consistency |
| `image.preview` | Image/observation plane, nan-percentiles, colormap, stretch, orientation, size, style | PNG/SVG artifact with display parameters |
| `image.rgb` | Three registered planes, channel order, per-channel scaling and stretch | Native RGB preview artifact; notebook workflow equivalent |
| `plot.render` | Catalog/isochrone ID, kind CMD/HR/spatial/IMF, columns/selections/axis conventions | SVG/PNG plot artifact and plotted data reference |

### Persistence and orchestration

| Operation | Inputs | Outputs / behavior |
| --- | --- | --- |
| `artifact.list` | Kind/filter/page | Retained artifact metadata; never implicit deletion |
| `artifact.inspect` | ID, manifest/member selector | Metadata, digests, schemas, shapes, units, input dependencies |
| `artifact.import` | Portable bundle/CSV/FITS plus kind/schema mapping | Validated artifact; no arbitrary pickle execution |
| `artifact.export` | ID, format/member/plane, destination, overwrite false by default | File references, hashes and bytes; FITS image selection covers upstream `image_type` |
| `artifact.clone` | ID, optional new label/export destination | Immutable alias or physical export; scientific content remains identical |
| `simulate.run` | Self-contained simulation spec or typed dependency plan | Population/source/PSF/observation refs, manifest, metrics and requested exports |
| `job.submit` | Producing operation ID+request, idempotency key, worker endpoint for CLI | Durable accepted job ID; requires long-lived service |
| `job.status` | Job ID | State, stage, progress, estimates, artifact refs/error |
| `job.cancel` | Job ID | Cancellation request acknowledged; final status reports whether completion won the race |
| `job.result` | Job ID | Completed result or typed not-ready/cancelled/failed response |

`mcp serve` and `worker serve` are transport/process bootstrap commands, not scientific operations. All commands in this document are proposed. No operation is allowed to advertise implemented support before its implementation and tests pass.

## 4. Agent workflow examples

Recommended sequence: inspect capabilities → resolve required data → validate/estimate → simulate → inspect summaries/previews → query a small catalog subset → export requested products. Validation and estimation are callable on their own; a valid small simulation request may directly execute without mandatory extra round trips.

Example complete simulation request, saved as `galaxy.json`:

```json
{
  "schema_version": "1",
  "execution": {
    "algorithm_profile": "rust-v1",
    "seed": "0000000000000064",
    "max_memory_bytes": 2147483648,
    "max_stars": 2000000,
    "max_grid_pixels": 10000000
  },
  "population": {
    "isochrone": {
      "kind": "mist",
      "version": "1.2",
      "rotation": 0.4,
      "alpha_enhancement": 0.0,
      "log_age_yr": 8.5,
      "feh": -1.5,
      "photometric_systems": ["LSST"],
      "magnitude_system": "AB"
    },
    "normalization": {"kind": "count", "value": 1000000},
    "imf": {"kind": "kroupa"},
    "distance": {"value": 5, "unit": "Mpc"},
    "extinction_mag": 0,
    "add_remnants": true
  },
  "source": {
    "spatial_model": {
      "kind": "sersic",
      "effective_radius": {"value": 250, "unit": "pc"},
      "n": 0.8,
      "ellipticity": 0.3,
      "theta": {"value": 135, "unit": "deg"},
      "num_effective_radii": 10,
      "sampler": "grid_cdf"
    },
    "geometry": {
      "width": 701,
      "height": 701,
      "pixel_scale_arcsec": 0.2,
      "coordinates": "pixel-centers-v1"
    },
    "offset_pixels": [0, 0],
    "outside": "retain_marked"
  },
  "psf": {
    "kind": "moffat",
    "fwhm": {"value": 0.6, "unit": "arcsec"},
    "alpha": 4.765,
    "width": 41,
    "height": 41,
    "mode": "center"
  },
  "instrument": {
    "kind": "physical",
    "photometric_systems": ["LSST"],
    "diameter": {"value": 6.4, "unit": "m"},
    "efficiency": 1,
    "read_noise_electrons": 4
  },
  "observations": [
    {"bandpass": "LSST_g", "exposure_s": 900, "sky_sb_mag_arcsec2": 22},
    {"bandpass": "LSST_r", "exposure_s": 900, "sky_sb_mag_arcsec2": 21},
    {"bandpass": "LSST_i", "exposure_s": 1800, "sky_sb_mag_arcsec2": 20}
  ],
  "render": {"kind": "artificial", "boundary": "wrap", "output_zpt": 27},
  "outputs": {
    "retain": ["population", "source", "psf", "observations"],
    "export_formats": ["fits", "png"],
    "include_variance": true
  }
}
```

Observation modes may also be `ideal`, which omits instrument/noise requirements. PSF pixel scale inherits the source geometry only through an explicit documented default in the resolved request. Cutoff parameters are optional; absence means full sampling. An operation-generated artifact ID is supplied explicitly to later calls; no hidden “current population” exists.

Proposed CLI usage:

```bash
artpop-rust system capabilities --format json
artpop-rust system schema --operation simulate.run --format json
artpop-rust data fetch --input lsst-v1.2-data.json
artpop-rust system validate --operation simulate.run --input galaxy.json
artpop-rust system estimate --operation simulate.run --input galaxy.json
artpop-rust simulate run --input galaxy.json --output-dir ./runs/galaxy
artpop-rust mcp serve --transport stdio --artifact-dir ./artifacts --data-dir ./models
```

For simple operations, ergonomic flags may mirror the same request:

```bash
artpop-rust imf sample --imf kroupa --count 100000 --seed 000000000000002a
artpop-rust population query --id POPULATION_ID --columns initial_mass,LSST_i --limit 20
```

Flags and a full JSON request cannot ambiguously override each other: input files define the operation request; only documented execution/output flags may override them, and the resolved result records those overrides. Requests can be read from stdin with `--input -`. Batch inputs use a separate explicitly selected JSONL mode with one envelope per request and a defined fail-fast/continue policy.

## 5. CLI behavior

- Default stdout is one JSON result for an operation. `--format text` opts into a human summary. Progress, tracing, dependency notices, and diagnostic logs go to stderr. No ANSI color in machine output.
- `--help`/`--version` are intentional text commands. In JSON operation mode, parser/validation failures use a JSON error envelope and a stable exit code.
- Exit codes: 0 completed/accepted, 2 syntax/schema/domain validation, 3 missing/unavailable data, 4 resource/deadline limit, 5 I/O/network failure, 6 numerical failure, 7 job-state conflict, 130 cancellation. Internal unexpected failures use 1 with a trace/request ID.
- Default operations run in the foreground and retain completed artifacts. Ctrl-C requests cooperative cancellation and returns only committed artifacts. Output writes use atomic staging; existing files are not overwritten unless explicitly requested.
- Job submission from a short-lived CLI requires a configured long-lived worker (`worker serve`, local IPC or configured service endpoint). Do not return an asynchronous job and then kill its only worker by exiting the process.
- Release binaries target Linux/macOS/Windows; scientific behavior is shared. Local worker IPC can use platform-native channels behind the same Rust API.

## 6. MCP behavior

Use the official Rust SDK with stdio as the required first transport. Streamable HTTP is an optional deployment transport after its access-control and dependency gates pass; all scientific functionality is already exposed over stdio. As checked on the analysis date, the latest specification resolves to **2026-07-28** and the official Rust SDK documents support for it plus 2025-11-25 and earlier. Pin and test the revisions actually supported by the release. [MCP specification](https://modelcontextprotocol.io/specification/2026-07-28), [Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)

Do not hand-roll a newline JSON server and label it MCP. Use SDK discovery, version/capability negotiation, tool listing/calls, cancellation, and resource handling for the negotiated revision. In particular, do not hard-code only a historical initialization/session model when supporting newer revisions. Test legacy initialization for clients on older supported revisions and current discovery/negotiation separately.

Each operation appears with strict inputSchema and outputSchema, a concise scientific description, and realistic annotations. Catalog inspection is read-only; artifact-producing tools write local state; fetch is network-capable; overwrite/delete behavior is never described as read-only. An idempotent calculation may still create a cache artifact, so annotations must reflect actual effects.

Use `structuredContent` for the result envelope, accompanied by compact compatible text content. Return file/image resources as resource links and optionally a small requested inline PNG preview. Application validation/numerical/data errors are tool execution failures with `isError: true`; malformed protocol requests use protocol errors. This preserves actionable scientific error details for agents. [Tools specification](https://modelcontextprotocol.io/specification/2026-07-28/server/tools)

Required resource URI patterns:

```text
artpop://capabilities
artpop://schemas/{operation}/{schema_version}
artpop://datasets/{dataset_id}/manifest
artpop://artifacts/{artifact_id}/manifest
artpop://artifacts/{artifact_id}/preview
artpop://jobs/{job_id}/status
```

Large binary members are exported/read through bounded service operations, not embedded wholesale into a resource response. Handles identify durable artifacts/jobs and do not rely on connection-local state. Default retention is until explicit deletion or a configured, advertised retention policy; a response reports expiry where applicable. Unknown/expired handles return typed errors.

Support progress and cancellation through the SDK where the client/revision supports them. Stable `job.*` operations remain the portable fallback. Protocol task extensions can map onto those same jobs when negotiated, but are not necessary to access long-running computations. Avoid elicitation for predictable missing parameters; return the validation error with an example correction.

No simulation needs a callback into an LLM or a model-sampling permission. The server is a scientific tool executor. Optional prompt templates for “build a mock galaxy” or “compare populations” may be added without becoming part of the numerical execution path.

Stdio stdout is reserved exclusively for protocol messages. The server never reads unrelated local files: configured data/artifact/import roots bound its file access. HTTP deployments must authenticate artifact/job access and do not expose arbitrary host paths. These are service design requirements, not extra user confirmation steps in the normal scientific workflow.

## 7. Jobs, retries, and state

Job states are `queued`, `running`, `succeeded`, `failed`, `cancelled`, and `interrupted`. Persist resolved requests and state transitions in the artifact/job store. On restart, mark unfinished non-resumable work interrupted; only advertise resume for stages with tested checkpoints. Do not claim that persisting a job ID makes partially generated RNG/catalog state resumable.

Idempotency keys bind to canonical request hashes. Reusing a key with the same request returns its existing job/result; reusing it with a different request returns `IDEMPOTENCY_CONFLICT`. A download uses a dataset lock and atomic publication. Rendering uses immutable inputs and new output IDs, allowing safe retry after a client disconnect. Job cancellation checks at bounded stages/chunks and never publishes a half-written completed artifact.

Rate/memory/concurrency limits come from service configuration and can be tightened per request. CLI/MCP users see the same limits. A request to increase limits beyond the service configuration fails explicitly.

## 8. Error contract

Example:

```json
{
  "schema_version": "1",
  "status": "error",
  "operation": "source.build",
  "request_id": "req-example",
  "error": {
    "code": "INVALID_PARAMETER",
    "message": "Sersic index must be greater than zero.",
    "path": "/spatial_model/n",
    "details": {"received": 0, "exclusive_minimum": 0},
    "retryable": false
  }
}
```

Stable code families: `INVALID_PARAMETER`, `UNKNOWN_FILTER`, `MISSING_ZEROPOINT`, `UNIT_MISMATCH`, `SHAPE_MISMATCH`, `DATA_MISSING`, `DATA_CORRUPT`, `MODEL_ALIGNMENT`, `OUT_OF_DOMAIN`, `ZERO_WEIGHT_GRID`, `NONFINITE_VALUE`, `RESOURCE_LIMIT`, `NUMERICAL_CONVERGENCE`, `UNSUPPORTED_FORMAT`, `ARTIFACT_NOT_FOUND`, `IO_ERROR`, `NETWORK_ERROR`, `CANCELLED`, `JOB_NOT_READY`, `IDEMPOTENCY_CONFLICT`, and `INTERNAL_ERROR`. Include retryability, affected parameter/artifact, and available alternatives where useful. Never expose a panic traceback as the only explanation.

## 9. Interface acceptance gates

A release claiming full CLI/MCP coverage must demonstrate:

1. Every operation in this catalog has an implemented request schema, executor, and two adapter routes.
2. Every declaration in the coverage ledger maps to a supported operation/parameter or an explicit language-specific replacement with a tested workflow.
3. The same request through CLI and MCP produces the same scientific manifest/content digest, excluding transport metadata and timing.
4. The example simulation runs offline after explicit data setup and produces usable FITS, previews, and queryable catalogs.
5. Error codes/paths, pagination, missing data, cancellation, idempotent retries, and artifact retention behave consistently in both adapters.

Schemas are to be generated and snapshot-tested from Rust types during implementation. The examples and tables here define the intended contract; they are not a claim that executable JSON Schema files or MCP tools have already been built.
