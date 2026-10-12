"""Extended Python-reference vs Rust parity checks.

Deterministic functions are compared value-by-value against pinned ArtPop
0.1.2; stochastic samplers are compared statistically. Every test needs a real
binary (``--rust-bin``) and is skipped otherwise; skips are never parity passes.
Documented rust-v1 corrections (VALIDATION_PLAN section 4) are asserted as
differences where they apply, not hidden by loosened tolerances.
"""
import json
import os
from pathlib import Path
import pickle
import subprocess

import numpy as np
import pytest

from harness import ROOT, reference_imports

FIXTURE = ROOT / "artpop-rust" / "fixtures" / "lsst_feh_m1.00_10gyr_iso.csv"
BANDS = ["LSST_u", "LSST_g", "LSST_r", "LSST_i", "LSST_z", "LSST_y"]


@pytest.fixture(scope="module")
def rust(request, tmp_path_factory):
    binary = request.config.getoption("--rust-bin")
    if binary is None:
        pytest.skip("Rust implementation not supplied; this is not a parity pass")
    store = tmp_path_factory.mktemp("rust-store")
    data_dir = os.environ.get("ARTPOP_PARITY_DATA_DIR")

    def call(op, **req):
        args = [str(Path(binary).resolve()), "--artifact-dir", str(store)]
        if data_dir:
            args += ["--data-dir", data_dir]
        p = subprocess.run(args + ["invoke", op, "--input", "-"], input=json.dumps({"schema_version": "1", **req}), capture_output=True, text=True, timeout=300)
        env = json.loads(p.stdout)
        assert env["status"] == "completed", (op, env.get("error"))
        return env["result"]
    call.store = store
    return call


@pytest.fixture(scope="module")
def ap():
    artpop, np_, fits = reference_imports()
    return artpop


@pytest.fixture(scope="module")
def iso(ap):
    with open(Path(ap.data_dir) / "feh_m1.00_vvcrit0.4_LSST_10gyr_test_iso.pkl", "rb") as f:
        t = pickle.load(f)  # trusted pinned repository fixture
    return ap.Isochrone(mini=t["initial_mass"], mact=t["star_mass"], mags={b: t[b] for b in BANDS}, eep=t["EEP"], log_L=t["log_L"], log_Teff=t["log_Teff"])


@pytest.fixture(scope="module")
def iso_id(rust):
    r = rust("isochrone.import", format="csv", path=str(FIXTURE), magnitude_columns={b: b for b in BANDS}, magnitude_system="AB", eep_column="EEP", log_l_column="log_L", log_teff_column="log_Teff")
    return r["isochrone_artifact_id"]


IMFS = {"kroupa": {"kind": "kroupa"}, "scalo": {"kind": "scalo"}, "salpeter": {"kind": "salpeter"}}


@pytest.mark.parametrize("name", ["kroupa", "scalo", "salpeter", "custom"])
def test_imf_integrals_and_weights(rust, ap, name):
    if name == "custom":
        py, spec = {"a": [-0.7, -1.6, -2.1], "b": [0.3, 2.0]}, {"kind": "broken_power_law", "slopes": [-0.7, -1.6, -2.1], "breaks": [0.3, 2.0]}
    else:
        py, spec = name, IMFS[name]
    integ = ap.IMFIntegrator(py)
    for lo, hi in [(0.1, 120.0), (0.08, 0.5), (0.4, 3.0), (1.0, 50.0)]:
        n = rust("imf.integrate", imf=spec, m_min=lo, m_max=hi, moment="number")["raw_integral"]
        m = rust("imf.integrate", imf=spec, m_min=lo, m_max=hi, moment="mass")["raw_integral"]
        # VALIDATION_PLAN tolerance for f64 integrals (1e-10 * scale): ArtPop
        # differences indefinite integrals anchored at 1e-3 Msun and loses
        # ~1e-12 relative precision to cancellation for steep slopes.
        np.testing.assert_allclose(n, integ.integrate(lo, hi), rtol=1e-10)
        np.testing.assert_allclose(m, integ.m_integrate(lo, hi), rtol=1e-10)
    masses = list(np.logspace(-1.5, 2, 37))
    for norm in [None, "sum", "number", "mass"]:
        r = rust("imf.evaluate", imf=spec, masses=masses, normalization=norm or "none")
        np.testing.assert_allclose(r["values"], integ.weights(np.array(masses), norm_type=norm), rtol=1e-12)


