"""Generate a source-linked declaration inventory and proposed port mapping."""
import ast
import hashlib
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
UP = ROOT / "ArtPop"
OUT = ROOT / "artpop-rust/docs"
COMMIT = subprocess.check_output(["git", "-C", str(UP), "rev-parse", "HEAD"], text=True).strip()

FUNCTIONS = {
    "get_filter_names": "filters.list", "get_filter_properties": "filters.inspect",
    "phot_system_lookup": "filters.list", "load_zero_point_converter": "filters.convert",
    "fnu_from_AB_mag": "imager.convert", "gaussian_psf": "psf.build", "moffat_psf": "psf.build",
    "kroupa": "imf.evaluate", "scalo": "imf.evaluate", "salpeter": "imf.evaluate",
    "sample_imf": "imf.sample", "build_galaxy": "imf.build",
    "fetch_mist_iso_cmd": "isochrone.mist", "constant_sb_stars_per_pix": "population.surface-density",
    "xy_from_grid": "spatial.sample", "sersic_xy": "spatial.sample", "plummer_xy": "spatial.sample", "uniform_xy": "spatial.sample",
    "check_random_state": "system.validate", "check_units": "system.validate", "check_odd": "system.validate", "check_xy_dim": "system.validate",
    "embed_slices": "image.embed", "fetch_mist_grid_if_needed": "data.fetch", "show_image": "image.preview",
}


def operation(module, owner, name):
    if owner is None:
        if name in FUNCTIONS: return FUNCTIONS[name]
        if name in ["phot_system_list"]: return "filters.list"
        if name in ["mpl_style", "jpg_style"]: return "image.preview"
        if name in ["kroupa_params", "scalo_params", "salpeter_params", "imf_params_dict"]: return "imf.evaluate"
        return "system.capabilities"
    if name in ["to_pickle", "from_pickle", "copy"]: return "artifact.export / artifact.import / artifact.clone"
    if owner in ["FilterSystem", "ZeroPointConverter"]:
        return "filters.convert" if owner == "ZeroPointConverter" else ("filters.import" if name == "__init__" else "filters.inspect")
    if owner in ["IsoReader", "IsoCmdReader"]: return "isochrone.import / isochrone.inspect"
    if owner in ["Isochrone", "MISTIsochrone"]:
        return {"__init__": "isochrone.mist" if owner == "MISTIsochrone" else "isochrone.import", "from_parsec":"isochrone.import", "interpolate":"isochrone.interpolate", "mag_to_mass":"isochrone.invert", "calculate_mag_limit":"isochrone.magnitude-limit", "nearest_mini":"isochrone.nearest", "imf_weights":"isochrone.weights", "ssp_color":"isochrone.integrated", "ssp_sbf_mag":"isochrone.integrated", "ssp_mag":"isochrone.integrated", "ssp_mean_star_mass":"isochrone.integrated", "ssp_surviving_mass":"isochrone.integrated"}.get(name,"isochrone.inspect")
    if owner == "IMFIntegrator":
        return "imf.integrate" if name in ["integrate","m_integrate"] else "imf.evaluate"
    if owner in ["StellarPopulation", "SSP", "MISTSSP", "CompositePopulation"]:
        if name in ["__init__", "build_pop"]: return "population.compose" if owner == "CompositePopulation" else "population.build"
        if name == "__add__": return "population.compose"
        if name in ["set_distance", "a_lam"]: return "population.update / population.inspect"
        if name in ["abs_mag_table", "mag_table", "star_mags"]: return "population.query"
        if name in ["mag_integrated_component", "sbf_mag", "integrated_color", "mean_mag", "total_mag"]: return "population.photometry"
        if name == "sample_fraction": return "population.sample-fraction"
        if name in ["select_phase", "get_star_phases", "phases"]: return "population.phases"
        return "population.inspect"
    if module == "source":
        if name == "__init__": return "source.build"
        if name == "__add__": return "source.compose"
        if name in ["select_stars", "x", "y"]: return "source.query"
        if name == "mag_to_image_amplitude": return "source.amplitude"
        return "source.inspect"
    if owner in ["Plummer2D", "Constant2D"]: return "spatial.evaluate"
    if owner in ["Observation", "IdealObservation", "ArtObservation"]:
        if name == "to_fits": return "artifact.export"
        return "observation.inspect / artifact.import"
    if owner in ["Imager", "IdealImager", "ArtImager"]:
        return {"inject_stars":"image.histogram", "apply_seeing":"image.convolve", "inject_smooth_model":"image.smooth", "observe":"observe.ideal" if owner == "IdealImager" else "observe.artificial", "__init__":"imager.inspect", "area":"imager.inspect", "mag_to_counts":"imager.convert", "sb_to_counts_per_pixel":"imager.convert", "calibration":"imager.convert"}[name]
    raise ValueError((module, owner, name))


