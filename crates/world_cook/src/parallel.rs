//! Order-preserving work on every core for pure cook stages. Callers read inputs and write
//! results on their own thread, so SQLite access and output order match a sequential pass.
use std::sync::atomic::{AtomicUsize, Ordering};

/// Workers take the next index from a shared counter, so uneven items (land and open sea
/// cells, for example) balance without chunking.
pub(crate) fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(items.len());
    if threads <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let mut results: Vec<(usize, R)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(index) else {
                            return done;
                        };
                        done.push((index, f(item)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("cook worker panicked"))
            .collect()
    });
    results.sort_unstable_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn results_keep_input_order() {
        let items: Vec<u64> = (0..1000).collect();
        let squares = super::map(&items, |x| {
            // Uneven work, so completion order differs from input order.
            std::thread::sleep(std::time::Duration::from_micros(x % 7));
            x * x
        });
        assert_eq!(squares, items.iter().map(|x| x * x).collect::<Vec<_>>());
        assert!(super::map(&[] as &[u64], |x| *x).is_empty());
    }
}
