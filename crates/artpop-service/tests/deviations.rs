//! One regression test per verified ArtPop 0.1.2 defect (VALIDATION_PLAN §4),
//! each with an independent expected answer, run through the service layer.

mod common;

use common::{Env, id, seed, synthetic_mist};
use serde_json::{Value, json};

#[test]
fn final_column_star_is_kept_by_default_and_dropped_by_legacy_bins() {
    let e = Env::new("hist-edges");
    let cat = e.write("edge.csv", "x,y,m\n4,2,20\n0,0,20\n4.49,1,20\n");
    let run = |conv: &str| -> Value {
        e.ok("image.histogram", json!({ "catalog": { "format": "csv", "path": cat, "magnitude_columns": { "b": "m" } }, "geometry": { "width": 5, "height": 5, "coordinates": conv } }))
    };
    let v1 = run("pixel-centers-v1");
    assert_eq!(v1["result"]["diagnostics"]["deposited"], 3);
    let legacy = run("artpop-0.1.2");
    // x = 4 and x = 4.49 are beyond ArtPop's [0, W-1) histogram range.
    assert_eq!(legacy["result"]["diagnostics"]["deposited"], 1);
    assert_eq!(legacy["result"]["diagnostics"]["outside"], 2);
}

#[test]
fn fractional_offsets_work_and_outside_policy_always_applies() {
    let e = Env::new("offsets");
    let g = e.ok("spatial.grid-build", json!({ "weights_yx": [[1, 1, 1, 1, 1], [1, 1, 1, 1, 1], [1, 1, 1, 1, 1], [1, 1, 1, 1, 1], [1, 1, 1, 1, 1]], "width": 5, "height": 5 }));
    let grid = id(&g, "spatial_grid_artifact_id");
    let s = e.ok("spatial.sample", json!({ "grid": { "artifact_id": grid }, "count": 2000, "geometry": { "width": 5, "height": 5 }, "offset_pixels": [0.5, 0.0], "outside": "drop", "execution": seed("0000000000000003") }));
    // Same-size grid shifted by half a pixel: the x = 4.5 column leaves the frame.
    let outside = s["result"]["outside"].as_u64().unwrap();
    assert!(outside > 250 && outside < 550, "{outside}");
    assert_eq!(s["result"]["retained"].as_u64().unwrap() + outside, 2000);
    let err = e.err("spatial.sample", json!({ "grid": { "artifact_id": grid }, "count": 100, "geometry": { "width": 5, "height": 5 }, "offset_pixels": [0.5, 0.0], "outside": "error", "execution": seed("0000000000000003") }));
    assert_eq!(err["code"], "OUT_OF_DOMAIN");
}

#[test]
fn logarithmic_imf_slopes_integrate() {
    let e = Env::new("imf-log");
    for slope in [-1.0, -2.0] {
        let r = e.ok("imf.integrate", json!({ "imf": { "kind": "power_law", "slope": slope }, "m_min": 1.0, "m_max": 10.0, "moment": "number" }));
        let v = r["result"]["raw_integral"].as_f64().unwrap();
        let expected = if slope == -1.0 { 301.0 * 10f64.ln() } else { 301.0f64.powi(2) * 0.9 };
        assert!((v / expected - 1.0).abs() < 1e-12, "slope {slope}: {v} vs {expected}");
    }
}

#[test]
fn instrumental_sky_scales_with_pixel_area() {
    let e = Env::new("sky");
    let conv = |ps: f64| -> f64 {
        let r = e.ok("imager.convert", json!({ "instrument": { "kind": "instrumental", "zero_points": { "i": 25 } }, "mode": "sky_counts", "band": "i", "values": [21.0], "exposure_s": 100, "pixel_scale_arcsec": ps }));
        r["result"]["result"]["values"][0].as_f64().unwrap()
    };
    assert!((conv(0.4) / conv(0.2) - 4.0).abs() < 1e-12);
}

