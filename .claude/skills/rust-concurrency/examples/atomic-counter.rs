use std::sync::atomic::{AtomicU64, Ordering};

pub struct AtomicCounter {
    count: AtomicU64,
}

impl AtomicCounter {
    pub fn new() -> Self {
        Self {
            count: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn increment(&self) -> u64 {
        self.count.fetch_add(1, Ordering::Relaxed)
    }

    #[inline]
    pub fn get(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn add(&self, value: u64) -> u64 {
        self.count.fetch_add(value, Ordering::Relaxed)
    }

    #[inline]
    pub fn compare_and_swap(&self, current: u64, new: u64) -> u64 {
        self.count.compare_exchange(
            current,
            new,
            Ordering::SeqCst,
            Ordering::Relaxed,
        ).unwrap_or_else(|x| x)
    }
}

// Benchmark function
pub fn benchmark_atomic_counter() {
    use std::thread;
    use std::time::Instant;

    let counter = AtomicCounter::new();
    let start = Instant::now();

    let handles: Vec<_> = (0..8)
        .map(|_| {
            let counter = &counter;
            thread::spawn(move || {
                for _ in 0..1_000_000 {
                    counter.increment();
                }
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap();
    }

    let duration = start.elapsed();
    println!("Final count: {}, Duration: {:?}", counter.get(), duration);
    println!("Ops/sec: {}", 8_000_000.0 / duration.as_secs_f64());
}