"""Larger equivalent workflows for Python-vs-Rust timing (scale_benchmark.py).

Each case runs a complete user-level workflow through the public API (Python)
or the public CLI (Rust), writing comparable products. Correctness at these
sizes is checked loosely here (counts, flux conservation, photometric
agreement); exact parity is established by test_e2e.py/test_extended_parity.py.
"""
import argparse
import json
import os
from pathlib import Path
import sys

from harness import ROOT, RustCli, dump, reference_imports

CASES = ("population_1m", "sersic_ideal_1m", "artificial_2k", "uniform_hist_10m", "mist_mass_cutoff")
FIXTURE = ROOT / "artpop-rust" / "fixtures" / "lsst_feh_m1.00_10gyr_iso.csv"
BANDS = ["LSST_g", "LSST_i"]
MIST_SYSTEM_DIR = "MIST_v1.2_vvcrit0.4_LSST"


def python_iso(artpop, np):
    a = np.genfromtxt(FIXTURE, delimiter=",", names=True)
    return artpop.Isochrone(a["initial_mass"], a["star_mass"], {b: a[b] for b in BANDS})


def run_python(case, out):
    artpop, np, fits = reference_imports()
    from astropy import units as u
    out = Path(out)
    out.mkdir(parents=True, exist_ok=True)
    if case == "population_1m":
        ssp = artpop.SSP(python_iso(artpop, np), num_stars=1_000_000, distance=10 * u.pc, random_state=1)
        dump(out / "summary.json", {"num_stars": int(ssp.num_stars), "total_mass_msun": float(ssp.total_mass.value), "total_mag": {b: float(ssp.total_mag(b)) for b in BANDS}, "sbf_mag": {b: float(ssp.sbf_mag(b)) for b in BANDS}})
    elif case == "sersic_ideal_1m":
        ssp = artpop.SSP(python_iso(artpop, np), num_stars=1_000_000, distance=5, random_state=2)
        src = artpop.SersicSP(ssp, r_eff=0.218, n=1.0, theta=30, ellip=0.3, xy_dim=801, pixel_scale=0.2)
        psf = artpop.moffat_psf(0.7, pixel_scale=0.2, shape=41)
        obs = artpop.IdealImager().observe(src, "LSST_i", psf=psf, zpt=27)
        obs.to_fits(out / "image.fits")
        dump(out / "summary.json", {"image_sum": float(obs.image.sum()), "stars": int(src.num_stars)})
    elif case == "artificial_2k":
        ssp = artpop.SSP(python_iso(artpop, np), num_stars=1_000_000, distance=5, random_state=3)
        src = artpop.PlummerSP(ssp, scale_radius=0.05, xy_dim=2047, pixel_scale=0.2)
        psf = artpop.moffat_psf(0.7, pixel_scale=0.2, shape=41)
        imager = artpop.ArtImager("LSST", diameter=6.4, read_noise=4, random_state=4)
        obs = imager.observe(src, "LSST_i", exptime=900, sky_sb=21, psf=psf, zpt=27)
        for name, data in [("image", obs.image), ("source_counts", obs.src_counts), ("raw_counts", obs.raw_counts)]:
            fits.writeto(out / f"{name}.fits", data, overwrite=True)
        dump(out / "summary.json", {"source_counts": float(obs.src_counts.sum()), "sky_counts": float(obs.sky_counts)})
    elif case == "uniform_hist_10m":
        xy = artpop.uniform_xy(10_000_000, 2047, random_state=5)
        img = artpop.IdealImager().inject_stars(xy[:, 0], xy[:, 1], np.ones(len(xy)), np.array([2047, 2047]))
        fits.writeto(out / "image.fits", img, overwrite=True)
        dump(out / "summary.json", {"deposited": float(img.sum())})
    elif case == "mist_mass_cutoff":
        ssp = artpop.MISTSSP(10.0, -1.5, "LSST", total_mass=1e7, distance=5, mag_limit=29.0, mag_limit_band="LSST_i", mist_path=os.environ["MIST_PATH"], random_state=6)
        dump(out / "summary.json", {"num_stars": int(ssp.num_stars), "total_mass_msun": float(ssp.total_mass.value), "total_mag": {b: float(ssp.total_mag(b)) for b in BANDS}, "sbf_mag": {b: float(ssp.sbf_mag(b)) for b in BANDS}})
    else:
        raise ValueError(case)