#[test]
fn conflicting_instrument_configuration_is_rejected() {
    let e = Env::new("conflict");
    let err = e.err("imager.inspect", json!({ "instrument": { "kind": "instrumental", "zero_points": { "LSST_i": 25 }, "photometric_systems": ["LSST"] } }));
    assert_eq!(err["code"], "INVALID_PARAMETER");
}

#[test]
fn custom_filter_curves_make_a_valid_instrument() {
    let e = Env::new("curves");
    let f = e.ok("filters.import", json!({ "curves": [{ "name": "box", "wavelength": [4000.0, 5000.0, 6000.0], "transmission": [1.0, 1.0, 1.0] }] }));
    let cat = id(&f, "catalog_artifact_id");
    assert!((f["result"]["filters"][0]["dlam_angstrom"].as_f64().unwrap() - 2000.0).abs() < 1e-9);
    let i = e.ok("imager.inspect", json!({ "instrument": { "kind": "filter_curves", "catalog": { "artifact_id": cat }, "diameter": { "value": 1, "unit": "m" } } }));
    assert_eq!(i["result"]["bands"], json!(["box"]));
    let c = e.ok("imager.convert", json!({ "instrument": { "kind": "filter_curves", "catalog": { "artifact_id": cat } }, "mode": "mag_counts", "band": "box", "values": [20.0], "exposure_s": 10 }));
    assert!(c["result"]["result"]["values"][0].as_f64().unwrap() > 0.0);
}

fn population(e: &Env, iso: &str, count: u64, limit: Option<f64>, s: &str, extinction: Value) -> String {
    let mut req = json!({ "isochrone": { "artifact_id": iso }, "normalization": { "kind": "count", "value": count }, "distance": { "value": 2, "unit": "Mpc" }, "extinction_mag": extinction, "execution": seed(s) });
    if let Some(m) = limit {
        req["mag_limit"] = json!({ "magnitude": m, "band": "LSST_i" });
    }
    id(&e.ok("population.build", req), "population_artifact_id")
}

fn phot(e: &Env, pop: &str, band: &str) -> (f64, f64) {
    let r = e.ok("population.photometry", json!({ "artifact_id": pop, "bands": [band], "metrics": ["total_mag", "sbf_mag"] }));
    (r["result"]["total_mag"][band].as_f64().unwrap(), r["result"]["sbf_mag"][band].as_f64().unwrap())
}

#[test]
fn composition_with_unresolved_light_works_in_both_orders() {
    let e = Env::new("compose");
    let iso = e.fixture_isochrone();
    let full = population(&e, &iso, 5000, None, "0000000000000011", json!(0));
    let partial = population(&e, &iso, 200000, Some(29.0), "0000000000000012", json!(0));
    let total = |p: &str| 10f64.powf(-0.4 * phot(&e, p, "LSST_r").0);
    for order in [[&full, &partial], [&partial, &full]] {
        let c = e.ok("population.compose", json!({ "populations": [{ "artifact_id": order[0] }, { "artifact_id": order[1] }] }));
        let cid = id(&c, "population_artifact_id");
        assert!(c["result"]["has_unresolved_light"].as_bool().unwrap());
        let sum = total(&full) + total(&partial);
        assert!((total(&cid) / sum - 1.0).abs() < 1e-12);
    }
}

#[test]
fn one_magnitude_of_extinction_shifts_total_and_sbf_by_one() {
    let e = Env::new("extinction");
    let iso = e.fixture_isochrone();
    let p0 = population(&e, &iso, 100000, Some(28.0), "0000000000000021", json!(0));
    let p1 = id(&e.ok("population.update", json!({ "artifact_id": p0, "extinction_mag": 1.0 })), "population_artifact_id");
    let (t0, s0) = phot(&e, &p0, "LSST_i");
    let (t1, s1) = phot(&e, &p1, "LSST_i");
    assert!((t1 - t0 - 1.0).abs() < 1e-10, "total {t0} -> {t1}");
    assert!((s1 - s0 - 1.0).abs() < 1e-10, "sbf {s0} -> {s1}");
    // Per-band map: only LSST_g changes.
    let p2 = id(&e.ok("population.update", json!({ "artifact_id": p0, "extinction_mag": { "LSST_g": 0.5 } })), "population_artifact_id");
    assert!((phot(&e, &p2, "LSST_g").0 - phot(&e, &p0, "LSST_g").0 - 0.5).abs() < 1e-10);
    assert!((phot(&e, &p2, "LSST_i").0 - t0).abs() < 1e-12);
}

