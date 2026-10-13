//! Check whether huge-page advice takes effect for large buffers (development aid).
use std::time::Instant;

fn rollup(key: &str) -> String {
    std::fs::read_to_string("/proc/self/smaps_rollup").ok().and_then(|s| s.lines().find(|l| l.starts_with(key)).map(|l| l.to_string())).unwrap_or_default()
}

fn main() {
    for advised in [false, true] {
        let n = 20_000_000usize;
        let t = Instant::now();
        let mut v: Vec<f64> = if advised { artpop_core::mem::zeroed_f64(n) } else { vec![0.0; n] };
        let t_alloc = t.elapsed();
        let t = Instant::now();
        for i in (0..n).step_by(512) {
            v[i] = 1.0;
        }
        let t_touch = t.elapsed();
        println!("advised={advised:<5} alloc {:6.2} ms  first-touch 160 MB {:6.2} ms  {}  {}", t_alloc.as_secs_f64() * 1e3, t_touch.as_secs_f64() * 1e3, rollup("AnonHugePages"), rollup("Rss"));
        drop(v);
    }
    let n = 20_000_000usize;
    let t = Instant::now();
    let mut v = vec![0.0f64; n];
    artpop_core::mem::advise_huge(&mut v);
    artpop_core::mem::prefault(&mut v);
    let t_pre = t.elapsed();
    let t = Instant::now();
    use rayon::prelude::*;
    v.par_chunks_mut(1 << 15).for_each(|c| c[0] = 1.0);
    println!("advised+prefault alloc+prefault {:6.2} ms  parallel first-touch {:6.2} ms  {}", t_pre.as_secs_f64() * 1e3, t.elapsed().as_secs_f64() * 1e3, rollup("AnonHugePages"));
    let t = Instant::now();
    let mut w = vec![0.0f64; n];
    artpop_core::mem::advise_huge(&mut w);
    w.par_chunks_mut(1 << 15).for_each(|c| c[0] = 1.0);
    println!("advised, no prefault: parallel first-touch {:6.2} ms", t.elapsed().as_secs_f64() * 1e3);
}
