//! Timing of CSV parsing strategies on the benchmark catalog (development aid).
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/arp-bench-final/inputs/catalog.csv".into());
    let text = std::fs::read_to_string(&path).unwrap();
    let tokens = text.matches(',').count() + text.lines().count();
    for _ in 0..3 {
        let t = Instant::now();
        let table = artpop_io::csv::parse(&text, None).unwrap();
        println!("csv::parse (parallel)            {:7.1} ms  rows {}", t.elapsed().as_secs_f64() * 1e3, table.rows());
    }
    let t = Instant::now();
    let mut sum = 0.0;
    for line in text.lines().skip(1) {
        for tok in line.split(',') {
            sum += tok.parse::<f64>().unwrap();
        }
    }
    let dt = t.elapsed().as_secs_f64();
    println!("split + f64::from_str (1 thread) {:7.1} ms  {:5.1} ns/token  ({sum:.3e})", dt * 1e3, dt * 1e9 / tokens as f64);
    let t = Instant::now();
    let mut count = 0usize;
    for line in text.lines().skip(1) {
        for _tok in line.split(',') {
            count += 1;
        }
    }
    let dt = t.elapsed().as_secs_f64();
    println!("split only (1 thread)            {:7.1} ms  {:5.1} ns/token  ({count})", dt * 1e3, dt * 1e9 / tokens as f64);
    let t = Instant::now();
    let bytes = text.as_bytes();
    let mut sum2 = 0.0;
    let mut start = text.find('\n').unwrap() + 1;
    while start < bytes.len() {
        let mut end = start;
        while end < bytes.len() && bytes[end] != b',' && bytes[end] != b'\n' {
            end += 1;
        }
        if end > start {
            sum2 += std::str::from_utf8(&bytes[start..end]).unwrap().parse::<f64>().unwrap();
        }
        start = end + 1;
    }
    let dt = t.elapsed().as_secs_f64();
    println!("byte scan + from_str (1 thread)  {:7.1} ms  {:5.1} ns/token  ({sum2:.3e})", dt * 1e3, dt * 1e9 / tokens as f64);
}
