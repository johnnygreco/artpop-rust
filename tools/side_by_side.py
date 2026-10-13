"""Side-by-side galaxy images from ArtPop 0.1.2 and artpop-rust.

The two backends draw independent random realizations (rust-v1 has its own
streams), so the images are statistically equivalent rather than pixel-
identical; every panel pair uses one display stretch computed from the
ArtPop image and applied to both. Produces, in --out-dir:

  side_by_side_rgb.png            new three-band (g, r, i) Sersic galaxy with detector
                                  noise, composed with identical Lupton asinh parameters
  side_by_side_sersic_ideal.png   801^2 noise-free Sersic image from the scale benchmark,
                                  plus azimuthally averaged surface-brightness profiles
  side_by_side_plummer.png        2047^2 detector image of a Plummer cluster from the
                                  scale benchmark (central crop), plus radial profiles

Usage:
  python tools/side_by_side.py --rust-bin target/release/artpop-rust --out-dir docs/images \
      [--scale-dir /tmp/arp-scale-v3] [--work-dir /tmp/arp-side-by-side]
"""
import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = ROOT / "fixtures" / "lsst_feh_m1.00_10gyr_iso.csv"
BANDS = ["LSST_g", "LSST_r", "LSST_i"]
SKY = {"LSST_g": 22.3, "LSST_r": 21.2, "LSST_i": 20.5}
EXPTIME = 900.0
DIM = 801
PIXEL_SCALE = 0.2
DISTANCE_MPC = 3.0
R_EFF_PC = 300.0
N_STARS = 4_000_000


def reference_imports():
    os.environ.setdefault("MIST_PATH", "/tmp/artpop-analysis-mist")
    os.environ.setdefault("MPLCONFIGDIR", "/tmp/artpop-analysis-mpl")
    sys.path.insert(0, str(ROOT.parent / "ArtPop" / "src"))
    import artpop
    import numpy as np
    return artpop, np


def python_rgb_images(np, artpop):
    a = np.genfromtxt(FIXTURE, delimiter=",", names=True)
    iso = artpop.Isochrone(a["initial_mass"], a["star_mass"], {b: a[b] for b in BANDS})
    ssp = artpop.SSP(iso, num_stars=N_STARS, distance=DISTANCE_MPC, random_state=11)
    src = artpop.SersicSP(ssp, r_eff=R_EFF_PC / 1000.0, n=1.0, theta=40, ellip=0.4, xy_dim=DIM, pixel_scale=PIXEL_SCALE)
    psf = artpop.moffat_psf(0.7, pixel_scale=PIXEL_SCALE, shape=41)
    imager = artpop.ArtImager("LSST", diameter=6.4, read_noise=4, random_state=12)
    return {b: imager.observe(src, b, exptime=EXPTIME, sky_sb=SKY[b], psf=psf, zpt=27).image for b in BANDS}


class Rust:
    def __init__(self, binary, work):
        self.binary, self.work, self.n = str(binary), Path(work), 0
        self.work.mkdir(parents=True, exist_ok=True)

    def invoke(self, op, **req):
        self.n += 1
        p = self.work / f"{self.n:02}-{op}.json"
        p.write_text(json.dumps({"schema_version": "1", **req}))
        r = subprocess.run([self.binary, "invoke", op, "--input", str(p), "--output-dir", str(self.work / "artifacts")], capture_output=True, text=True, check=True)
        env = json.loads(r.stdout)
        if env["status"] != "completed":
            raise RuntimeError(env)
        return env

    def artifact(self, env, kind):
        return [a["artifact_id"] for a in env["artifacts"] if a["kind"] == kind][0]


