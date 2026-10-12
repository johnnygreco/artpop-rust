//! Direct tests for operations not covered by the deviation/workflow suites.

mod common;

use common::{Env, id, seed, synthetic_mist};
use serde_json::{Value, json};

#[test]
fn discovery_schema_estimate_and_citations() {
    let e = Env::new("discovery");
    let caps = e.ok("system.capabilities", json!({}));
    assert_eq!(caps["result"]["operation_count"], 68);
    assert_eq!(caps["result"]["filters"]["filter_count"], 164);
    assert!(caps["result"]["native_libraries"]["cfitsio"].as_str().unwrap().len() > 2);
    let only = e.ok("system.capabilities", json!({ "domain": "imf" }));
    assert_eq!(only["result"]["operations"].as_array().unwrap().len(), 4);
    let input = e.ok("system.schema", json!({ "operation": "population.build" }));
    assert!(input["result"]["schema"]["properties"]["normalization"].is_object());
    assert_eq!(input["result"]["mcp_tool"], "artpop_population_build");
    let output = e.ok("system.schema", json!({ "operation": "population.build", "side": "output" }));
    assert_eq!(output["result"]["schema"]["title"], "ResultEnvelope");
    let est = e.ok("system.estimate", json!({ "operation": "population.build", "request": { "schema_version": "1", "isochrone": { "artifact_id": "sha256:x" }, "normalization": { "kind": "count", "value": 1000000 } } }));
    assert_eq!(est["result"]["stars"]["maximum"], 1000000);
    let iso = e.fixture_isochrone();
    let cit = e.ok("system.citations", json!({ "artifact_ids": [iso] }));
    assert!(cit["result"]["references"].as_array().unwrap().len() >= 10);
    assert_eq!(cit["result"]["artifacts"][0]["kind"], "isochrone");
    assert_eq!(cit["result"]["bundled_data_provenance"]["upstream"]["commit"], "38a886906ed175d0205bf48b6dc03f54aeb4a81a");
}

#[test]
fn dataset_listing_and_filter_inspection() {
    let e = Env::new("datalist");
    assert_eq!(e.ok("data.list", json!({}))["result"]["registered"].as_array().unwrap().len(), 0);
    let dir = "MIST_v1.2_vvcrit0.0_HSC";
    for feh in artpop_core::mist::feh_grid() {
        let text = synthetic_mist(feh, 0.0, &[9.0], 0.0).replace("LSST (AB)", "HSC (AB)");
        e.write(&format!("grid/{dir}/{}", artpop_core::mist::file_name(feh, 0.0, "HSC")), &text);
    }
    e.ok("data.import", json!({ "path": e.path(&format!("grid/{dir}")) }));
    let l = e.ok("data.list", json!({ "photometric_system": "hsc" }));
    assert_eq!(l["result"]["registered"][0]["dataset_id"], "mist-v1.2-vvcrit0.0-HSC");
    let f = e.ok("filters.inspect", json!({ "filters": ["LSST_i", "WFC3_UVIS_F814W"] }));
    assert_eq!(f["result"]["filters"][1]["system"], "HST_WFC3");
    assert_eq!(f["result"]["filters"][1]["native_magnitude_system"], "Vega");
    assert_eq!(e.err("filters.inspect", json!({ "filters": ["nope"] }))["code"], "UNKNOWN_FILTER");
}

#[test]
fn imf_build_policies_and_nearest_masses() {
    let e = Env::new("imfbuild");
    let whole = e.ok("imf.build", json!({ "target_initial_mass": { "value": 5000, "unit": "Msun" }, "batch_size": 1000, "execution": seed("0000000000000701") }));
    let r = &whole["result"];
    assert!(r["realized_initial_mass_msun"].as_f64().unwrap() >= 5000.0);
    assert_eq!(r["draws"].as_u64().unwrap() % 1000, 0, "whole batches are retained");
    let near = e.ok("imf.build", json!({ "target_initial_mass": { "value": 5000, "unit": "Msun" }, "batch_size": 1000, "stopping": "nearest_target", "m_max": 2.0, "execution": seed("0000000000000701") }));
    assert!(near["result"]["overshoot_msun"].as_f64().unwrap().abs() <= 1.0);
    let iso = e.fixture_isochrone();
    let n = e.ok("isochrone.nearest", json!({ "artifact_id": iso, "masses": [0.1, 0.0, 5.0] }));
    assert_eq!(n["result"]["results"][0]["row"], 0);
    assert_eq!(n["result"]["results"][1]["row"], 0);
    assert_eq!(n["result"]["results"][2]["row"], 1466);
}