PRESETS = {"kroupa": ([-0.3, -1.3, -2.3], [0.08, 0.5]), "scalo": ([-1.2, -2.7, -2.3], [1.0, 10.0]), "salpeter": ([-2.35], [])}


def oracle_mass_weights(mini, name, norm_moment=1):
    """IMF cell weights at 50 significant digits (mpmath), normalized by the
    mass integral (or the number integral with ``norm_moment=0``), using the
    same f64 cell boundaries as both implementations: an independent oracle."""
    import mpmath
    mpmath.mp.dps = 50
    a, b = PRESETS[name]
    a = [mpmath.mpf(repr(x)) for x in a]
    b = [mpmath.mpf(repr(x)) for x in b]
    c = [(b[0] if b else mpmath.mpf(301)) ** (-a[0])]
    for i in range(1, len(a)):
        c.append(c[-1] * b[i - 1] ** (a[i - 1] - a[i]))
    edges = [mpmath.mpf(0)] + b + [mpmath.inf]

    def integ(lo, hi, k):
        lo, hi = mpmath.mpf(float(lo)), mpmath.mpf(float(hi))
        tot = mpmath.mpf(0)
        for s in range(len(a)):
            x0, x1 = max(lo, edges[s]), min(hi, edges[s + 1])
            if x1 > x0:
                q = a[s] + 1 + k
                tot += c[s] * (x1 ** q - x0 ** q) / q
        return tot
    n = len(mini)
    w = [integ(mini[0] if i == 0 else mini[i] - 0.5 * (mini[i] - mini[i - 1]), mini[i] if i == n - 1 else mini[i] + 0.5 * (mini[i + 1] - mini[i]), 0) for i in range(n)]
    norm = integ(mini[0], mini[-1], norm_moment)
    return np.array([float(x / norm) for x in w]), [x / norm for x in w]


def test_isochrone_integrated_quantities(rust, iso, iso_id):
    mini = np.asarray(iso.mini, dtype=np.float64)
    for imf in ["kroupa", "scalo", "salpeter"]:
        r = rust("isochrone.integrated", artifact_id=iso_id, imf=IMFS[imf], colors=[["LSST_u", "LSST_y"], ["LSST_g", "LSST_i"]])
        import mpmath
        _, w = oracle_mass_weights(mini, imf)
        for b in BANDS:
            f = [mpmath.mpf(10) ** (mpmath.mpf(-0.4) * mpmath.mpf(float(m))) for m in iso.mag_table[b]]
            s1 = mpmath.fsum(wi * fi for wi, fi in zip(w, f))
            s2 = mpmath.fsum(wi * fi * fi for wi, fi in zip(w, f))
            oracle_mag = float(-2.5 * mpmath.log10(s1))
            oracle_sbf = float(-2.5 * mpmath.log10(s2 / s1))
            # Rust vs 50-digit oracle: 1e-12 mag.
            np.testing.assert_allclose([r["ssp_mag"][b], r["sbf_mag"][b]], [oracle_mag, oracle_sbf], rtol=0, atol=1e-12)
            # Rust vs ArtPop 0.1.2: bounded by ArtPop's own cancellation error
            # (measured up to 2.2e-7 mag for SBF against the same oracle).
            np.testing.assert_allclose(r["ssp_mag"][b], iso.ssp_mag(b, imf=imf), rtol=0, atol=1e-8)
            np.testing.assert_allclose(r["sbf_mag"][b], iso.ssp_sbf_mag(b, imf=imf), rtol=0, atol=5e-7)
        np.testing.assert_allclose(r["colors"]["LSST_u-LSST_y"], iso.ssp_color("LSST_u", "LSST_y", imf=imf), atol=1e-8)
        np.testing.assert_allclose(r["mean_star_mass_msun"], iso.ssp_mean_star_mass(imf), rtol=1e-9)
        np.testing.assert_allclose(r["surviving_mass"]["with_remnants"]["total"], iso.ssp_surviving_mass(imf), rtol=1e-9)
        np.testing.assert_allclose(r["surviving_mass"]["live_only"], iso.ssp_surviving_mass(imf, add_remnants=False), rtol=1e-9)
        n = rust("isochrone.integrated", artifact_id=iso_id, imf=IMFS[imf], metrics=["ssp_mag"], norm_type="number")
        np.testing.assert_allclose(n["ssp_mag"]["LSST_i"], iso.ssp_mag("LSST_i", imf=imf, norm_type="number"), atol=1e-8)
        wn = rust("isochrone.weights", artifact_id=iso_id, imf=IMFS[imf], norm_type="number", m_max_norm=float(iso.m_max))
        # Rust vs 50-digit oracle: relative 1e-12 on every cell. Rust vs
        # ArtPop: absolute 1e-12, since ArtPop's tiny-cell integrals cancel
        # (relative errors up to ~3e-2 on weights of order 1e-11).
        oracle, _ = oracle_mass_weights(mini, imf, norm_moment=0)
        np.testing.assert_allclose(wn["weights"], oracle, rtol=1e-12, atol=0)
        np.testing.assert_allclose(wn["weights"], iso.imf_weights(imf, m_max_norm=iso.m_max, norm_type="number"), rtol=0, atol=1e-12)


