//! Synchronous parallelism for independent raster rows. Workers only receive
//! immutable captured inputs and a disjoint output slice; the caller joins
//! them before returning to device code.
use std::ops::Range;

#[cfg(not(target_arch = "wasm32"))]
use std::{mem::size_of, thread};

#[cfg(not(target_arch = "wasm32"))]
const MAX_WORKERS: usize = 2;
#[cfg(not(target_arch = "wasm32"))]
const MIN_PARALLEL_BYTES: usize = 6 * 1024;

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn rows<T: Send>(
    values: &mut [T],
    row_length: usize,
    row_count: usize,
    requested_workers: usize,
    compute: impl Fn(Range<usize>, &mut [T]) + Sync,
) -> Result<(), &'static str> {
    assert!(row_length > 0 && values.len() == row_length * row_count);
    let workers = requested_workers.min(MAX_WORKERS).min(row_count);
    let total_bytes = values.len().saturating_mul(size_of::<T>());
    if workers <= 1 || total_bytes < MIN_PARALLEL_BYTES {
        compute(0..row_count, values);
        return Ok(());
    }

    let rows_per_chunk = row_count.div_ceil(workers);
    let first_length = rows_per_chunk * row_length;
    let (first, remaining) = values.split_at_mut(first_length);
    thread::scope(|scope| {
        let mut jobs = Vec::with_capacity(workers - 1);
        for (index, chunk) in remaining
            .chunks_mut(rows_per_chunk * row_length)
            .enumerate()
        {
            let start = (index + 1) * rows_per_chunk;
            let end = start + chunk.len() / row_length;
            let compute = &compute;
            jobs.push(scope.spawn(move || compute(start..end, chunk)));
        }
        compute(0..first.len() / row_length, first);
        let mut failed = false;
        for job in jobs {
            failed |= job.join().is_err();
        }
        if failed {
            Err("raster worker panicked")
        } else {
            Ok(())
        }
    })
}

#[cfg(target_arch = "wasm32")]
pub(super) fn rows<T>(
    values: &mut [T],
    row_length: usize,
    row_count: usize,
    _requested_workers: usize,
    compute: impl Fn(Range<usize>, &mut [T]),
) -> Result<(), &'static str> {
    assert!(row_length > 0 && values.len() == row_length * row_count);
    compute(0..row_count, values);
    Ok(())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::rows;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn bounded_rows_cover_each_output_once_and_preserve_global_coordinates() {
        let mut output = vec![0usize; 4096 * 2];
        let visited = AtomicUsize::new(0);
        rows(&mut output, 4096, 2, 8, |range, chunk| {
            visited.fetch_add(range.len(), Ordering::Relaxed);
            for (local, value) in chunk.iter_mut().enumerate() {
                *value = range.start * 4096 + local;
            }
        })
        .unwrap();
        assert_eq!(visited.load(Ordering::Relaxed), 2);
        assert_eq!(output, (0..8192).collect::<Vec<_>>());
    }

    #[test]
    fn zero_workers_uses_the_same_serial_row_coordinates() {
        let mut output = vec![0usize; 32];
        rows(&mut output, 8, 4, 0, |range, chunk| {
            for (local, value) in chunk.iter_mut().enumerate() {
                *value = range.start * 8 + local;
            }
        })
        .unwrap();
        assert_eq!(output, (0..32).collect::<Vec<_>>());
    }
}
