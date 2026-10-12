#!/usr/bin/env bash
# Chain the example requests with --get. Run from the repository root:
#   examples/run.sh [path/to/artpop-rust] [output-dir]
set -euo pipefail
B=${1:-./target/release/artpop-rust}
OUT=${2:-example-output}
A=(--artifact-dir "$OUT/artifacts")

ISO=$("$B" "${A[@]}" isochrone import --input examples/isochrone-import.json --get /result/isochrone_artifact_id)
POP=$("$B" "${A[@]}" population build --input examples/population.json --set "isochrone.artifact_id=$ISO" --seed 000000000000002a --get /result/population_artifact_id)
"$B" "${A[@]}" population photometry --set "artifact_id=$POP" --set 'bands=["LSST_g","LSST_i"]' --format text
SRC=$("$B" "${A[@]}" source build --input examples/source-sersic.json --set "population.artifact_id=$POP" --seed 000000000000002b --get /result/source_artifact_id)
"$B" "${A[@]}" source query --set "artifact_id=$SRC" --set 'columns=["x","y","LSST_i","label"]' --set 'page={"limit":5}' --format text
"$B" "${A[@]}" simulate run --input examples/simulate-artificial.json --set "population.isochrone.artifact_id=$ISO" --set "outputs.directory=$OUT/simulation" --format text
ls "$OUT/simulation"