def run_rust(case, out, binary, data_dir):
    out = Path(out).resolve()
    cli = RustCli(binary, out / "calls")
    if data_dir:
        cli.extra_args = ["--data-dir", str(data_dir)]

    def iso():
        r = cli.invoke("isochrone.import", format="csv", path=str(FIXTURE), magnitude_columns={b: b for b in BANDS}, magnitude_system="AB")
        return cli.ref(r, "isochrone")

    def pop(n, d_mpc, seed):
        r = cli.invoke("population.build", isochrone=iso(), normalization={"kind": "count", "value": n}, distance={"value": d_mpc, "unit": "Mpc"}, execution={"seed": seed})
        return cli.ref(r, "population")

    psf = lambda: cli.ref(cli.invoke("psf.build", kind="moffat", fwhm={"value": 0.7, "unit": "arcsec"}, pixel_scale_arcsec=0.2, width=41, height=41), "psf")
    if case == "population_1m":
        p = cli.ref(cli.invoke("population.build", isochrone=iso(), normalization={"kind": "count", "value": 1_000_000}, distance={"value": 10, "unit": "pc"}, execution={"seed": "0000000000000001"}), "population")
        phot = cli.invoke("population.photometry", **p, bands=BANDS, metrics=["total_mag", "sbf_mag"])["result"]
        info = cli.invoke("population.inspect", **p, fields=["num_stars_discrete", "total_mass_msun"])["result"]
        dump(out / "summary.json", {"num_stars": info["num_stars_discrete"], "total_mass_msun": info["total_mass_msun"], "total_mag": phot["total_mag"], "sbf_mag": phot["sbf_mag"]})
    elif case == "sersic_ideal_1m":
        p = pop(1_000_000, 5, "0000000000000002")
        s = cli.ref(cli.invoke("source.build", population=p, spatial_model={"kind": "sersic", "effective_radius": {"value": 218, "unit": "pc"}, "n": 1.0, "ellipticity": 0.3, "theta": {"value": 30, "unit": "deg"}}, geometry={"width": 801, "height": 801, "pixel_scale_arcsec": 0.2}, execution={"seed": "0000000000000003"}), "source")
        r = cli.invoke("observe.ideal", source=s, bandpass="LSST_i", psf=psf(), output_zpt=27)
        cli.export(cli.ref(r, "observation"), out / "image.fits", "fits")
        dump(out / "summary.json", {"image_sum": r["result"]["image_sum"], "stars": r["result"]["histogram"]["deposited"]})
    elif case == "artificial_2k":
        p = pop(1_000_000, 5, "0000000000000004")
        s = cli.ref(cli.invoke("source.build", population=p, spatial_model={"kind": "plummer", "scale_radius": {"value": 50, "unit": "pc"}}, geometry={"width": 2047, "height": 2047, "pixel_scale_arcsec": 0.2}, execution={"seed": "0000000000000005"}), "source")
        r = cli.invoke("observe.artificial", source=s, instrument={"kind": "physical", "photometric_systems": ["LSST"], "diameter": {"value": 6.4, "unit": "m"}, "read_noise_electrons": 4}, bandpass="LSST_i", exposure_s=900, sky_sb_mag_arcsec2=21, psf=psf(), output_zpt=27, execution={"seed": "0000000000000006"})
        obs = cli.ref(r, "observation")
        for plane in ["image", "source_counts", "raw_counts"]:
            cli.export(obs, out / f"{plane}.fits", "fits", plane=plane)
        dump(out / "summary.json", {"source_counts": r["result"]["expected_source_counts"], "sky_counts": r["result"]["sky_counts"]})
    elif case == "uniform_hist_10m":
        geom = {"width": 2047, "height": 2047, "coordinates": "artpop-0.1.2"}
        pos = cli.ref(cli.invoke("spatial.sample", distribution={"kind": "uniform"}, count=10_000_000, geometry=geom, execution={"seed": "0000000000000007"}), "positions")
        r = cli.invoke("image.histogram", positions=pos)
        cli.export(cli.ref(r, "image"), out / "image.fits", "fits")
        dump(out / "summary.json", {"deposited": r["result"]["sum"]})
    elif case == "mist_mass_cutoff":
        r = cli.invoke("population.build", isochrone={"kind": "mist", "log_age_yr": 10.0, "feh": -1.5, "photometric_systems": ["LSST"]}, normalization={"kind": "current_mass", "value": 1e7}, distance={"value": 5, "unit": "Mpc"}, mag_limit={"magnitude": 29.0, "band": "LSST_i", "frame": "intrinsic_apparent"}, mass_tolerance=0.05, execution={"seed": "0000000000000008"})
        p = cli.ref(r, "population")
        phot = cli.invoke("population.photometry", **p, bands=BANDS, metrics=["total_mag", "sbf_mag"])["result"]
        dump(out / "summary.json", {"num_stars": r["result"]["num_stars_discrete"], "total_mass_msun": r["result"]["total_mass_msun"], "total_mag": phot["total_mag"], "sbf_mag": phot["sbf_mag"]})
    else:
        raise ValueError(case)


