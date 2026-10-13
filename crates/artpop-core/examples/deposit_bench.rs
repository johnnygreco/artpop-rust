//! Timing of the weighted histogram deposit at benchmark sizes (development aid).
use artpop_core::image::{InvalidPolicy, deposit};
use artpop_core::spatial::Coordinates;
use std::time::Instant;

fn main() {
    let mut st = artpop_core::rng::Seed(3).stream("bench", &[]);
    for (n, w, weighted, conv) in [(1_000_000usize, 2047usize, true, Coordinates::PixelCentersV1), (10_000_000, 2047, false, Coordinates::PixelCentersV1), (10_000_000, 2047, false, Coordinates::ArtPop012), (100_000, 201, false, Coordinates::ArtPop012)] {
        let xs: Vec<f64> = (0..n).map(|_| st.uniform_range(0.0, w as f64)).collect();
        let ys: Vec<f64> = (0..n).map(|_| st.uniform_range(0.0, w as f64)).collect();
        let ws: Option<Vec<f64>> = weighted.then(|| (0..n).map(|_| st.uniform()).collect());
        let mask = vec![true; n];
        for _ in 0..3 {
            let t = Instant::now();
            let (img, stats) = deposit(conv, w, w, &xs, &ys, ws.as_deref(), Some(&mask), InvalidPolicy::Error).unwrap();
            println!("deposit n={n:>9} {w}^2 weighted={weighted:<5} {conv:?} {:7.1} ms ({} deposited, sum {:.3e})", t.elapsed().as_secs_f64() * 1e3, stats.deposited, img.sum());
        }
    }
}
