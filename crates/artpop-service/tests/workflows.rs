//! Agent workflow tests (VALIDATION_PLAN §5) through the service layer.

mod common;

use common::{Env, id, seed, synthetic_mist};
use serde_json::{Value, json};

#[test]
fn offline_end_to_end_from_bundled_data_to_fits_and_png() {
    let e = Env::new("offline");
    let f = e.ok("filters.list", json!({ "systems": ["LSST"] }));
    assert_eq!(f["result"]["filter_count"], 6);
    let iso = e.fixture_isochrone();
    let pop = id(&e.ok("population.build", json!({ "isochrone": { "artifact_id": iso }, "normalization": { "kind": "current_mass", "value": 2e4 }, "distance": { "value": 3, "unit": "Mpc" }, "mag_limit": { "magnitude": 27, "band": "LSST_i" }, "execution": seed("0000000000000101") })), "population_artifact_id");
    let src = e.ok("source.build", json!({ "population": { "artifact_id": pop }, "spatial_model": { "kind": "sersic", "effective_radius": { "value": 30, "unit": "pc" }, "n": 0.8, "ellipticity": 0.3, "theta": { "value": 45, "unit": "deg" } }, "geometry": { "width": 151, "height": 121, "pixel_scale_arcsec": 0.2 }, "execution": seed("0000000000000102") }));
    let sid = id(&src, "source_artifact_id");
    assert_eq!(src["result"]["smooth_components"].as_array().unwrap().len(), 1, "cutoff population carries smooth light");
    let psf = id(&e.ok("psf.build", json!({ "kind": "moffat", "fwhm": { "value": 0.7, "unit": "arcsec" }, "pixel_scale_arcsec": 0.2, "width": 25, "height": 25 })), "psf_artifact_id");
    let obs = e.ok("observe.ideal", json!({ "source": { "artifact_id": sid }, "bandpass": "LSST_i", "psf": { "artifact_id": psf } }));
    let oid = id(&obs, "observation_artifact_id");
    // Total flux = deposited stars + captured smooth light (wrap conserves flux).
    let smooth = obs["result"]["smooth_components"][0]["captured_in_frame"].as_f64().unwrap();
    let stars = obs["result"]["histogram"]["deposited_signal"].as_f64().unwrap();
    let total = obs["result"]["image_sum"].as_f64().unwrap();
    assert!((total / (stars + smooth) - 1.0).abs() < 1e-9);
    let fits = e.path("out/i.fits");
    e.ok("artifact.export", json!({ "artifact_id": oid, "destination": fits, "format": "fits" }));
    let back = artpop_io::fits::read_image(std::path::Path::new(&fits), None, None).unwrap();
    assert_eq!((back.image.width, back.image.height), (151, 121));
    assert!(back.cards.iter().any(|c| c.starts_with("BANDPASS=") && c.contains("'LSST_i ")), "{:?}", back.cards);
    let pv = id(&e.ok("image.preview", json!({ "image": { "artifact_id": oid } })), "preview_artifact_id");
    let png = e.path("out/i.png");
    e.ok("artifact.export", json!({ "artifact_id": pv, "destination": png, "format": "png" }));
    assert_eq!(&std::fs::read(&png).unwrap()[1..4], b"PNG");
    // Exports never overwrite silently.
    assert_eq!(e.err("artifact.export", json!({ "artifact_id": oid, "destination": fits, "format": "fits" }))["path"], "/destination");
}

