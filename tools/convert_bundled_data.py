"""Convert ArtPop's bundled pickle/text metadata into reviewable JSON for artpop-rust.

Development tooling only: the Rust build and runtime read the generated JSON and
never execute Python or unpickle data. Run from the artpop-rust directory with the
reference environment; the pinned upstream checkout must be at ../ArtPop.
"""
import hashlib
import json
import pickle
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
UPSTREAM = ROOT.parent / "ArtPop"
DATA = UPSTREAM / "src/artpop/data"
OUT = ROOT / "data"
FIXTURES = ROOT / "fixtures"
COMMIT = "38a886906ed175d0205bf48b6dc03f54aeb4a81a"


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def load(name):
    # Trusted pinned repository assets, never user uploads.
    with open(DATA / name, "rb") as f:
        return pickle.load(f)


def dump(path, value):
    path.write_text(json.dumps(value, indent=1, allow_nan=False) + "\n")


def main():
    OUT.mkdir(exist_ok=True)
    FIXTURES.mkdir(exist_ok=True)
    names = load("mist_filter_names.pkl")
    systems = ["HST_WFC3", "HST_ACSWF", "SDSSugriz", "CFHTugriz", "DECam", "HSC",
               "JWST", "LSST", "UBVRIplus", "UKIDSS", "WFIRST", "GALEX"]
    assert sorted(systems) == sorted(names), names.keys()
    props = load("filter_properties.pkl")
    lookup = load("phot_system_lookup.pkl")
    filters = []
    for system in systems:
        for band in names[system]:
            i = props["bandpass"].index(band)
            filters.append({"name": band, "system": system, "dlam_angstrom": float(props["dlam"][i]),
                            "lam_eff_angstrom": float(props["lam_eff"][i])})
            assert lookup[band] == system
    assert len(filters) == len(props["bandpass"]) == len(lookup) == 164
    zeropoints = []
    for line in (DATA / "zeropoints.txt").read_text().splitlines()[1:]:
        band, system, vega_st, vega_ab = line.split()
        zeropoints.append({"filter": band, "native_system": system, "vega_to_st": float(vega_st),
                           "vega_to_ab": float(vega_ab)})
    dump(OUT / "filters.json", {"schema_version": "1", "systems": systems, "filters": filters})
    dump(OUT / "zeropoints.json", {"schema_version": "1", "columns": "filter native_system mag(Vega/ST) mag(Vega/AB)",
                                   "zeropoints": zeropoints})

    iso = load("feh_m1.00_vvcrit0.4_LSST_10gyr_test_iso.pkl")
    columns = [c for c in iso if c != "phase"] + ["phase"]
    lines = [",".join(columns)]
    for row in zip(*[iso[c] for c in columns]):
        lines.append(",".join(repr(float(v)) if not isinstance(v, int) else str(v) for v in row))
    fixture = FIXTURES / "lsst_feh_m1.00_10gyr_iso.csv"
    fixture.write_text("\n".join(lines) + "\n")

    sources = ["mist_filter_names.pkl", "filter_properties.pkl", "phot_system_lookup.pkl", "zeropoints.txt",
               "feh_m1.00_vvcrit0.4_LSST_10gyr_test_iso.pkl"]
    dump(OUT / "provenance.json", {
        "schema_version": "1",
        "upstream": {"repository": "https://github.com/ArtificialStellarPopulations/ArtPop", "commit": COMMIT,
                     "license": "MIT"},
        "generator": "tools/convert_bundled_data.py",
        "python": sys.version.split()[0],
        "sources": {name: sha256(DATA / name) for name in sources},
        "outputs": {p.relative_to(ROOT).as_posix(): sha256(p) for p in
                    [OUT / "filters.json", OUT / "zeropoints.json", fixture]},
        "notes": "MIST filter metadata and zero points originate from the MIST project (https://mist.science); "
                 "redistribution terms of external model data are tracked separately from the ArtPop MIT license.",
    })
    print(f"wrote {len(filters)} filters, {len(zeropoints)} zero points, fixture rows {len(lines) - 1}")


if __name__ == "__main__":
    main()