#[test]
fn uniform_surface_brightness_count_depends_on_the_imf() {
    let e = Env::new("uniform-sb");
    let iso = e.fixture_isochrone();
    let count = |imf: &str| -> u64 {
        let r = e.ok(
            "source.build",
            json!({ "uniform_surface_brightness": { "isochrone": { "artifact_id": iso }, "surface_brightness": 27.0, "band": "LSST_i", "distance": { "value": 5, "unit": "Mpc" }, "imf": { "kind": imf } }, "geometry": { "width": 51, "height": 51, "pixel_scale_arcsec": 0.2 }, "execution": seed("0000000000000031") }),
        );
        r["result"]["num_stars"].as_u64().unwrap()
    };
    let (k, s) = (count("kroupa"), count("salpeter"));
    assert_ne!(k, s, "ArtPop 0.1.2 used Kroupa regardless of the IMF");
}

#[test]
fn restricted_remnant_limit_cannot_subtract_mass() {
    let e = Env::new("remnants");
    let iso = e.fixture_isochrone();
    let r = e.ok("isochrone.integrated", json!({ "artifact_id": iso, "metrics": ["surviving_mass"], "imf_upper_mass": 2.0 }));
    let s = &r["result"]["surviving_mass"];
    let with = s["with_remnants"]["total"].as_f64().unwrap();
    let live = s["live_only"].as_f64().unwrap();
    assert!(with >= live, "with remnants {with} < live {live}");
    assert_eq!(s["with_remnants"]["black_holes"], 0.0);
}

#[test]
fn expected_variance_is_nonnegative_even_when_observed_pixels_are_negative() {
    let e = Env::new("variance");
    let cat = e.write("faint.csv", "x,y,i\n2,2,40\n");
    let src = id(&e.ok("source.build", json!({ "catalog": { "format": "csv", "path": cat, "magnitude_columns": { "i": "i" }, "magnitude_system": "AB" }, "geometry": { "width": 25, "height": 25, "pixel_scale_arcsec": 0.2 } })), "source_artifact_id");
    let obs = e.ok(
        "observe.artificial",
        json!({ "source": { "artifact_id": src }, "instrument": { "kind": "instrumental", "zero_points": { "i": 20 }, "read_noise_electrons": 1.0 }, "bandpass": "i", "exposure_s": 1, "include_observed_variance": true, "execution": seed("0000000000000041") }),
    );
    let oid = id(&obs, "observation_artifact_id");
    let stats = |plane: &str| e.ok("observation.inspect", json!({ "artifact_id": oid, "plane": plane, "statistics": true, "fields": ["read_noise"] }))["result"][format!("statistics:{plane}")].clone();
    assert_eq!(stats("expected_variance_counts")["negative_pixels"], 0);
    assert!(stats("observed_variance_estimator")["negative_pixels"].as_u64().unwrap() > 0, "read noise should make some ArtPop-style estimates negative");
    assert!(stats("raw_counts")["negative_pixels"].as_u64().unwrap() > 0, "negative observed pixels are legitimate");
}

#[test]
fn partly_invalid_positions_are_never_deposited() {
    let e = Env::new("invalid");
    let cat = e.write("nan.csv", "x,y,i\n1,1,20\nnan,2,20\n3,nan,20\n");
    let err = e.err("source.build", json!({ "catalog": { "format": "csv", "path": cat, "magnitude_columns": { "i": "i" } }, "geometry": { "width": 5, "height": 5 } }));
    assert_eq!(err["code"], "NONFINITE_VALUE");
    let s = e.ok("source.build", json!({ "catalog": { "format": "csv", "path": cat, "magnitude_columns": { "i": "i" } }, "geometry": { "width": 5, "height": 5 }, "invalid": "drop" }));
    assert_eq!(s["result"]["num_stars"], 1);
}