#[test]
fn missing_mist_data_is_typed_and_recoverable() {
    let e = Env::new("missing");
    let req = json!({ "log_age_yr": 9.0, "feh": 0.0, "photometric_systems": ["LSST"] });
    let err = e.err("isochrone.mist", req.clone());
    assert_eq!(err["code"], "DATA_MISSING");
    assert_eq!(err["details"]["fetch_request"]["operation"], "data.fetch");
    let dir = "MIST_v1.2_vvcrit0.4_LSST";
    for feh in artpop_core::mist::feh_grid() {
        e.write(&format!("grid/{dir}/{}", artpop_core::mist::file_name(feh, 0.4, "LSST")), &synthetic_mist(feh, 0.4, &[8.95, 9.0], 0.0));
    }
    e.ok("data.import", json!({ "path": e.path(&format!("grid/{dir}")) }));
    let ok = e.ok("isochrone.mist", req);
    assert_eq!(ok["result"]["rows"], 4);
    assert_eq!(ok["provenance"]["data_manifest_ids"].as_array().unwrap().len(), 1);
    // Tampering is detected on read.
    let f = e.path(&format!("grid/{dir}/{}", artpop_core::mist::file_name(0.0, 0.4, "LSST")));
    std::fs::write(&f, std::fs::read_to_string(&f).unwrap().replace("0.2511", "0.2512")).unwrap();
    assert_eq!(e.err("isochrone.mist", json!({ "log_age_yr": 9.0, "feh": 0.0, "photometric_systems": ["LSST"] }))["code"], "DATA_CORRUPT");
    let v = e.ok("data.verify", json!({ "dataset_id": "mist-v1.2-vvcrit0.4-LSST" }));
    assert_eq!(v["result"]["corrupt"].as_array().unwrap().len(), 1);
}

#[test]
fn three_band_observations_share_stars_with_independent_noise() {
    let e = Env::new("quickstart");
    let iso = e.fixture_isochrone();
    let pop = id(&e.ok("population.build", json!({ "isochrone": { "artifact_id": iso }, "normalization": { "kind": "count", "value": 20000 }, "distance": { "value": 1, "unit": "Mpc" }, "execution": seed("0000000000000201") })), "population_artifact_id");
    let src = id(&e.ok("source.build", json!({ "population": { "artifact_id": pop }, "spatial_model": { "kind": "plummer", "scale_radius": { "value": 5, "unit": "pc" } }, "geometry": { "width": 101, "height": 101, "pixel_scale_arcsec": 0.2 }, "execution": seed("0000000000000202") })), "source_artifact_id");
    let observe = |band: &str, s: &str| -> String {
        id(&e.ok("observe.artificial", json!({ "source": { "artifact_id": src }, "instrument": { "kind": "physical", "photometric_systems": ["LSST"], "diameter": { "value": 6.4, "unit": "m" }, "read_noise_electrons": 4 }, "bandpass": band, "exposure_s": 900, "sky_sb_mag_arcsec2": 21, "execution": seed(s) })), "observation_artifact_id")
    };
    let (g, r, i) = (observe("LSST_g", "0000000000000203"), observe("LSST_r", "0000000000000203"), observe("LSST_i", "0000000000000203"));
    assert!(g != r && r != i);
    // Deterministic replay: same request => same content-addressed artifact.
    assert_eq!(observe("LSST_r", "0000000000000203"), r);
    assert_ne!(observe("LSST_r", "0000000000000204"), r);
    let rgb = e.ok("image.rgb", json!({ "red": { "artifact_id": i }, "green": { "artifact_id": r }, "blue": { "artifact_id": g } }));
    assert!(rgb["result"]["width"].as_u64().unwrap() == 101);
}

#[test]
fn adding_bands_does_not_change_sampled_masses_or_positions() {
    let e = Env::new("streams");
    let iso6 = e.fixture_isochrone();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/lsst_feh_m1.00_10gyr_iso.csv");
    let iso2 = id(&e.ok("isochrone.import", json!({ "format": "csv", "path": path, "magnitude_columns": { "LSST_g": "LSST_g", "LSST_i": "LSST_i" }, "magnitude_system": "AB" })), "isochrone_artifact_id");
    let build = |iso: &str| id(&e.ok("population.build", json!({ "isochrone": { "artifact_id": iso }, "normalization": { "kind": "count", "value": 500 }, "execution": seed("0000000000000301") })), "population_artifact_id");
    let rows = |p: &str| e.ok("population.query", json!({ "artifact_id": p, "columns": ["initial_mass", "LSST_i"], "page": { "limit": 500 } }))["result"]["rows"].clone();
    assert_eq!(rows(&build(&iso6)), rows(&build(&iso2)));
}