def test_magnitude_limit_inversion_and_interpolation(rust, ap, iso, iso_id):
    for kw in [{"frac_num_sampled": 0.1}, {"frac_mass_sampled": 0.4}, {"frac_num_sampled": 0.9}]:
        for d in [10.0, 2.0e6]:
            r = rust("isochrone.magnitude-limit", artifact_id=iso_id, imf={"kind": "kroupa"}, band="LSST_i", distance={"value": d, "unit": "pc"}, **kw)
            mag, mass = iso.calculate_mag_limit("kroupa", "LSST_i", distance=d / 1e6, **kw)
            np.testing.assert_allclose([r["mag_limit"], r["mass_limit_msun"]], [mag, mass], rtol=1e-12)
    for m in [-1.0, 2.0, 4.5, 9.0]:
        rs = sorted(c["initial_mass"] for c in rust("isochrone.invert", artifact_id=iso_id, band="LSST_i", magnitudes=[m])["results"][0]["crossings"])
        py = sorted(set(np.round(iso.mag_to_mass(m, "LSST_i"), 14)))
        np.testing.assert_allclose(rs, py, rtol=1e-13)
    q = list(np.linspace(0.11, 0.87, 41))
    for kind in ["linear", "nearest", "previous", "next", "zero", "slinear", "quadratic", "cubic"]:
        r = rust("isochrone.interpolate", artifact_id=iso_id, y_column="LSST_g", x_values=q, kind=kind)
        np.testing.assert_allclose(r["values"], iso.interpolate("LSST_g", q, kind=kind), rtol=1e-9, atol=1e-9, err_msg=kind)


@pytest.mark.parametrize("kind", ["gaussian", "moffat"])
@pytest.mark.parametrize("mode", ["center", "linear_interp", "oversample", "integrate"])
def test_psf_modes(rust, ap, tmp_path, kind, mode):
    from astropy.io import fits
    shape, fwhm, ps = (9 if mode == "integrate" else 21), 0.7, 0.2
    r = rust("psf.build", kind=kind, fwhm={"value": fwhm, "unit": "arcsec"}, pixel_scale_arcsec=ps, width=shape, height=shape, mode=mode)
    out = tmp_path / "psf.fits"
    rust("artifact.export", artifact_id=r["psf_artifact_id"], destination=str(out), format="fits")
    fn = ap.gaussian_psf if kind == "gaussian" else ap.moffat_psf
    ref = fn(fwhm, pixel_scale=ps, shape=shape, mode=mode)
    # Astropy integrates with dblquad (~1.5e-8 absolute); other modes are exact formulas.
    np.testing.assert_allclose(fits.getdata(out), ref, rtol=0, atol=1e-7 if mode == "integrate" else 1e-15)


@pytest.mark.parametrize("boundary", ["wrap", "fill"])
def test_convolution_matches_astropy(rust, ap, tmp_path, boundary):
    from astropy.convolution import convolve_fft
    from astropy.io import fits
    rng = np.random.RandomState(4)
    img = rng.exponential(5.0, size=(37, 54))
    img[3, 4] = 400.0
    src = tmp_path / "img.fits"
    fits.writeto(src, img)
    image = rust("artifact.import", format="fits", path=str(src))["image_artifact_id"]
    psf = rust("psf.build", kind="moffat", fwhm={"value": 3.1, "unit": "pixel"}, width=15, height=11)["psf_artifact_id"]
    for method in ["direct", "fft"]:
        out = tmp_path / f"c-{method}.fits"
        cid = rust("image.convolve", image={"artifact_id": image}, psf={"artifact_id": psf}, boundary=boundary, method=method)["image_artifact_id"]
        rust("artifact.export", artifact_id=cid, destination=str(out), format="fits")
        k = ap.moffat_psf(3.1, pixel_scale=1.0, shape=(11, 15))
        ref = convolve_fft(img, k, boundary=boundary, normalize_kernel=True)
        np.testing.assert_allclose(fits.getdata(out), ref, rtol=1e-11, atol=1e-11)


