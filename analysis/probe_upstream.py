"""Analysis-only probes of the pinned Python checkout; not port implementation.

Run with PYTHONPATH=../ArtPop/src and the temporary analysis environment.
Writes JSON to stdout; intentionally records failures as evidence.
"""
import importlib.metadata
import json
import pickle
import sys
from pathlib import Path
from unittest.mock import patch

import numpy as np
from astropy.table import Table
from astropy import units as u
import artpop
from artpop.stars import isochrones


ROOT = Path(__file__).resolve().parents[2]
results = {}


def record(name, fn):
    try:
        results[name] = {"result": fn()}
    except Exception as exc:
        results[name] = {"error": type(exc).__name__, "message": str(exc)}


with open(Path(artpop.data_dir) / "feh_m1.00_vvcrit0.4_LSST_10gyr_test_iso.pkl", "rb") as f:
    fixture = Table(pickle.load(f))
filters = artpop.get_filter_names("LSST")
iso = artpop.Isochrone(fixture["initial_mass"], fixture["star_mass"], fixture[filters])


def hist_edges():
    imager = artpop.IdealImager()
    out = []
    for x in [-0.01, 0, 0.79, 0.8, 1, 3.99, 4, 4.99, 5]:
        a = imager.inject_stars(np.array([x]), np.array([1.0]), np.array([1.0]), [5, 5])
        out.append({"x": x, "sum": float(a.sum()), "yx": np.argwhere(a).tolist()})
    return out


record("histogram_edges", hist_edges)
record("grid_float_offset", lambda: artpop.xy_from_grid(2, artpop.Constant2D(), 5, dx=0.5, random_state=1).tolist())
record("grid_same_size_shift", lambda: artpop.xy_from_grid(1000, artpop.Constant2D(), 5, dx=1, random_state=1).max(axis=0).tolist())
record("grid_asymmetric_orientation", lambda: artpop.xy_from_grid(3, lambda x, y: ((x == 1) & (y == 3)).astype(float), 5, random_state=1).tolist())
record("imf_minus_one", lambda: artpop.IMFIntegrator({"a": -1.0}).integrate(0.1, 1))
record("imf_minus_two", lambda: artpop.IMFIntegrator({"a": -2.0}).m_integrate(0.1, 1))
record("instrument_sky_pixel_scale", lambda: [artpop.ArtImager(zpt_inst={"f": 25}).sb_to_counts_per_pixel(25, "f", 100, s * u.arcsec / u.pixel) for s in [0.2, 0.4]])
record("instrument_precedence", lambda: {"zpt_inst": artpop.ArtImager(phot_system="LSST", zpt_inst={"f": 25}).zpt_inst})


def filter_constructor():
    f = artpop.FilterSystem([str(ROOT / "ArtPop/filter_curves/LSST/LSST_i.csv")], ["f"], delimiter=",", skiprows=1)
    return artpop.ArtImager(filter_system=f).filters


record("filter_system_constructor", filter_constructor)


def population_pair():
    full = artpop.SSP(iso, num_stars=10000, random_state=2)
    part = artpop.SSP(iso, num_stars=10000, mag_limit=5, mag_limit_band="LSST_i", random_state=2)
    return full, part


record("composite_full_plus_partial", lambda: (population_pair()[0] + population_pair()[1]).total_mag("LSST_i"))


def partial_extinction():
    kw = dict(num_stars=10000, mag_limit=5, mag_limit_band="LSST_i", random_state=2)
    a, b = artpop.SSP(iso, **kw), artpop.SSP(iso, a_lam=1, **kw)
    return {"sbf_change_for_one_mag_extinction": float(b.sbf_mag("LSST_i") - a.sbf_mag("LSST_i")), "total_mag_change": float(b.total_mag("LSST_i") - a.total_mag("LSST_i")), "sampled": a.num_stars, "integrated": int(a.num_stars_integrated)}


record("unresolved_sbf_extinction", partial_extinction)


def uniform_imf():
    counts = {}
    for imf in ["kroupa", "salpeter"]:
        src = artpop.UniformSSP(iso, 0.00001, 5, 0.2, 0, "LSST_i", imf=imf, random_state=1)
        counts[imf] = {"requested_stars": src.ssp_kw["num_stars"], "expected_mean_mag": float(iso.ssp_mag("LSST_i", imf=imf, norm_type="number"))}
    return counts


record("uniform_imf_normalization", uniform_imf)


def rotation_forwarding():
    calls = []
    def fetch(*args, **kwargs):
        calls.append({"positional": list(args), "keyword": kwargs})
        return np.array([(1, 1.0), (2, 2.0)], dtype=[("EEP", "i4"), ("initial_mass", "f8")])
    obj = object.__new__(artpop.MISTIsochrone)
    obj.feh, obj.log_age, obj.mist_path, obj.v_over_vcrit = -1.1, 10, "/tmp/unused", 0.0
    with patch.object(isochrones, "fetch_mist_iso_cmd", fetch):
        obj._interp_on_feh("LSST")
    return calls


record("mist_interpolation_rotation_forwarding", rotation_forwarding)
record("remnants_restricted_upper_mass", lambda: {"with": float(iso.ssp_surviving_mass("kroupa", m_max=2)), "without": float(iso.ssp_surviving_mass("kroupa", m_max=2, add_remnants=False))})


def negative_variance():
    src = artpop.Source(np.empty((0, 2)), {"f": np.array([])}, 5, pixel_scale=0.2)
    obs = artpop.ArtImager(zpt_inst={"f":25}, read_noise=0.1, random_state=1).observe(src, "f", 1)
    return {"min_variance": float(obs.var_image.min()), "negative_pixels": int((obs.var_image < 0).sum())}


record("negative_observed_variance", negative_variance)
record("partially_masked_coordinates", lambda: artpop.Source(np.ma.array([[1.,2.]], mask=[[True,False]]), {"f":[25.]}, 5).num_stars)
record("mist_age_grid", lambda: {"min": float(artpop.MISTIsochrone._log_age_min), "max": float(artpop.MISTIsochrone._log_age_max), "count": len(artpop.MISTIsochrone._log_age_grid)})

names = artpop.get_filter_names()
props = artpop.get_filter_properties()
curves = list((ROOT / "ArtPop/filter_curves").rglob("*.csv"))
record("data_inventory", lambda: {"systems": {k: len(v) for k,v in names.items()}, "filter_name_entries": sum(map(len,names.values())), "filter_properties_rows": len(props), "curve_files": len(curves), "fixture_rows": len(fixture), "fixture_columns": fixture.colnames, "missing_curves_by_stem": sorted(set(sum(names.values(),[])) - {p.stem for p in curves})})

print(json.dumps({"python":sys.version, "packages":{p:importlib.metadata.version(p) for p in ["numpy","scipy","astropy","matplotlib","fast-histogram","pytest","requests"]}, "probes":results}, indent=2, allow_nan=False))
