//! Shared helpers for service integration tests.

#![allow(dead_code)]

use artpop_service::Service;
use artpop_service::config::ServiceConfig;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub struct Env {
    pub svc: Service,
    pub dir: PathBuf,
}

impl Env {
    pub fn new(name: &str) -> Env {
        let dir = std::env::temp_dir().join(format!("artpop-svc-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut cfg = ServiceConfig::new(dir.join("artifacts"));
        cfg.data_dir = Some(dir.join("data"));
        cfg.allow_network = false;
        Env { svc: Service::new(cfg), dir }
    }

    /// Execute and require success; returns the full envelope.
    pub fn ok(&self, op: &str, req: Value) -> Value {
        let env = serde_json::to_value(self.svc.execute(op, with_version(req))).unwrap();
        assert_eq!(env["status"], "completed", "{op} failed: {}", serde_json::to_string_pretty(&env["error"]).unwrap());
        env
    }

    /// Execute and require an error; returns the error body.
    pub fn err(&self, op: &str, req: Value) -> Value {
        let env = serde_json::to_value(self.svc.execute(op, with_version(req))).unwrap();
        assert_eq!(env["status"], "error", "{op} unexpectedly succeeded");
        env["error"].clone()
    }

    pub fn path(&self, name: &str) -> String {
        self.dir.join(name).display().to_string()
    }

    pub fn write(&self, name: &str, text: &str) -> String {
        let p = self.dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, text).unwrap();
        p.display().to_string()
    }

    /// Import the bundled LSST test isochrone (ArtPop 0.1.2 fixture).
    pub fn fixture_isochrone(&self) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/lsst_feh_m1.00_10gyr_iso.csv");
        let bands: serde_json::Map<String, Value> = ["LSST_u", "LSST_g", "LSST_r", "LSST_i", "LSST_z", "LSST_y"].iter().map(|b| (b.to_string(), json!(b))).collect();
        let e = self.ok(
            "isochrone.import",
            json!({ "format": "csv", "path": path, "magnitude_columns": bands, "magnitude_system": "AB", "eep_column": "EEP", "log_l_column": "log_L", "log_teff_column": "log_Teff" }),
        );
        e["result"]["isochrone_artifact_id"].as_str().unwrap().to_string()
    }

    pub fn csv(&self, rel: &str) -> Vec<Vec<String>> {
        std::fs::read_to_string(self.dir.join(rel)).unwrap().lines().map(|l| l.split(',').map(String::from).collect()).collect()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn with_version(mut v: Value) -> Value {
    if let Value::Object(m) = &mut v {
        m.entry("schema_version").or_insert(json!("1"));
    }
    v
}

pub fn seed(s: &str) -> Value {
    json!({ "seed": s })
}

pub fn id(env: &Value, key: &str) -> String {
    env["result"][key].as_str().unwrap_or_else(|| panic!("no {key} in {}", env["result"])).to_string()
}

/// A tiny synthetic MIST `.iso.cmd` with the given ages; every column is
/// offset by `shift` so tests can tell grids apart.
pub fn synthetic_mist(feh: f64, rotation: f64, ages: &[f64], shift: f64) -> String {
    let mut s = String::new();
    s.push_str("# MIST version number  = 1.2\n# MESA revision number =     7503\n# photometric system   = LSST (AB)\n# ----\n");
    s.push_str("#  Yinit        Zinit   [Fe/H]   [a/Fe]  v/vcrit\n");
    s.push_str(&format!("# 0.2511  1.42857E-03    {feh:.2}     0.00     {rotation:.2}\n# ----\n"));
    s.push_str(&format!("# number of isochrones =   {}\n# CCM89 extinction: Av =  0.000\n# ----\n", ages.len()));
    for age in ages {
        s.push_str("# number of EEPs, cols =     4   13\n#   1   2   3   4   5   6   7   8   9  10  11  12  13\n");
        s.push_str("# EEP log10_isochrone_age_yr initial_mass star_mass log_Teff log_L LSST_u LSST_g LSST_r LSST_i LSST_z LSST_y phase\n");
        for (k, m) in [0.1, 0.4, 0.8, 0.85].iter().enumerate() {
            let eep = 200 + 100 * k;
            let mag = 10.0 - 8.0 * (m - 0.1) + shift + (age - 9.0);
            s.push_str(&format!("{eep} {age:.16E} {m:.16E} {:.16E} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.6} {:.1}\n", m * 0.99, 3.6 + shift * 0.01, -1.0 + m + shift * 0.01, mag + 2.0, mag + 1.0, mag + 0.5, mag, mag - 0.3, mag - 0.5, k as f64));
        }
        s.push_str("\n\n");
    }
    s
}