def test_smooth_amplitudes_and_detector_conversions(rust, ap):
    from astropy import units as u
    # Sersic: ArtPop SersicSP.mag_to_image_amplitude with r_sky in arcsec.
    from scipy.special import gammaincinv, gamma
    for n, ellip, r_sky in [(0.8, 0.3, 4.2), (4.0, 0.1, 1.3), (1.0, 0.0, 10.0)]:
        r = rust("source.amplitude", profile={"kind": "sersic", "r_eff_arcsec": r_sky, "n": n, "ellip": ellip, "theta_rad": 0.4}, total_mag=18.5, zpt=27.0, pixel_scale_arcsec=0.2)
        b_n = gammaincinv(2.0 * n, 0.5)
        f_n = gamma(2 * n) * n * np.exp(b_n) / b_n ** (2 * n)
        area = np.pi * (r_sky * np.sqrt(1 - ellip)) ** 2
        mu_e = 18.5 + 2.5 * np.log10(2 * area) + 2.5 * np.log10(f_n)
        np.testing.assert_allclose([r["surface_brightness_mag_arcsec2"], r["amplitude_flux_per_pixel"]], [mu_e, 10 ** (0.4 * (27 - mu_e)) * 0.04], rtol=1e-12)
    r = rust("source.amplitude", profile={"kind": "plummer", "scale_radius_arcsec": 2.5}, total_mag=20.0, zpt=27.0, pixel_scale_arcsec=0.2)
    flux, area = 10 ** (0.4 * 7.0), np.pi * 2.5 ** 2
    np.testing.assert_allclose([r["surface_brightness_mag_arcsec2"], r["amplitude_flux_per_pixel"]], [27 - 2.5 * np.log10(flux / area), flux / area * 0.04], rtol=1e-12)
    # Physical LSST instrument: counts and calibration match ArtImager.
    imager = ap.ArtImager("LSST", diameter=6.4, efficiency=0.8)
    inst = {"kind": "physical", "photometric_systems": ["LSST"], "diameter": {"value": 6.4, "unit": "m"}, "efficiency": 0.8}
    for band in ["LSST_g", "LSST_i"]:
        mags = [18.0, 22.5, 27.0]
        c = rust("imager.convert", instrument=inst, mode="mag_counts", band=band, values=mags, exposure_s=300)
        np.testing.assert_allclose(c["result"]["values"], imager.mag_to_counts(np.array(mags), band, 300 * u.s), rtol=1e-12)
        cal = rust("imager.convert", instrument=inst, mode="calibration", band=band, exposure_s=300, zpt=27.0)
        np.testing.assert_allclose(cal["result"]["calibration"], imager.calibration(band, 300 * u.s, 27.0), rtol=1e-12)
        for ps in [0.2, 1.0]:
            sky = rust("imager.convert", instrument=inst, mode="sky_counts", band=band, values=[21.0], exposure_s=300, pixel_scale_arcsec=ps)
            np.testing.assert_allclose(sky["result"]["values"][0], imager.sb_to_counts_per_pixel(21.0, band, 300 * u.s, ps * u.arcsec / u.pixel), rtol=1e-12)
    # Instrumental zero point: identical at 1"/pix; rust-v1 multiplies by pixel area (documented fix).
    zi = ap.ArtImager(zpt_inst={"i": 25.0})
    zinst = {"kind": "instrumental", "zero_points": {"i": 25.0}}
    s1 = rust("imager.convert", instrument=zinst, mode="sky_counts", band="i", values=[21.0], exposure_s=100, pixel_scale_arcsec=1.0)["result"]["values"][0]
    s2 = rust("imager.convert", instrument=zinst, mode="sky_counts", band="i", values=[21.0], exposure_s=100, pixel_scale_arcsec=0.2)["result"]["values"][0]
    up = zi.sb_to_counts_per_pixel(21.0, "i", 100 * u.s, 0.2 * u.arcsec / u.pixel)
    np.testing.assert_allclose(s1, up, rtol=1e-13)
    np.testing.assert_allclose(s2, up * 0.04, rtol=1e-13)