#[test]
fn inspect_clone_and_bundle_round_trip() {
    let e = Env::new("bundle");
    let cat = e.write("c.csv", "x,y,i,lab\n1,1,20,3\n2,3,21,4\n");
    let src = e.ok("source.build", json!({ "catalog": { "format": "csv", "path": cat, "magnitude_columns": { "i": "i" }, "label_column": "lab" }, "geometry": { "width": 7, "height": 5 } }));
    let sid = id(&src, "source_artifact_id");
    let si = e.ok("source.inspect", json!({ "artifact_id": sid, "fields": ["num_stars", "labels"] }));
    assert_eq!(si["result"], json!({ "num_stars": 2, "labels": { "kind": "numeric", "name": "lab" } }));
    let m = e.ok("artifact.inspect", json!({ "artifact_id": sid }));
    assert_eq!(m["result"]["manifest"]["kind"], "source");
    let dest = e.path("exported-bundle");
    let c = e.ok("artifact.clone", json!({ "artifact_id": sid, "export_destination": dest }));
    assert_eq!(c["result"]["artifact_id"], sid.as_str());
    // Import into a fresh store reproduces the same content-addressed ID.
    let other = Env::new("bundle-import");
    let imported = other.ok("artifact.import", json!({ "format": "bundle", "path": dest }));
    assert_eq!(imported["result"]["artifact_id"], sid.as_str());
    assert_eq!(imported["result"]["declared_id_matches"], true);
    let q = other.ok("source.query", json!({ "artifact_id": sid, "columns": ["label", "i"] }));
    assert_eq!(q["result"]["rows"], json!([[3.0, 20.0], [4.0, 21.0]]));
}

#[test]
fn smooth_light_and_combine_reproduce_ideal_observation() {
    let e = Env::new("smooth");
    let iso = e.fixture_isochrone();
    let pop = id(&e.ok("population.build", json!({ "isochrone": { "artifact_id": iso }, "normalization": { "kind": "current_mass", "value": 1e5 }, "distance": { "value": 4, "unit": "Mpc" }, "mag_limit": { "magnitude": 26.5, "band": "LSST_i" }, "execution": seed("0000000000000801") })), "population_artifact_id");
    let src = e.ok("source.build", json!({ "population": { "artifact_id": pop }, "spatial_model": { "kind": "sersic", "effective_radius": { "value": 60, "unit": "pc" }, "n": 1.2, "ellipticity": 0.2, "theta": { "value": 20, "unit": "deg" } }, "geometry": { "width": 121, "height": 101, "pixel_scale_arcsec": 0.2 }, "execution": seed("0000000000000802") }));
    let sid = id(&src, "source_artifact_id");
    let smooth = e.ok("image.smooth", json!({ "source": { "artifact_id": sid }, "bandpass": "LSST_i" }));
    let comp = &smooth["result"]["smooth_components"][0];
    let (captured, intrinsic) = (comp["captured_in_frame"].as_f64().unwrap(), comp["intrinsic_total"].as_f64().unwrap());
    assert!(captured > 0.3 * intrinsic && captured < intrinsic, "{captured} of {intrinsic}");
    let stars = e.ok("image.histogram", json!({ "source": { "artifact_id": sid }, "weights": { "kind": "magnitude_flux", "band": "LSST_i", "zpt": 27 } }));
    let combined = e.ok("image.combine", json!({ "images": [{ "artifact_id": id(&stars, "image_artifact_id") }, { "artifact_id": id(&smooth, "image_artifact_id") }], "allow_mixed_units": true }));
    let ideal = e.ok("observe.ideal", json!({ "source": { "artifact_id": sid }, "bandpass": "LSST_i" }));
    let read = |aid: &str, plane: Option<&str>| -> Vec<f64> {
        let p = e.path(&format!("{}.fits", &aid[7..19]));
        let mut req = json!({ "artifact_id": aid, "destination": p, "format": "fits" });
        if let Some(pl) = plane {
            req["plane"] = json!(pl);
        }
        e.ok("artifact.export", req);
        artpop_io::fits::read_image(std::path::Path::new(&p), None, None).unwrap().image.data
    };
    assert_eq!(read(&id(&combined, "image_artifact_id"), None), read(&id(&ideal, "observation_artifact_id"), Some("image")));
    assert_eq!(e.err("image.combine", json!({ "images": [{ "artifact_id": id(&stars, "image_artifact_id") }, { "artifact_id": id(&ideal, "observation_artifact_id") }] }))["code"], "UNIT_MISMATCH");
}

