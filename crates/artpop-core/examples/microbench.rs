//! Timing of primitive operations behind large observations (development aid).
use artpop_core::convolve::{Boundary, Method, convolve};
use artpop_core::image::Image;
use rayon::prelude::*;
use std::time::Instant;

fn time<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    println!("{label:42} {:8.1} ms", t.elapsed().as_secs_f64() * 1e3);
    r
}

fn main() {
    let n = 2047 * 2047;
    let v: Vec<f64> = time("alloc+fill 4.2M f64 (serial)", || (0..n).map(|i| i as f64).collect());
    let _w: Vec<f64> = time("par map 4.2M f64", || v.par_iter().map(|x| x * 2.0 + 1.0).collect());
    let _c: Vec<f64> = time("serial map 4.2M f64", || v.iter().map(|x| x * 2.0 + 1.0).collect());
    let _b: Vec<u8> = time("byte copy 33 MB", || unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, n * 8) }.to_vec());
    let img = Image::new(2047, 2047, v.iter().map(|x| (x % 97.0) + 1.0).collect()).unwrap();
    let k = artpop_core::psf::kernel(&artpop_core::psf::Profile::moffat_from_fwhm(3.5, 4.765), 41, 41, &Default::default()).unwrap().0;
    for (label, b) in [("convolve wrap 2047^2 x 41^2 (fft)", Boundary::Wrap), ("convolve fill 2047^2 x 41^2 (fft)", Boundary::Fill)] {
        time(label, || convolve(&img, &k, b, Method::Fft, true).unwrap());
    }
    let small = Image::new(1001, 1001, (0..1001 * 1001).map(|i| (i % 13) as f64).collect()).unwrap();
    time("convolve wrap 1001^2 x 41^2 (fft)", || convolve(&small, &k, Boundary::Wrap, Method::Fft, true).unwrap());
    println!("rayon threads: {}", rayon::current_num_threads());
}
