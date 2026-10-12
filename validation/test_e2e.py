"""Reference workflows run now; Rust comparisons require a real target binary."""
from pathlib import Path
import json
import shutil

import pytest

from harness import CASES, prepare, run_python, run_rust, check


@pytest.fixture(scope="session")
def shared_inputs(tmp_path_factory):
    return prepare(tmp_path_factory.mktemp("inputs"), count=20000)


@pytest.fixture(scope="session")
def reference_outputs(shared_inputs, tmp_path_factory):
    root=tmp_path_factory.mktemp("python")
    for case in CASES:
        run_python(case, shared_inputs, root/case)
    return root


@pytest.mark.parametrize("case",CASES)
def test_python_reference_e2e(case, shared_inputs, reference_outputs):
    check(case,shared_inputs,reference_outputs/case)


@pytest.mark.parametrize("case",CASES)
def test_rust_matches_reference_e2e(case, shared_inputs, reference_outputs, tmp_path, request):
    binary=request.config.getoption("--rust-bin")
    if binary is None:
        pytest.skip("Rust implementation not supplied; this is not a parity pass")
    assert Path(binary).is_file(), f"Rust binary does not exist: {binary}"
    run_rust(case,shared_inputs,tmp_path,binary)
    check(case,shared_inputs,tmp_path,reference_outputs/case)


def test_comparator_rejects_flux_preserving_pixel_error(shared_inputs, reference_outputs, tmp_path):
    from astropy.io import fits
    shutil.copytree(reference_outputs/"ideal",tmp_path,dirs_exist_ok=True)
    image=fits.getdata(tmp_path/"image.fits")
    image[0,0]+=1
    image[0,1]-=1
    fits.writeto(tmp_path/"image.fits",image,overwrite=True)
    with pytest.raises(AssertionError):
        check("ideal",shared_inputs,tmp_path,reference_outputs/"ideal")


def test_comparator_rejects_grid_axis_swap(shared_inputs, reference_outputs, tmp_path):
    import numpy as np
    xy=np.loadtxt(reference_outputs/"grid/positions.csv",delimiter=",",skiprows=1)
    np.savetxt(tmp_path/"positions.csv",xy[:,::-1],delimiter=",",header="x,y",comments="")
    with pytest.raises(AssertionError):
        check("grid",shared_inputs,tmp_path)


def test_comparator_rejects_inconsistent_population_mass(shared_inputs, reference_outputs, tmp_path):
    shutil.copytree(reference_outputs/"population",tmp_path,dirs_exist_ok=True)
    summary=json.loads((tmp_path/"summary.json").read_text())
    summary["total_mass_msun"]*=1.01
    (tmp_path/"summary.json").write_text(json.dumps(summary))
    with pytest.raises(AssertionError):
        check("population",shared_inputs,tmp_path)


def test_comparator_rejects_missing_detector_noise(shared_inputs, reference_outputs, tmp_path):
    from astropy.io import fits
    shutil.copytree(reference_outputs/"detector",tmp_path,dirs_exist_ok=True)
    summary=json.loads((tmp_path/"summary.json").read_text())
    source=fits.getdata(tmp_path/"source_counts.fits")
    fits.writeto(tmp_path/"raw_counts.fits",source+summary["sky_counts"],overwrite=True)
    fits.writeto(tmp_path/"image.fits",source*summary["calibration"],overwrite=True)
    with pytest.raises(AssertionError):
        check("detector",shared_inputs,tmp_path)