def test_filters_zero_points_and_curves(rust, ap):
    from artpop.filters import load_zero_point_converter
    zp = load_zero_point_converter()
    listing = rust("filters.list")
    names = [f["name"] for s in listing["systems"] for f in s["filters"]]
    assert len(names) == 164
    props = ap.get_filter_properties()
    for s in listing["systems"]:
        for f in s["filters"]:
            row = props[props["bandpass"] == f["name"]]
            assert f["dlam_angstrom"] == float(row["dlam"][0]) and f["lam_eff_angstrom"] == float(row["lam_eff"][0])
    checked = 0
    for name in names:
        if not hasattr(zp, name):
            continue
        native = getattr(zp, name)[0]
        for target, method in [("AB", zp.to_ab), ("Vega", zp.to_vega), ("ST", zp.to_st)]:
            r = rust("filters.convert", filter=name, **{"from": native, "to": target}, values=[0.0])
            np.testing.assert_allclose(r["offset_mag"], method(name), atol=1e-12)
            checked += 1
    assert checked >= 3 * 150
    curves = [ROOT / "ArtPop" / "filter_curves" / "LSST" / "LSST_i.csv", ROOT / "ArtPop" / "filter_curves" / "HSC" / "HSC_r.csv"]
    curves = [c for c in curves if c.exists()]
    fs = ap.FilterSystem([str(c) for c in curves], [c.stem for c in curves], delimiter=",", skiprows=1)
    r = rust("filters.import", curves=[{"name": c.stem, "path": str(c)} for c in curves])
    for c, f in zip(curves, r["filters"]):
        np.testing.assert_allclose([f["lam_eff_angstrom"], f["lam_pivot_angstrom"], f["dlam_angstrom"], f["effective_throughput"]],
                                   [fs.lam_eff(c.stem).value, fs.lam_pivot(c.stem).value, fs.dlam(c.stem).value, fs.effective_throughput(c.stem)], rtol=1e-12)


def test_sample_fraction_and_surface_density(rust, ap, iso, iso_id):
    for limit, d_mpc in [(26.0, 2.0), (29.0, 5.0), (31.0, 0.5)]:
        ssp = ap.SSP(iso, num_stars=10, distance=d_mpc, mag_limit=limit, mag_limit_band="LSST_i", random_state=1)
        m_lim, fn, fm = ssp.sample_fraction(limit, "LSST_i")
        r = rust("population.sample-fraction", isochrone={"artifact_id": iso_id}, imf={"kind": "kroupa"}, mag_limit={"magnitude": limit, "band": "LSST_i", "frame": "intrinsic_apparent"}, distance={"value": d_mpc, "unit": "Mpc"})["sample_fraction"]
        np.testing.assert_allclose([r["mass_limit"], r["frac_num_sampled"], r["frac_mass_sampled"]], [m_lim, fn, fm], rtol=1e-12)
    from artpop.stars import constant_sb_stars_per_pix
    r = rust("population.surface-density", surface_brightness=26.0, mean_mag=5.2, distance={"value": 3, "unit": "Mpc"}, pixel_scale_arcsec=0.17)
    np.testing.assert_allclose(r["stars_per_pixel"], constant_sb_stars_per_pix(26.0, 5.2, 3.0, 0.17), rtol=1e-13)


def test_mist_selection_matches_upstream(rust, ap):
    mist = Path(os.environ.get("MIST_PATH", "/nonexistent")) / "MIST_v1.2_vvcrit0.4_LSST"
    if not mist.is_dir() or not os.environ.get("ARTPOP_PARITY_DATA_DIR"):
        pytest.skip("Downloaded MIST LSST grid and ARTPOP_PARITY_DATA_DIR are required")
    rust("data.import", path=str(mist))
    for age, feh in [(10.0, -1.0), (9.35, -1.1), (8.0, 0.3), (7.12, -2.7)]:
        r = rust("isochrone.mist", log_age_yr=age, feh=feh, photometric_systems=["LSST"])
        rows, cursor = [], None
        while True:
            page = rust("isochrone.inspect", artifact_id=r["isochrone_artifact_id"], columns=["initial_mass", "current_mass", "eep", "LSST_g", "LSST_i"], page={"limit": 1000, **({"cursor": cursor} if cursor else {})})["page"]
            rows += page["rows"]
            cursor = page["next_cursor"]
            if cursor is None:
                break
        py = ap.MISTIsochrone(age, feh, "LSST", mist_path=os.environ["MIST_PATH"])
        rows = np.array(rows, dtype=float)
        assert len(rows) == len(py.mini)
        for k, col in enumerate([py.mini, py.mact, py.eep, py.mag_table["LSST_g"], py.mag_table["LSST_i"]]):
            np.testing.assert_array_equal(rows[:, k], np.asarray(col, dtype=float))