#[test]
fn phases_composition_and_labels() {
    let e = Env::new("phases");
    let iso = e.fixture_isochrone();
    let pop = |n: u64, s: &str, label: &str| id(&e.ok("population.build", json!({ "isochrone": { "artifact_id": iso }, "normalization": { "kind": "count", "value": n }, "label": label, "execution": seed(s) })), "population_artifact_id");
    let (a, b) = (pop(30000, "0000000000000401", "old"), pop(10000, "0000000000000402", "older"));
    let counts = e.ok("population.phases", json!({ "artifact_id": a }));
    let c = &counts["result"]["counts"];
    assert_eq!(c["MS"].as_u64().unwrap() + c["giants"].as_u64().unwrap() + c["PMS"].as_u64().unwrap() + c["postAGB"].as_u64().unwrap() + c["WDCS"].as_u64().unwrap(), 30000);
    let mask = id(&e.ok("population.phases", json!({ "artifact_id": a, "mode": "mask", "phase": "giants" })), "mask_artifact_id");
    let giants = e.ok("population.photometry", json!({ "artifact_id": a, "bands": ["LSST_i"], "selection": { "kind": "mask", "artifact_id": mask } }));
    let all = e.ok("population.photometry", json!({ "artifact_id": a, "bands": ["LSST_i"] }));
    assert!(giants["result"]["total_mag"]["LSST_i"].as_f64().unwrap() > all["result"]["total_mag"]["LSST_i"].as_f64().unwrap());
    let comp = e.ok("population.compose", json!({ "populations": [{ "artifact_id": a, "label": "A" }, { "artifact_id": b, "label": "B" }] }));
    assert_eq!(comp["result"]["components"][1]["label"], "B");
    let fr: f64 = comp["result"]["number_fractions"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).sum();
    assert!((fr - 1.0).abs() < 1e-12);
    let cid = id(&comp, "population_artifact_id");
    let only_b = e.ok("population.query", json!({ "artifact_id": cid, "columns": ["component"], "selection": { "kind": "components", "ids": [1] }, "page": { "limit": 20 } }));
    assert_eq!(only_b["result"]["selected"], 10000);
    assert_eq!(only_b["result"]["rows"].as_array().unwrap().len(), 20);
    assert!(only_b["result"]["next_cursor"].is_string());
    let src = id(&e.ok("source.build", json!({ "population": { "artifact_id": cid }, "spatial_model": { "kind": "uniform" }, "geometry": { "width": 41, "height": 41 }, "label_type": "phases", "execution": seed("0000000000000403") })), "source_artifact_id");
    let q = e.ok("source.query", json!({ "artifact_id": src, "columns": ["label"], "selection": { "kind": "phases", "phases": ["giants"] }, "page": { "limit": 1000 } }));
    assert!(q["result"]["rows"].as_array().unwrap().iter().all(|r| ["RGB", "CHeB", "EAGB", "TPAGB"].contains(&r[0].as_str().unwrap())));
}

