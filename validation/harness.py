"""Executable reference E2E workflows and future Rust CLI differential adapter.

This is validation tooling, not a Rust implementation. Rust requests below are
contract fixtures for the proposed CLI; no Rust parity is claimed until they run.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
CASES = ("histogram", "ideal", "detector", "population", "grid")


def dump(path, value):
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + "\n")


def reference_imports():
    os.environ.setdefault("MIST_PATH", "/tmp/artpop-analysis-mist")
    os.environ.setdefault("MPLCONFIGDIR", "/tmp/artpop-analysis-mpl")
    Path(os.environ["MIST_PATH"]).mkdir(parents=True, exist_ok=True)
    sys.path.insert(0, str(ROOT / "ArtPop/src"))
    import artpop
    import numpy as np
    from astropy.io import fits
    return artpop, np, fits


def prepare(directory, count=100000):
    """Generate shared inputs, outside the timed execution region."""
    import pickle
    artpop, np, _ = reference_imports()
    directory = Path(directory).resolve()
    directory.mkdir(parents=True, exist_ok=True)
    rng = np.random.RandomState(8128)
    catalog = np.column_stack((rng.uniform(0.01, 199.99, count),
                               rng.uniform(0.01, 99.99, count),
                               rng.uniform(22, 28, count)))
    np.savetxt(directory / "catalog.csv", catalog, delimiter=",", header="x,y,LSST_i", comments="", fmt="%.17g")
    # Trusted pinned repository fixture; never an arbitrary uploaded pickle.
    with open(Path(artpop.data_dir) / "feh_m1.00_vvcrit0.4_LSST_10gyr_test_iso.pkl", "rb") as f:
        iso = pickle.load(f)
    cols = ["initial_mass", "star_mass", "LSST_g", "LSST_i"]
    np.savetxt(directory / "isochrone.csv", np.column_stack([iso[c] for c in cols]), delimiter=",", header=",".join(cols), comments="", fmt="%.17g")
    weights = np.array([[1 + x + 3*y for x in range(7)] for y in range(7)], dtype=float)
    weights[0, 0] = 0
    dump(directory / "input.json", {"count": count, "width": 201, "height": 101, "grid_weights_yx": weights.tolist(), "seed": "000000000000007b"})
    files = [directory / x for x in ["catalog.csv", "isochrone.csv", "input.json"]]
    dump(directory / "manifest.json", {"upstream_commit": "38a886906ed175d0205bf48b6dc03f54aeb4a81a", "files": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in files}})
    return directory


def run_python(case, inputs, out):
    artpop, np, fits = reference_imports()
    inputs, out = Path(inputs), Path(out)
    out.mkdir(parents=True, exist_ok=True)
    params = json.loads((inputs / "input.json").read_text())
    if case in ("histogram", "ideal", "detector"):
        a = np.loadtxt(inputs / "catalog.csv", delimiter=",", skiprows=1, ndmin=2)
        src = artpop.Source(a[:, :2], {"LSST_i": a[:, 2]}, [201, 101], pixel_scale=1.0)
    if case == "histogram":
        value = artpop.IdealImager().inject_stars(src.x, src.y, np.ones(len(a)), src.xy_dim)
        fits.writeto(out / "image.fits", value, overwrite=True)
    elif case == "ideal":
        psf = artpop.moffat_psf(0.6, pixel_scale=1, shape=21, mode="center")
        obs = artpop.IdealImager().observe(src, "LSST_i", psf=psf, zpt=27, boundary="wrap")
        obs.to_fits(out / "image.fits")
    elif case == "detector":
        imager = artpop.ArtImager(zpt_inst={"LSST_i": 25}, read_noise=4, random_state=123)
        obs = imager.observe(src, "LSST_i", exptime=100, sky_sb=21, zpt=27)
        for name, data in [("source_counts", obs.src_counts), ("raw_counts", obs.raw_counts), ("image", obs.image)]:
            fits.writeto(out / (name + ".fits"), data, overwrite=True)
        dump(out / "summary.json", {"sky_counts": float(obs.sky_counts), "calibration": float(obs.calibration), "read_noise": 4.0})
    elif case == "population":
        a = np.genfromtxt(inputs / "isochrone.csv", delimiter=",", names=True)
        iso = artpop.Isochrone(a["initial_mass"], a["star_mass"], {b: a[b] for b in ["LSST_g", "LSST_i"]})
        pop = artpop.SSP(iso, num_stars=params["count"], random_state=123)
        np.savetxt(out / "catalog.csv", np.column_stack([pop.initial_masses, pop.star_masses, pop.star_mags("LSST_g"), pop.star_mags("LSST_i")]), delimiter=",", header="initial_mass,star_mass,LSST_g,LSST_i", comments="", fmt="%.17g")
        dump(out / "summary.json", {"total_mass_msun": float(pop.total_mass.value), "total_mag": {b: float(pop.total_mag(b)) for b in ["LSST_g", "LSST_i"]}, "sbf_mag": {b: float(pop.sbf_mag(b)) for b in ["LSST_g", "LSST_i"]}})
    elif case == "grid":
        weights = np.array(params["grid_weights_yx"])
        xy = artpop.xy_from_grid(params["count"], lambda x,y: weights[y,x], 7, random_state=123)
        np.savetxt(out / "positions.csv", xy.data, delimiter=",", header="x,y", comments="", fmt="%.17g")
    else:
        raise ValueError(case)


class RustCli:
    """Strict adapter to the request fixtures documented in validation/README.md."""
    def __init__(self, binary, directory):
        self.binary = str(Path(binary).resolve())
        self.directory = Path(directory).resolve()
        self.directory.mkdir(parents=True, exist_ok=True)
        self.index = 0
        self.extra_args = []

    def invoke(self, operation, **params):
        self.index += 1
        request = self.directory / f"{self.index:02}-{operation}.request.json"
        dump(request, {"schema_version": "1", **params})
        response = subprocess.run([self.binary, *self.extra_args, "invoke", operation, "--input", str(request), "--output-dir", str(self.directory / "artifacts")], text=True, capture_output=True, timeout=300)
        (self.directory / f"{self.index:02}-{operation}.stdout").write_text(response.stdout)
        (self.directory / f"{self.index:02}-{operation}.stderr").write_text(response.stderr)
        if response.returncode:
            raise RuntimeError(f"{operation}: exit {response.returncode}: {response.stdout[:1000]} {response.stderr[:1000]}")
        result = json.loads(response.stdout)
        if result.get("status") != "completed":
            raise AssertionError(f"{operation} did not complete: {result}")
        return result

    @staticmethod
    def ref(result, kind):
        matches = [a for a in result["artifacts"] if a["kind"] == kind]
        if len(matches) != 1:
            raise AssertionError(f"Expected one {kind} artifact, got {matches}")
        return {"artifact_id": matches[0]["artifact_id"]}

    def export(self, ref, destination, fmt, **options):
        self.invoke("artifact.export", **ref, destination=str(Path(destination).resolve()), format=fmt, **options)
        if not Path(destination).is_file():
            raise AssertionError(f"Export did not write {destination}")


def run_rust(case, inputs, out, binary):
    inputs, out = Path(inputs).resolve(), Path(out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    p = json.loads((inputs / "input.json").read_text())
    cli = RustCli(binary, out / "calls")
    execution = {"algorithm_profile": "rust-v1", "seed": p["seed"]}
    table = {"format": "csv", "path": str(inputs / "catalog.csv"), "x_column": "x", "y_column": "y", "magnitude_columns": {"LSST_i": "LSST_i"}, "magnitude_system": "AB"}
    geometry = {"width": 201, "height": 101, "pixel_scale_arcsec": 1.0, "coordinates": "artpop-0.1.2"}
    if case == "histogram":
        result = cli.invoke("image.histogram", catalog=table, geometry=geometry, weights={"kind":"unit"}, convention="artpop-0.1.2")
        cli.export(cli.ref(result,"image"), out / "image.fits", "fits")
    elif case in ("ideal", "detector"):
        source = cli.ref(cli.invoke("source.build", catalog=table, geometry=geometry), "source")
        if case == "ideal":
            psf = cli.ref(cli.invoke("psf.build", kind="moffat", fwhm={"value":0.6,"unit":"arcsec"}, pixel_scale_arcsec=1.0, width=21, height=21, alpha=4.765, mode="center"), "psf")
            result = cli.invoke("observe.ideal", source=source, bandpass="LSST_i", psf=psf, output_zpt=27, boundary="wrap", histogram_convention="artpop-0.1.2")
        else:
            result = cli.invoke("observe.artificial", source=source, instrument={"kind":"instrumental","zero_points":{"LSST_i":25},"magnitude_system":"AB","read_noise_electrons":4}, bandpass="LSST_i", exposure_s=100, sky_sb_mag_arcsec2=21, output_zpt=27, histogram_convention="artpop-0.1.2", execution=execution)
        obs = cli.ref(result,"observation")
        cli.export(obs, out / "image.fits", "fits", plane="image")
        if case == "detector":
            for plane in ["source_counts", "raw_counts"]:
                cli.export(obs, out / f"{plane}.fits", "fits", plane=plane)
            summary = cli.invoke("observation.inspect", **obs, fields=["sky_counts","calibration","read_noise"])["result"]
            dump(out / "summary.json", summary)
    elif case == "population":
        iso = cli.ref(cli.invoke("isochrone.import", format="csv", path=str(inputs / "isochrone.csv"), initial_mass_column="initial_mass", current_mass_column="star_mass", magnitude_columns={"LSST_g":"LSST_g","LSST_i":"LSST_i"}, magnitude_system="AB"), "isochrone")
        pop = cli.ref(cli.invoke("population.build", isochrone=iso, normalization={"kind":"count","value":p["count"]}, imf={"kind":"kroupa"}, distance={"value":10,"unit":"pc"}, extinction_mag=0, add_remnants=True, execution=execution), "population")
        cli.export(pop, out / "catalog.csv", "csv", columns=["initial_mass","star_mass","LSST_g","LSST_i"], magnitude_frame="apparent")
        phot = cli.invoke("population.photometry", **pop, bands=["LSST_g","LSST_i"], metrics=["total_mag","sbf_mag"])["result"]
        mass = cli.invoke("population.inspect", **pop, fields=["total_mass_msun"])["result"]
        dump(out / "summary.json", {**phot, **mass})
    elif case == "grid":
        grid = cli.ref(cli.invoke("spatial.grid-build", weights_yx=p["grid_weights_yx"], width=7, height=7, sampler="cdf"), "spatial_grid")
        positions = cli.ref(cli.invoke("spatial.sample", grid=grid, count=p["count"], geometry={"width":7,"height":7,"coordinates":"pixel-centers-v1"}, offset_pixels=[0,0], outside="error", execution=execution), "positions")
        cli.export(positions, out / "positions.csv", "csv", columns=["x","y"])
    else:
        raise ValueError(case)


def check(case, inputs, output, reference=None):
    """Independent invariants plus differential checks, without same-seed equality."""
    artpop, np, fits = reference_imports()
    from scipy.stats import chisquare, ks_2samp
    inputs, output = Path(inputs), Path(output)
    p = json.loads((inputs / "input.json").read_text())
    metrics = {}
    if case in ("histogram", "ideal", "detector"):
        image = fits.getdata(output / "image.fits")
        assert image.shape == (101,201)
        assert np.isfinite(image).all()
    if case == "histogram":
        assert image.sum() == p["count"]
        assert (image >= 0).all()
        if reference:
            np.testing.assert_array_equal(image, fits.getdata(Path(reference) / "image.fits"))
    elif case == "ideal":
        a = np.loadtxt(inputs / "catalog.csv", delimiter=",", skiprows=1, ndmin=2)
        expected_flux = np.sum(10**(0.4*(27-a[:,2])))
        np.testing.assert_allclose(image.sum(), expected_flux, rtol=1e-10)
        if reference:
            base = fits.getdata(Path(reference) / "image.fits")
            relative_l1 = float(np.abs(image-base).sum()/np.abs(base).sum())
            metrics["relative_l1_error"] = relative_l1
            assert relative_l1 <= 1e-9
            np.testing.assert_allclose(image,base,rtol=1e-9,atol=1e-10)
    elif case == "detector":
        s = json.loads((output / "summary.json").read_text())
        source = fits.getdata(output / "source_counts.fits")
        raw = fits.getdata(output / "raw_counts.fits")
        expected_sky = 100*10**(0.4*(25-21))
        expected_cal = 10**(0.4*(27-25))/100
        np.testing.assert_allclose([s["sky_counts"],s["calibration"]],[expected_sky,expected_cal],rtol=1e-12)
        np.testing.assert_allclose(image,(raw-s["sky_counts"])*s["calibration"],rtol=1e-12,atol=1e-12)
        variance = source+s["sky_counts"]+s["read_noise"]**2
        z = (raw-source-s["sky_counts"])/np.sqrt(variance)
        metrics.update(standardized_mean=float(z.mean()), standardized_second_moment=float(np.mean(z*z)))
        # Conservative bounds over >20k independent pixels: no identical RNG required.
        assert abs(z.mean()) < 0.08
        assert abs(np.mean(z*z)-1) < 0.08
        if reference:
            np.testing.assert_allclose(source,fits.getdata(Path(reference)/"source_counts.fits"),rtol=1e-10,atol=1e-10)
    elif case == "population":
        a = np.genfromtxt(inputs/"isochrone.csv", delimiter=",", names=True)
        stars = np.genfromtxt(output/"catalog.csv", delimiter=",", names=True)
        assert len(stars) == p["count"]
        iso = artpop.Isochrone(a["initial_mass"],a["star_mass"],{b:a[b] for b in ["LSST_g","LSST_i"]})
        assert np.isfinite(np.column_stack([stars[n] for n in stars.dtype.names])).all()
        assert stars["initial_mass"].min() >= iso.m_min
        assert stars["initial_mass"].max() <= iso.m_max
        np.testing.assert_allclose(stars["star_mass"],iso.interpolate("mact",stars["initial_mass"]),rtol=1e-10,atol=1e-12)
        summary=json.loads((output/"summary.json").read_text())
        rem_factor=iso.ssp_surviving_mass("kroupa",add_remnants=False)/iso.ssp_surviving_mass("kroupa",add_remnants=True)
        np.testing.assert_allclose(summary["total_mass_msun"],stars["star_mass"].sum()/rem_factor,rtol=1e-10)
        for band in ["LSST_g","LSST_i"]:
            np.testing.assert_allclose(stars[band],iso.interpolate(band,stars["initial_mass"]),rtol=1e-10,atol=1e-10)
            flux=10**(-0.4*stars[band])
            np.testing.assert_allclose(summary["total_mag"][band],-2.5*np.log10(flux.sum()),atol=1e-8,rtol=0)
            np.testing.assert_allclose(summary["sbf_mag"][band],-2.5*np.log10((flux*flux).sum()/flux.sum()),atol=1e-8,rtol=0)
        if reference:
            other=np.genfromtxt(Path(reference)/"catalog.csv",delimiter=",",names=True)
            ks=ks_2samp(stars["initial_mass"],other["initial_mass"])
            metrics["mass_ks_pvalue"]=float(ks.pvalue)
            assert ks.pvalue > 1e-8
    elif case == "grid":
        xy=np.loadtxt(output/"positions.csv",delimiter=",",skiprows=1,ndmin=2)
        assert xy.shape==(p["count"],2)
        assert np.isfinite(xy).all() and (xy>=0).all() and (xy<7).all()
        np.testing.assert_array_equal(xy,np.floor(xy))
        counts=np.zeros((7,7))
        for x,y in xy.astype(int): counts[y,x]+=1
        w=np.array(p["grid_weights_yx"])
        assert (counts[w==0]==0).all()
        expected=p["count"]*w/w.sum()
        assert expected[w>0].min()>=5, "Use at least 10,000 samples for the grid statistical test"
        c=chisquare(counts[w>0],expected[w>0])
        metrics["grid_chisquare_pvalue"]=float(c.pvalue)
        assert c.pvalue>1e-8
    return metrics


def main():
    p=argparse.ArgumentParser()
    sub=p.add_subparsers(dest="command",required=True)
    prep=sub.add_parser("prepare")
    prep.add_argument("directory",type=Path)
    prep.add_argument("--count",type=int,default=100000)
    run=sub.add_parser("run")
    run.add_argument("--backend",choices=["python","rust"],required=True)
    run.add_argument("--case",choices=CASES,required=True)
    run.add_argument("--inputs",type=Path,required=True)
    run.add_argument("--output",type=Path,required=True)
    run.add_argument("--rust-bin",type=Path)
    chk=sub.add_parser("check")
    chk.add_argument("--case",choices=CASES,required=True)
    chk.add_argument("--inputs",type=Path,required=True)
    chk.add_argument("--output",type=Path,required=True)
    chk.add_argument("--reference",type=Path)
    args=p.parse_args()
    if args.command=="prepare":
        if args.count<10000: p.error("count must be at least 10000 for statistical fixtures")
        print(prepare(args.directory,args.count))
    elif args.command=="run":
        if args.backend=="rust":
            if not args.rust_bin or not args.rust_bin.is_file(): p.error("--rust-bin must point to an existing implementation")
            run_rust(args.case,args.inputs,args.output,args.rust_bin)
        else: run_python(args.case,args.inputs,args.output)
        print(json.dumps({"status":"completed","backend":args.backend,"case":args.case}))
    else:
        print(json.dumps(check(args.case,args.inputs,args.output,args.reference),allow_nan=False))


if __name__=="__main__": main()