def _export_csv(rust, artifact, path, **kw):
    rust("artifact.export", artifact_id=artifact, destination=str(path), format="csv", **kw)
    return np.genfromtxt(path, delimiter=",", names=True)


def test_spatial_samplers_match_upstream_distributions(rust, ap, tmp_path):
    from scipy.stats import ks_2samp
    from astropy import units as u
    n, dim, ps, d = 40000, 301, 0.2, 2.0
    geom = {"width": dim, "height": dim, "pixel_scale_arcsec": ps, "coordinates": "artpop-0.1.2"}
    cases = {
        "sersic": ({"kind": "sersic", "effective_radius": {"value": 40, "unit": "pc"}, "n": 1.3, "ellipticity": 0.35, "theta": {"value": 30, "unit": "deg"}, "num_effective_radii": 4},
                   lambda rs: ap.sersic_xy(n, d, dim, ps, r_eff=0.04, n=1.3, ellip=0.35, theta=30, num_r_eff=4, random_state=rs)),
        "plummer": ({"kind": "plummer", "scale_radius": {"value": 15, "unit": "pc"}},
                    lambda rs: ap.plummer_xy(n, d, dim, ps, scale_radius=0.015, random_state=rs)),
    }
    for name, (spec, py) in cases.items():
        r = rust("spatial.sample", distribution=spec, distance={"value": d, "unit": "Mpc"}, count=n, geometry=geom, outside="retain_marked", execution={"seed": "0000000000000a11"})
        xy = _export_csv(rust, r["positions_artifact_id"], tmp_path / f"{name}.csv")
        ref = np.ma.getdata(py(7))
        c = (dim // 2) if name == "sersic" else dim / 2
        for axis, label in [(0, "x"), (1, "y")]:
            p = ks_2samp(xy[label] - c, ref[:, axis] - c).pvalue
            assert p > 1e-6, (name, label, p)
        rr = np.hypot(xy["x"] - c, xy["y"] - c)
        assert ks_2samp(rr, np.hypot(ref[:, 0] - c, ref[:, 1] - c)).pvalue > 1e-6, name


def test_magnitude_limited_population_photometry(rust, ap, iso, iso_id):
    """Total and SBF magnitudes of cutoff populations agree statistically; the
    rust-v1 SBF second moment also attenuates with extinction (documented fix)."""
    n, d, limit = 2_000_000, 4.0, 30.0
    py_tot, py_sbf = [], []
    for s in range(4):
        ssp = ap.SSP(iso, num_stars=n, distance=d, mag_limit=limit, mag_limit_band="LSST_i", random_state=100 + s)
        py_tot.append(ssp.total_mag("LSST_i"))
        py_sbf.append(ssp.sbf_mag("LSST_i"))
    rs_tot, rs_sbf = [], []
    for s in range(4):
        pop = rust("population.build", isochrone={"artifact_id": iso_id}, normalization={"kind": "count", "value": n}, distance={"value": d, "unit": "Mpc"},
                   mag_limit={"magnitude": limit, "band": "LSST_i", "frame": "intrinsic_apparent"}, execution={"seed": f"{0x500 + s:016x}"})
        assert pop["components"][0]["regime"] == "partial"
        ph = rust("population.photometry", artifact_id=pop["population_artifact_id"], bands=["LSST_i"])
        rs_tot.append(ph["total_mag"]["LSST_i"])
        rs_sbf.append(ph["sbf_mag"]["LSST_i"])
    # Allow 5 standard errors of the pooled scatter (plus a 1 mmag floor).
    for a, b in [(py_tot, rs_tot), (py_sbf, rs_sbf)]:
        tol = 5 * np.sqrt(np.var(a, ddof=1) / len(a) + np.var(b, ddof=1) / len(b)) + 1e-3
        assert abs(np.mean(a) - np.mean(b)) < tol, (np.mean(a), np.mean(b), tol)