#[test]
fn ring_grid_and_empirical_psf() {
    let e = Env::new("ring");
    let g = e.ok("spatial.grid-build", json!({ "model": { "kind": "ring", "x0": 50, "y0": 50, "amplitude": 1, "r_in": 20, "width": 10 }, "width": 101, "height": 101 }));
    let grid = id(&g, "spatial_grid_artifact_id");
    let pos = id(&e.ok("spatial.sample", json!({ "grid": { "artifact_id": grid }, "count": 5000, "geometry": { "width": 101, "height": 101 }, "execution": seed("0000000000000501") })), "positions_artifact_id");
    let q = e.ok("artifact.export", json!({ "artifact_id": pos, "destination": e.path("ring.csv"), "format": "csv" }));
    assert_eq!(q["result"]["details"]["rows"], 5000);
    for row in e.csv("ring.csv").iter().skip(1) {
        let (x, y): (f64, f64) = (row[0].parse().unwrap(), row[1].parse().unwrap());
        let r = ((x - 50.0).powi(2) + (y - 50.0).powi(2)).sqrt();
        assert!((20.0..=30.0).contains(&r), "radius {r}");
    }
    let psf = id(&e.ok("psf.import", json!({ "values_yx": [[0, 1, 0], [1, 4, 1], [0, 1, 0]] })), "psf_artifact_id");
    let img = id(&e.ok("image.histogram", json!({ "positions": { "artifact_id": pos } })), "image_artifact_id");
    let conv = e.ok("image.convolve", json!({ "image": { "artifact_id": img }, "psf": { "artifact_id": psf } }));
    assert!((conv["result"]["convolution"]["output_sum"].as_f64().unwrap() - 5000.0).abs() < 1e-8);
    assert_eq!(e.err("psf.import", json!({ "values_yx": [[0, -1, 0], [1, 4, 1], [0, 1, 0]] }))["path"], "/allow_signed");
}

#[test]
fn embed_into_fits_background_with_clipping_and_headers() {
    let e = Env::new("embed");
    let bg = artpop_core::image::Image::new(20, 10, vec![1.0; 200]).unwrap();
    let bgp = e.path("bg.fits");
    artpop_io::fits::write_image(std::path::Path::new(&bgp), &bg, &[("TELESCOP", artpop_io::fits::Key::Str("HSC".into()), "")], &[]).unwrap();
    let stamp = id(&e.ok("spatial.evaluate", json!({ "model": { "kind": "constant", "amplitude": 2.0 }, "grid": { "width": 5, "height": 5 } })), "image_artifact_id");
    let r = e.ok("image.embed", json!({ "destination": { "fits": { "path": bgp } }, "source": { "artifact_id": stamp }, "center": [19, 0] }));
    // Center (19, 0) keeps a 3x3 corner of the 5x5 stamp: 9/25 of its signal.
    assert!((r["result"]["lost_fraction"].as_f64().unwrap() - 16.0 / 25.0).abs() < 1e-12);
    let out = e.path("embedded.fits");
    e.ok("artifact.export", json!({ "artifact_id": id(&r, "image_artifact_id"), "destination": out, "format": "fits" }));
    let back = artpop_io::fits::read_image(std::path::Path::new(&out), None, None).unwrap();
    assert!(back.cards.iter().any(|c| c.starts_with("TELESCOP=") && c.contains("'HSC ")), "{:?}", back.cards);
    assert!((back.image.sum() - (200.0 + 18.0)).abs() < 1e-12);
    assert_eq!(back.image.get(19, 0), 3.0);
}

#[test]
fn bounded_catalog_preview_and_strict_validation() {
    let e = Env::new("preview");
    let iso = e.fixture_isochrone();
    let pop = id(&e.ok("population.build", json!({ "isochrone": { "artifact_id": iso }, "normalization": { "kind": "count", "value": 100000 }, "execution": seed("0000000000000601") })), "population_artifact_id");
    let page = e.ok("population.query", json!({ "artifact_id": pop, "page": { "limit": 20 } }));
    let s = serde_json::to_string(&page).unwrap();
    assert!(s.len() < 20_000, "a 20-row page must stay small ({} bytes)", s.len());
    let next = page["result"]["next_cursor"].as_str().unwrap().to_string();
    let page2 = e.ok("population.query", json!({ "artifact_id": pop, "page": { "limit": 20, "cursor": next } }));
    assert_ne!(page["result"]["rows"][0], page2["result"]["rows"][0]);
    assert_eq!(e.err("population.query", json!({ "artifact_id": pop, "page": { "limit": 5000 } }))["path"], "/page/limit");
    // Unknown fields, units, and seeds produce actionable pointers.
    assert_eq!(e.err("population.build", json!({ "isochrone": { "artifact_id": iso }, "normalization": { "kind": "count", "value": 10 }, "distnace": { "value": 1, "unit": "pc" } }))["path"], "/distnace");
    assert_eq!(e.err("population.build", json!({ "isochrone": { "artifact_id": iso }, "normalization": { "kind": "count", "value": 10 }, "distance": { "value": 1, "unit": "arcsec" }, "execution": seed("0000000000000001") }))["code"], "UNIT_MISMATCH");
    assert_eq!(e.err("population.build", json!({ "isochrone": { "artifact_id": iso }, "normalization": { "kind": "count", "value": 10 } }))["path"], "/execution/seed");
    assert_eq!(e.err("population.inspect", json!({ "artifact_id": "sha256:".to_string() + &"0".repeat(64) }))["code"], "ARTIFACT_NOT_FOUND");
    let v: Value = e.ok("system.validate", json!({ "operation": "psf.build", "request": { "schema_version": "1", "kind": "gaussian", "fwhm": { "value": 2, "unit": "pixel" } } }));
    assert_eq!(v["result"]["resolved_request"]["width"], 41);
}