def rust_rgb_images(np, binary, work):
    from astropy.io import fits
    cli = Rust(binary, work)
    iso = cli.artifact(cli.invoke("isochrone.import", format="csv", path=str(FIXTURE), magnitude_columns={b: b for b in BANDS}, magnitude_system="AB"), "isochrone")
    pop = cli.artifact(cli.invoke("population.build", isochrone={"artifact_id": iso}, normalization={"kind": "count", "value": N_STARS}, distance={"value": DISTANCE_MPC, "unit": "Mpc"}, execution={"seed": "0000000000000b0b"}), "population")
    src = cli.artifact(cli.invoke("source.build", population={"artifact_id": pop}, spatial_model={"kind": "sersic", "effective_radius": {"value": R_EFF_PC, "unit": "pc"}, "n": 1.0, "ellipticity": 0.4, "theta": {"value": 40, "unit": "deg"}}, geometry={"width": DIM, "height": DIM, "pixel_scale_arcsec": PIXEL_SCALE}, execution={"seed": "0000000000000b0c"}), "source")
    psf = cli.artifact(cli.invoke("psf.build", kind="moffat", fwhm={"value": 0.7, "unit": "arcsec"}, pixel_scale_arcsec=PIXEL_SCALE, width=41, height=41), "psf")
    images = {}
    for k, b in enumerate(BANDS):
        obs = cli.artifact(cli.invoke("observe.artificial", source={"artifact_id": src}, instrument={"kind": "physical", "photometric_systems": ["LSST"], "diameter": {"value": 6.4, "unit": "m"}, "read_noise_electrons": 4}, bandpass=b, exposure_s=EXPTIME, sky_sb_mag_arcsec2=SKY[b], psf={"artifact_id": psf}, output_zpt=27, execution={"seed": f"0000000000000c{k:02x}"}), "observation")
        dest = cli.work / f"rust_{b}.fits"
        cli.invoke("artifact.export", artifact_id=obs, destination=str(dest), format="fits", plane="image", overwrite=True)
        images[b] = fits.getdata(dest).astype(float)
    return images


