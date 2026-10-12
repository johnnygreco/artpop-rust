# Evidence files

Raw outputs behind the claims in the documentation, all from 2026-10-12 on the analysis VM (AMD EPYC 7B13, 4 vCPUs, Linux x86_64).

## Upstream analysis (Python reference only)

| File | Contents |
| --- | --- |
| `upstream-tests.txt`, `mist-tests-initial.txt` | ArtPop's own test suite: 15 tests + 17 subtests, and 3 MIST tests. |
| `probes.json`, `probe-warnings.txt` | 18 boundary/defect probes of ArtPop 0.1.2 ([`analysis/probe_upstream.py`](../../analysis/probe_upstream.py)). |
| `api-inventory.json`, `upstream-assets.json` | Public declarations of the pinned checkout and bundled asset hashes. |
| `python-benchmark.json`, `python-benchmark-log.txt` | First Python-only baseline (superseded by the comparison below). |
| `e2e-tests.txt`, `e2e-required-rust-gate.txt` | Harness results before a Rust binary existed (9 passed, 5 skipped), and proof that `--require-rust` refuses to run without one. |

## Rust implementation

| File | Contents |
| --- | --- |
| `rust-tests.txt` | `cargo test --workspace` output for the final source: 85 passed. |
| `clippy.txt` | `cargo clippy --workspace --all-targets` output (no warnings). |
| `e2e-rust-parity.txt` | `pytest validation --require-rust -rA` against the final release binary (SHA-256 `08bfee21…`): 36 passed (MIST comparisons enabled). |
| `rust-vs-python-benchmark.json` | Binary `fe53d95e…`; code-identical to the final build, see [BENCHMARK_RESULTS.md](../BENCHMARK_RESULTS.md#results). Interleaved fresh-process timings of the five reference workflows (Python and Rust, raw samples, RSS, load average, per-sample validation). |
| `rust-vs-python-scale-benchmark.json` | Same for the five larger workflows, with statistical consistency metrics. |
| `data-fetch-lsst.json` | Live `data.fetch` of the MIST v1.2 LSST rotation-0.4 grid from mist.science (archive SHA-256 and sizes). |
| `operation-catalog.json` | The 68 operations with CLI/MCP names, implementation status, and the test files that exercise them. |