#[test]
fn plots_render_as_svg() {
    let e = Env::new("plots");
    let iso = e.fixture_isochrone();
    let pop = id(&e.ok("population.build", json!({ "isochrone": { "artifact_id": iso }, "normalization": { "kind": "count", "value": 3000 }, "execution": seed("0000000000000901") })), "population_artifact_id");
    let src = id(&e.ok("source.build", json!({ "population": { "artifact_id": pop }, "spatial_model": { "kind": "uniform" }, "geometry": { "width": 31, "height": 31 }, "execution": seed("0000000000000902") })), "source_artifact_id");
    for req in [
        json!({ "kind": "cmd", "artifact_id": pop, "blue": "LSST_g", "red": "LSST_i" }),
        json!({ "kind": "cmd", "artifact_id": iso, "blue": "LSST_g", "red": "LSST_i", "band": "LSST_r" }),
        json!({ "kind": "hr", "artifact_id": iso }),
        json!({ "kind": "spatial", "artifact_id": src }),
        json!({ "kind": "imf", "imf": { "kind": "scalo" } }),
    ] {
        let p = e.ok("plot.render", req);
        let out = e.path(&format!("{}.svg", p["result"]["kind"].as_str().unwrap()));
        e.ok("artifact.export", json!({ "artifact_id": id(&p, "plot_artifact_id"), "destination": out, "format": "svg", "overwrite": true }));
        let svg = std::fs::read_to_string(&out).unwrap();
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>") && svg.contains("<circle") | svg.contains("<path"));
    }
}

#[test]
fn simulate_run_end_to_end() {
    let e = Env::new("simulate");
    let iso = e.fixture_isochrone();
    let out = e.path("run");
    let r = e.ok(
        "simulate.run",
        json!({
            "population": { "isochrone": { "artifact_id": iso }, "normalization": { "kind": "count", "value": 200000 }, "distance": { "value": 3, "unit": "Mpc" }, "mag_limit": { "magnitude": 27.5, "band": "LSST_i" } },
            "source": { "spatial_model": { "kind": "sersic", "effective_radius": { "value": 80, "unit": "pc" }, "n": 0.8, "ellipticity": 0.3, "theta": { "value": 135, "unit": "deg" } }, "geometry": { "width": 151, "height": 151, "pixel_scale_arcsec": 0.2 } },
            "psf": { "kind": "moffat", "fwhm": { "value": 0.6, "unit": "arcsec" }, "width": 21, "height": 21 },
            "instrument": { "kind": "physical", "photometric_systems": ["LSST"], "diameter": { "value": 6.4, "unit": "m" }, "read_noise_electrons": 4 },
            "observations": [{ "bandpass": "LSST_g", "exposure_s": 900, "sky_sb_mag_arcsec2": 22 }, { "bandpass": "LSST_i", "exposure_s": 1800, "sky_sb_mag_arcsec2": 20 }],
            "render": { "kind": "artificial" },
            "outputs": { "directory": out, "export_formats": ["fits", "png", "csv"], "include_variance": true },
            "execution": seed("0000000000000a01"),
        }),
    );
    let obs = r["result"]["stages"]["observations"].as_array().unwrap();
    assert_eq!(obs.len(), 2);
    assert_eq!(obs[0]["exports"].as_array().unwrap().len(), 6, "5 FITS planes + PNG");
    for f in ["LSST_g_image.fits", "LSST_i_raw_counts.fits", "LSST_i_expected_variance_calibrated.fits", "LSST_i_preview.png", "population.csv"] {
        assert!(std::path::Path::new(&out).join(f).is_file(), "{f}");
    }
    // The PSF inherited the source pixel scale; smooth light was rendered.
    assert!((r["result"]["stages"]["psf"]["fwhm_pixels"].as_f64().unwrap() - 3.0).abs() < 1e-12);
    assert_eq!(obs[1]["summary"]["smooth_components"].as_array().unwrap().len(), 1);
    // Same request => same artifacts (deterministic replay, overwrite disabled for files).
    let kinds: Vec<Value> = r["artifacts"].as_array().unwrap().iter().map(|a| a["kind"].clone()).collect();
    assert!(kinds.contains(&json!("population")) && kinds.contains(&json!("observation")) && kinds.contains(&json!("preview")));
}