#[test]
fn every_operation_has_a_strict_object_schema() {
    for d in artpop_service::registry::all() {
        let s = d.input_schema();
        assert_eq!(s["type"], "object", "{} schema", d.id);
        assert_eq!(s["additionalProperties"], false, "{}", d.id);
        assert_eq!(s["required"][0], "schema_version", "{}", d.id);
    }
    assert_eq!(artpop_service::registry::all().len(), 68);
}

#[test]
fn parsec_import_selects_age_metallicity_and_filters() {
    let e = Env::new("parsec");
    let mut text = String::from("# File generated by CMD 3.7 (http://stev.oapd.inaf.it/cmd)\n# Photometric system: UBVRIJHK\n");
    text.push_str("# Zini     MH   logAge Mini        int_IMF         Mass   logL    logTe  logg  label   mbolmag  Umag    Bmag    Vmag\n");
    for (z, age) in [(0.0152, 9.0), (0.0152, 10.0), (0.004, 10.0)] {
        for (k, m) in [0.1, 0.5, 0.9, 1.0].iter().enumerate() {
            let v = 12.0 - 9.0 * m + (age - 9.0) + z * 10.0;
            text.push_str(&format!("{z} 0.0 {age} {m} {} {} {} 3.7 4.5 {k} {} {} {} {}\n", 0.1 * k as f64, m * 0.98, -1.0 + m, v - 0.2, v + 1.5, v + 0.8, v));
        }
    }
    text.push_str("#isochrone terminated\n");
    let path = e.write("parsec.dat", &text);
    let r = e.ok("isochrone.import", json!({ "format": "parsec", "path": path, "log_age": 10.0, "zini": 0.004, "magnitude_system": "Vega" }));
    assert_eq!(r["result"]["rows"], 4);
    assert_eq!(r["result"]["filters"], json!(["Umag", "Bmag", "Vmag"]));
    let iso = id(&r, "isochrone_artifact_id");
    let q = e.ok("isochrone.inspect", json!({ "artifact_id": iso, "columns": ["initial_mass", "current_mass", "Vmag", "log_L"] }));
    let row = &q["result"]["page"]["rows"][1];
    assert_eq!(row[0], 0.5);
    assert!((row[1].as_f64().unwrap() - 0.49).abs() < 1e-12);
    assert!((row[2].as_f64().unwrap() - (12.0 - 4.5 + 1.0 + 0.04)).abs() < 1e-12);
    // num_filters overrides detection; missing selections are typed errors.
    let two = e.ok("isochrone.import", json!({ "format": "parsec", "path": path, "log_age": 9.0, "zini": 0.0152, "num_filters": 2, "magnitude_system": "Vega" }));
    assert_eq!(two["result"]["filters"], json!(["Bmag", "Vmag"]));
    assert_eq!(e.err("isochrone.import", json!({ "format": "parsec", "path": path, "log_age": 8.0, "magnitude_system": "Vega" }))["code"], "OUT_OF_DOMAIN");
}
