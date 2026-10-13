//! Allocation helpers for large numeric buffers.
//!
//! Buffers of tens to hundreds of megabytes are otherwise faulted in by the
//! kernel in 4 KiB pages; advising transparent huge pages (Linux
//! `MADV_HUGEPAGE`, effective when THP is in `madvise` or `always` mode) cuts
//! the number of page faults by about 500x. The advice is only a paging hint:
//! values, layout, and results never depend on it, and it is a no-op elsewhere.

use rayon::prelude::*;

/// Buffers at least this large are advised for huge pages.
pub const HUGE_PAGE_ADVICE_MIN_BYTES: usize = 4 << 20;

/// Elements per parallel work item for element-wise maps.
pub const PAR_CHUNK: usize = 1 << 15;

/// Advise transparent huge pages for the allocation behind `v` (call before
/// the memory is first touched for the full effect).
pub fn advise_huge<T>(v: &mut Vec<T>) {
    #[cfg(target_os = "linux")]
    {
        let bytes = v.capacity() * std::mem::size_of::<T>();
        if bytes < HUGE_PAGE_ADVICE_MIN_BYTES {
            return;
        }
        let page = 4096usize;
        let start = v.as_mut_ptr() as usize;
        let aligned = (start + page - 1) & !(page - 1);
        let end = (start + bytes) & !(page - 1);
        if end > aligned {
            // SAFETY: the range lies inside the live allocation owned by `v`;
            // MADV_HUGEPAGE only changes the kernel's paging policy and the
            // return value is deliberately ignored (the hint may be unsupported).
            unsafe {
                libc::madvise(aligned as *mut libc::c_void, end - aligned, libc::MADV_HUGEPAGE);
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = v;
    }
}

/// `vec![0.0; n]` (zeroed by the allocator, untouched) with huge-page advice.
pub fn zeroed_f64(n: usize) -> Vec<f64> {
    let mut v = vec![0.0f64; n];
    advise_huge(&mut v);
    v
}

/// `vec![0; n]` for `u32` with huge-page advice.
pub fn zeroed_u32(n: usize) -> Vec<u32> {
    let mut v = vec![0u32; n];
    advise_huge(&mut v);
    v
}

/// `Vec::with_capacity(n)` with huge-page advice (for push/extend fills).
pub fn with_capacity<T>(n: usize) -> Vec<T> {
    let mut v = Vec::with_capacity(n);
    advise_huge(&mut v);
    v
}

/// Element-wise map evaluated in parallel chunks; `out[i] = f(src[i])`
/// exactly as a sequential loop would compute it.
pub fn par_map_f64(src: &[f64], f: impl Fn(f64) -> f64 + Sync) -> Vec<f64> {
    let mut out = zeroed_f64(src.len());
    out.par_chunks_mut(PAR_CHUNK).zip(src.par_chunks(PAR_CHUNK)).for_each(|(o, s)| {
        for (a, b) in o.iter_mut().zip(s) {
            *a = f(*b);
        }
    });
    out
}

/// Gather `src[idx[i]]` for every index, in parallel chunks.
pub fn par_gather_f64(src: &[f64], idx: &[usize]) -> Vec<f64> {
    let mut out = zeroed_f64(idx.len());
    out.par_chunks_mut(PAR_CHUNK).zip(idx.par_chunks(PAR_CHUNK)).for_each(|(o, ix)| {
        for (a, i) in o.iter_mut().zip(ix) {
            *a = src[*i];
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_and_gathers_match_sequential_loops() {
        let src: Vec<f64> = (0..200_000).map(|i| i as f64 * 0.37).collect();
        let m = par_map_f64(&src, |x| (x * 0.1).exp());
        assert!(m.iter().zip(&src).all(|(a, b)| a.to_bits() == (b * 0.1).exp().to_bits()));
        let idx: Vec<usize> = (0..src.len()).rev().collect();
        let g = par_gather_f64(&src, &idx);
        assert!(g.iter().zip(idx.iter()).all(|(a, i)| *a == src[*i]));
        let z = zeroed_f64(3_000_000);
        assert!(z.iter().all(|v| *v == 0.0));
        let _u = zeroed_u32(10);
        let _c: Vec<u8> = with_capacity(10 << 20);
    }
}