def radial_profile(np, img, zpt=27.0, pixel_scale=PIXEL_SCALE, nbins=60, rmax_pix=None):
    """Azimuthally averaged surface brightness (mag/arcsec^2) in circular annuli about the image center."""
    h, w = img.shape
    yy, xx = np.indices(img.shape)
    r = np.hypot(xx - (w - 1) / 2.0, yy - (h - 1) / 2.0)
    rmax = rmax_pix or min(w, h) / 2.0
    edges = np.linspace(0, rmax, nbins + 1)
    idx = np.digitize(r.ravel(), edges) - 1
    flat = img.ravel()
    valid = (idx >= 0) & (idx < nbins)
    sums = np.bincount(idx[valid], weights=flat[valid], minlength=nbins)
    counts = np.bincount(idx[valid], minlength=nbins)
    mean = sums / np.maximum(counts, 1)
    with np.errstate(divide="ignore", invalid="ignore"):
        mu = zpt - 2.5 * np.log10(mean / pixel_scale**2)
    centers = 0.5 * (edges[1:] + edges[:-1]) * pixel_scale
    return centers, mu, mean


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rust-bin", type=Path, required=True)
    ap.add_argument("--out-dir", type=Path, required=True)
    ap.add_argument("--scale-dir", type=Path, default=Path("/tmp/arp-scale-v3"))
    ap.add_argument("--work-dir", type=Path, default=Path("/tmp/arp-side-by-side"))
    args = ap.parse_args()
    artpop, np = reference_imports()
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from astropy.io import fits
    from astropy.visualization import AsinhStretch, ImageNormalize, make_lupton_rgb
    args.out_dir.mkdir(parents=True, exist_ok=True)
    label = {"py": "ArtPop 0.1.2 (Python)", "rs": "artpop-rust"}

    # ---- 1. Three-band Sersic galaxy, detector images, Lupton RGB with identical parameters.
    py = python_rgb_images(np, artpop)
    rs = rust_rgb_images(np, args.rust_bin, args.work_dir / "rust")
    # Asinh softening from the sky noise of the ArtPop i image (robust sigma), used for both.
    sigma = 1.4826 * float(np.median(np.abs(py["LSST_i"] - np.median(py["LSST_i"]))))
    stretch = 12.0 * sigma
    fig, axes = plt.subplots(1, 2, figsize=(12, 6.6))
    for ax, (key, imgs) in zip(axes, [("py", py), ("rs", rs)]):
        rgb = make_lupton_rgb(imgs["LSST_i"], imgs["LSST_r"], imgs["LSST_g"], Q=8, stretch=stretch, minimum=0.0)
        ax.imshow(rgb, origin="lower", interpolation="nearest")
        ax.set_title(label[key])
        ax.set_xticks([])
        ax.set_yticks([])
        tot = {b: 27 - 2.5 * np.log10(imgs[b].sum()) for b in BANDS}
        ax.text(0.02, 0.02, "total g, r, i = " + ", ".join(f"{tot[b]:.2f}" for b in BANDS), color="w", transform=ax.transAxes, fontsize=9)
    fig.suptitle(f"Sérsic dwarf (n=1, r_eff={R_EFF_PC:.0f} pc, {N_STARS:,} stars) at {DISTANCE_MPC:.0f} Mpc: LSST g,r,i, {EXPTIME:.0f} s each, {DIM}² at {PIXEL_SCALE}\"/pixel.\nSame Lupton asinh stretch (Q=8, stretch = 12 σ_sky); independent random realizations.", fontsize=10)
    fig.tight_layout(rect=(0, 0, 1, 0.93))
    fig.savefig(args.out_dir / "side_by_side_rgb.png", dpi=100)
    plt.close(fig)

    # ---- 2. Sersic ideal image from the scale benchmark (801^2, 1M stars, 41^2 Moffat PSF).
    sersic = {k: fits.getdata(args.scale_dir / "sersic_ideal_1m" / b / "sample0" / "image.fits").astype(float) for k, b in [("py", "python"), ("rs", "rust")]}
    vmin, vmax = np.percentile(sersic["py"], [0.5, 99.95])
    norm = ImageNormalize(vmin=vmin, vmax=vmax, stretch=AsinhStretch(0.02))
    fig, axes = plt.subplots(1, 3, figsize=(16, 5.6), gridspec_kw={"width_ratios": [1, 1, 1.15]})
    for ax, key in zip(axes[:2], ["py", "rs"]):
        ax.imshow(sersic[key], origin="lower", cmap="gray_r", norm=norm, interpolation="nearest")
        ax.set_title(f"{label[key]}: total mag {27 - 2.5 * np.log10(sersic[key].sum()):.3f}")
        ax.set_xticks([])
        ax.set_yticks([])
    for key, color in [("py", "C0"), ("rs", "C3")]:
        rr, mu, _ = radial_profile(np, sersic[key], nbins=80)
        axes[2].plot(rr, mu, color=color, lw=1.6, label=label[key], alpha=0.85)
    axes[2].invert_yaxis()
    axes[2].set_xlabel("radius [arcsec]")
    axes[2].set_ylabel("mean surface brightness [mag/arcsec²] (zpt 27)")
    axes[2].set_title("Azimuthally averaged profiles")
    axes[2].grid(alpha=0.3)
    axes[2].legend()
    fig.suptitle("Benchmark workflow: 1M-star SSP → Sérsic source → 801² ideal image (41² Moffat PSF). Same asinh stretch; independent random realizations.", fontsize=10)
    fig.tight_layout(rect=(0, 0, 1, 0.94))
    fig.savefig(args.out_dir / "side_by_side_sersic_ideal.png", dpi=100)
    plt.close(fig)

    # ---- 3. Plummer detector image from the scale benchmark (2047^2 central crop).
    plummer = {k: fits.getdata(args.scale_dir / "artificial_2k" / b / "sample0" / "image.fits").astype(float) for k, b in [("py", "python"), ("rs", "rust")]}
    c, half = 1023, 150
    crops = {k: v[c - half:c + half + 1, c - half:c + half + 1] for k, v in plummer.items()}
    vmin, vmax = np.percentile(crops["py"], [1.0, 99.95])
    norm = ImageNormalize(vmin=vmin, vmax=vmax, stretch=AsinhStretch(0.05))
    fig, axes = plt.subplots(1, 3, figsize=(16, 5.8), gridspec_kw={"width_ratios": [1, 1, 1.15]})
    for ax, key in zip(axes[:2], ["py", "rs"]):
        ax.imshow(crops[key], origin="lower", cmap="gray_r", norm=norm, interpolation="nearest")
        ax.set_title(f"{label[key].split(' (')[0]}: central {(2 * half + 1) * PIXEL_SCALE:.0f}\" of 2047², i band")
        ax.set_xticks([])
        ax.set_yticks([])
    for key, color in [("py", "C0"), ("rs", "C3")]:
        rr, mu, mean = radial_profile(np, plummer[key], nbins=100, rmax_pix=100)
        ok = mean > 0
        axes[2].plot(rr[ok], mu[ok], color=color, lw=1.6, label=label[key], alpha=0.85)
    axes[2].invert_yaxis()
    axes[2].set_xlabel("radius [arcsec]")
    axes[2].set_ylabel("mean surface brightness [mag/arcsec²] (zpt 27)")
    axes[2].set_title("Azimuthally averaged profiles (sky-subtracted, where positive)")
    axes[2].grid(alpha=0.3)
    axes[2].legend()
    fig.suptitle("Benchmark workflow: 1M-star SSP → Plummer source → 2047² LSST detector image (900 s, sky 21 mag/arcsec², 41² PSF).\nSame asinh stretch; independent random realizations (noise and star positions differ, structure matches).", fontsize=10)
    fig.tight_layout(rect=(0, 0, 1, 0.92))
    fig.savefig(args.out_dir / "side_by_side_plummer.png", dpi=100)
    plt.close(fig)
    for f in ["side_by_side_rgb.png", "side_by_side_sersic_ideal.png", "side_by_side_plummer.png"]:
        print(f, (args.out_dir / f).stat().st_size, "bytes")


if __name__ == "__main__":
    main()