rows, modules = [], []
for p in sorted((UP / "src/artpop").rglob("*.py")):
    if p.name in ["__init__.py", "log.py"]: continue
    rel = str(p.relative_to(UP))
    module = str(p.relative_to(UP / "src/artpop").with_suffix("")).replace("/", ".")
    tree = ast.parse(p.read_text())
    exports = []
    for n in tree.body:
        if isinstance(n, ast.Assign) and any(isinstance(t,ast.Name) and t.id == "__all__" for t in n.targets): exports = ast.literal_eval(n.value)
    modules.append({"module":module, "file":rel, "exports":exports})
    def add(n, owner=None, kind="function", name=None):
        name = name or n.name
        rows.append({"module":module, "symbol":f"{owner}.{name}" if owner else name, "kind":kind, "owner":owner, "exported_owner":(owner or name) in exports, "line":n.lineno, "file":rel, "signature":ast.unparse(n.args) if isinstance(n,ast.FunctionDef) else None, "operation":operation(module,owner,name), "status":"specified; not implemented"})
    for n in tree.body:
        if isinstance(n,ast.FunctionDef) and not n.name.startswith("_"): add(n)
        elif isinstance(n,ast.ClassDef):
            # Class declaration documents inherited constructors and behavior too.
            rows.append({"module":module,"symbol":n.name,"kind":"class","owner":None,"exported_owner":n.name in exports,"line":n.lineno,"file":rel,"bases":[ast.unparse(b) for b in n.bases],"operation":operation(module,n.name,"__init__") if n.name not in ["Imager", "Plummer2D", "Constant2D", "CompositeSource"] else {"Imager":"imager.inspect","Plummer2D":"spatial.evaluate","Constant2D":"spatial.evaluate","CompositeSource":"source.compose"}[n.name],"status":"specified; not implemented"})
            for m in n.body:
                if isinstance(m,ast.FunctionDef) and (not m.name.startswith("_") or m.name in ["__init__","__add__"]):
                    decorators=[ast.unparse(d) for d in m.decorator_list]
                    kind="property" if "property" in decorators else "setter" if any(d.endswith(".setter") for d in decorators) else "method"
                    add(m,n.name,kind)
                elif isinstance(m,ast.Assign):
                    for target in m.targets:
                        if isinstance(target,ast.Name) and not target.id.startswith("_"): add(m,n.name,"class attribute",target.id)
        elif isinstance(n,ast.Assign):
            for target in n.targets:
                if isinstance(target,ast.Name) and target.id in exports: add(n,None,"constant",target.id)

manifest={"repository":"https://github.com/ArtificialStellarPopulations/ArtPop", "commit":COMMIT,"analysis_date":"2026-10-12","modules":modules,"declarations":rows}
(OUT / "evidence/api-inventory.json").write_text(json.dumps(manifest,indent=2)+"\n")
lines=["# API coverage ledger", "", f"Pinned upstream: `{COMMIT}`. This is a specification ledger; **no Rust operations are implemented in this pass**.", "", "Every public declaration in the scientific modules is listed, including module-accessible base classes and the MIST readers. Inherited methods are listed once at their defining class; the `bases` fields in the JSON inventory retain inheritance. Constructors map to typed requests, properties to inspection/query results, operators to composition operations. Python pickle methods map to portable artifact persistence, not pickle compatibility. `MISTIsochrone.from_parsec` is an upstream rejection path; the Rust import dispatcher routes PARSEC to the generic isochrone type.", "", "All operation IDs below have both a CLI route (dots become spaces) and an MCP tool (`artpop_` plus dots/hyphens replaced with underscores). Slash-separated entries describe several related operations, not one command. Detailed contracts are in [CLI_MCP_CONTRACT.md](CLI_MCP_CONTRACT.md).", "", "Logging internals and Python import/representation mechanics become Rust tracing and typed display/serialization. Third-party object APIs and arbitrary Python keyword forwarding are not independent ArtPop scientific features; supported replacements are specified explicitly.", ""]
for m in modules:
    lines.extend([f"## `{m['module']}`", "", "| Upstream declaration | Kind | Proposed operation(s) |", "| --- | --- | --- |"])
    for r in rows:
        if r['module'] != m['module']: continue
        url=f"https://github.com/ArtificialStellarPopulations/ArtPop/blob/{COMMIT}/{r['file']}#L{r['line']}"
        lines.append(f"| [`{r['symbol']}`]({url}) | {r['kind']} | `{r['operation']}` |")
    lines.append("")
(OUT / "API_COVERAGE.md").write_text("\n".join(lines)+"\n")
assets=[]
for p in sorted((UP / "src/artpop/data").iterdir()):
    assets.append({"path":str(p.relative_to(UP)),"bytes":p.stat().st_size,"sha256":hashlib.sha256(p.read_bytes()).hexdigest()})
(OUT / "evidence/upstream-assets.json").write_text(json.dumps(assets,indent=2)+"\n")
print(json.dumps({"declarations":len(rows),"module_exports":sum(len(m['exports']) for m in modules),"operations":sorted({o.strip() for r in rows for o in r['operation'].split('/')})},indent=2))