def flux_moments(band):
    """Mean and variance of one star's absolute flux under the Kroupa IMF
    (number-normalized cell weights of the fixture isochrone)."""
    artpop, np, _ = reference_imports()
    a = np.genfromtxt(FIXTURE, delimiter=",", names=True)
    iso = artpop.Isochrone(a["initial_mass"], a["star_mass"], {band: a[band]})
    w = iso.imf_weights("kroupa", norm_type="number", m_max_norm=iso.m_max)
    f = 10 ** (-0.4 * a[band])
    mu = np.sum(w * f) / np.sum(w)
    return mu, np.sum(w * f * f) / np.sum(w) - mu * mu


def check(case, output, reference):
    """Statistical cross-backend consistency at scale. The two backends draw
    independent realizations, so totals are compared against the expected
    realization scatter (6 sigma); exact parity is tested elsewhere."""
    import numpy as np
    a = json.loads((Path(output) / "summary.json").read_text())
    b = json.loads((Path(reference) / "summary.json").read_text())
    metrics = {}
    n = 1_000_000
    if case == "population_1m":
        for band in BANDS:
            mu, var = flux_moments(band)
            for name, s in [("rust", a), ("python", b)]:
                z = (10 ** (-0.4 * s["total_mag"][band]) - n * mu) / np.sqrt(n * var)
                metrics[f"z_total_flux_{band}_{name}"] = float(z)
                assert abs(z) < 6, metrics
            assert np.isfinite(a["sbf_mag"][band])
        assert abs(a["total_mass_msun"] / b["total_mass_msun"] - 1) < 0.01
    elif case == "mist_mass_cutoff":
        for band in BANDS:
            metrics[f"total_mag_diff_{band}"] = a["total_mag"][band] - b["total_mag"][band]
            assert abs(metrics[f"total_mag_diff_{band}"]) < 0.05, metrics
        assert abs(a["total_mass_msun"] / b["total_mass_msun"] - 1) < 0.06
    elif case in ("sersic_ideal_1m", "artificial_2k"):
        key = "image_sum" if case == "sersic_ideal_1m" else "source_counts"
        mu, var = flux_moments("LSST_i")
        rel = np.sqrt(var / n) / mu
        metrics[f"{key}_ratio"] = a[key] / b[key]
        metrics["realization_rel_sigma"] = float(rel)
        assert abs(metrics[f"{key}_ratio"] - 1) < 6 * np.sqrt(2) * rel, metrics
        if case == "artificial_2k":
            assert abs(a["sky_counts"] / b["sky_counts"] - 1) < 1e-9
    elif case == "uniform_hist_10m":
        metrics["deposited_ratio"] = a["deposited"] / b["deposited"]
        # Legacy bins drop x or y in [W-1, W): about 2/2047 of the stars.
        assert abs(metrics["deposited_ratio"] - 1) < 2e-3
    return metrics


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--backend", choices=["python", "rust"], required=True)
    p.add_argument("--case", choices=CASES, required=True)
    p.add_argument("--output", type=Path, required=True)
    p.add_argument("--rust-bin", type=Path)
    p.add_argument("--data-dir", type=Path)
    a = p.parse_args()
    if a.backend == "python":
        run_python(a.case, a.output)
    else:
        run_rust(a.case, a.output, a.rust_bin, a.data_dir)
    print(json.dumps({"status": "completed", "case": a.case, "backend": a.backend}))


if __name__ == "__main__":
    main()