#[test]
fn off_grid_mist_uses_the_requested_rotation_and_accepts_age_10_3() {
    let e = Env::new("mist-rot");
    let ages = [10.25, 10.3];
    for (rot, shift) in [(0.4, 0.0), (0.0, 1.0)] {
        let dir = format!("MIST_v1.2_vvcrit{rot:.1}_LSST");
        for feh in artpop_core::mist::feh_grid() {
            e.write(&format!("grid/{dir}/{}", artpop_core::mist::file_name(feh, rot, "LSST")), &synthetic_mist(feh, rot, &ages, shift + feh * 0.1));
        }
        e.ok("data.import", json!({ "path": e.path(&format!("grid/{dir}")) }));
    }
    let mag = |rot: f64, feh: f64| -> f64 {
        let r = e.ok("isochrone.mist", json!({ "log_age_yr": 10.3, "feh": feh, "rotation": rot, "photometric_systems": ["LSST"] }));
        assert_eq!(r["result"]["grid_age_index"], 106);
        let iso = id(&r, "isochrone_artifact_id");
        let q = e.ok("isochrone.inspect", json!({ "artifact_id": iso, "columns": ["LSST_i"] }));
        q["result"]["page"]["rows"][0][0].as_f64().unwrap()
    };
    // Off-grid [Fe/H] = -1.1 interpolates -1.25 and -1.0 within one rotation grid.
    let m00 = mag(0.0, -1.1);
    let m04 = mag(0.4, -1.1);
    assert!((m00 - m04 - 1.0).abs() < 1e-9, "rotation 0.0 grid is offset by +1 mag: {m00} vs {m04}");
    let expected = 10.0 + 0.0 + (-1.1) * 0.1 + (10.3 - 9.0);
    assert!((m04 - expected).abs() < 1e-9, "{m04} vs {expected}");
}

#[test]
fn composition_keeps_extinction_labels_and_smooth_light() {
    let e = Env::new("scene");
    let iso = e.fixture_isochrone();
    let a = population(&e, &iso, 300000, Some(28.5), "0000000000000051", json!(0.3));
    let b = population(&e, &iso, 300000, Some(28.5), "0000000000000052", json!(0.0));
    let src = |p: &str, s: &str| -> String {
        id(
            &e.ok("source.build", json!({ "population": { "artifact_id": p }, "spatial_model": { "kind": "plummer", "scale_radius": { "value": 20, "unit": "pc" } }, "geometry": { "width": 101, "height": 101, "pixel_scale_arcsec": 0.2 }, "execution": seed(s) })),
            "source_artifact_id",
        )
    };
    let (sa, sb) = (src(&a, "0000000000000053"), src(&b, "0000000000000054"));
    let scene = e.ok("source.compose", json!({ "sources": [{ "artifact_id": sa }, { "artifact_id": sb }] }));
    assert_eq!(scene["result"]["smooth_components"].as_array().unwrap().len(), 2);
    assert_eq!(scene["result"]["labels"]["name"], "source");
    let sid = id(&scene, "source_artifact_id");
    let rows = e.ok("source.query", json!({ "artifact_id": sid, "columns": ["label", "component"], "selection": { "kind": "labels", "values": [2] }, "page": { "limit": 5 } }));
    assert!(rows["result"]["rows"].as_array().unwrap().iter().all(|r| r[0] == 2 && r[1] == 1));
    // Extinction survives: the first component's smooth light is fainter by 0.3 mag in every band.
    let sc = &scene["result"]["smooth_components"];
    let (ma, mb) = (sc[0]["apparent_mag"]["LSST_i"].as_f64().unwrap(), sc[1]["apparent_mag"]["LSST_i"].as_f64().unwrap());
    assert!((ma - mb - 0.3).abs() < 0.05, "{ma} vs {mb}");
}
