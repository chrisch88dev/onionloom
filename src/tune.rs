//! Measured CPU auto-tuning.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, ensure};

use crate::engine::{BatchSize, benchmark_worker};
use crate::filter::FilterSet;

/// One measured configuration.
#[derive(Clone, Copy, Debug)]
pub struct Measurement {
    /// Worker threads.
    pub workers: usize,
    /// Candidate batch size.
    pub batch_size: BatchSize,
    /// Completed candidates per wall-clock second.
    pub attempts_per_second: f64,
    /// Actual measurement duration.
    pub elapsed: Duration,
    /// Completed candidate count.
    pub attempts: u64,
}

/// Results selected by the tuner.
#[derive(Debug)]
pub struct TuneResult {
    /// Selected configuration.
    pub selected: Measurement,
    /// Every batch-size measurement.
    pub batch_measurements: Vec<Measurement>,
    /// Every worker-count measurement.
    pub worker_measurements: Vec<Measurement>,
}

/// Benchmarks batch sizes and every worker count up to the requested limit.
pub fn auto_tune(
    per_trial: Duration,
    filters: Option<Arc<FilterSet>>,
    max_workers: usize,
) -> Result<TuneResult> {
    ensure!(
        per_trial >= Duration::from_millis(25),
        "tuning trial must be at least 25 ms"
    );
    ensure!(
        max_workers > 0,
        "automatic tuning needs at least one worker"
    );
    let logical = std::thread::available_parallelism()?.get();
    let worker_limit = logical.min(max_workers);

    let mut batch_measurements = Vec::with_capacity(BatchSize::ALL.len());
    for batch_size in BatchSize::ALL {
        batch_measurements.push(measure_inner(1, batch_size, per_trial, filters.clone())?);
    }
    let best_batch = batch_measurements
        .iter()
        .max_by(|left, right| {
            left.attempts_per_second
                .total_cmp(&right.attempts_per_second)
        })
        .expect("batch sizes are non-empty")
        .batch_size;

    let mut worker_measurements = Vec::with_capacity(worker_limit);
    for workers in 1..=worker_limit {
        worker_measurements.push(measure_inner(
            workers,
            best_batch,
            per_trial,
            filters.clone(),
        )?);
    }
    let selected = *worker_measurements
        .iter()
        .max_by(|left, right| {
            left.attempts_per_second
                .total_cmp(&right.attempts_per_second)
        })
        .expect("worker measurements are non-empty");

    Ok(TuneResult {
        selected,
        batch_measurements,
        worker_measurements,
    })
}

/// Measures one concrete configuration.
pub fn measure(workers: usize, batch_size: BatchSize, duration: Duration) -> Result<Measurement> {
    measure_inner(workers, batch_size, duration, None)
}

/// Measures one configuration with the real compiled pattern filter.
pub fn measure_filtered(
    workers: usize,
    batch_size: BatchSize,
    duration: Duration,
    filters: Arc<FilterSet>,
) -> Result<Measurement> {
    measure_inner(workers, batch_size, duration, Some(filters))
}

fn measure_inner(
    workers: usize,
    batch_size: BatchSize,
    duration: Duration,
    filters: Option<Arc<FilterSet>>,
) -> Result<Measurement> {
    ensure!(workers > 0, "worker count must be greater than zero");
    let stop = Arc::new(AtomicBool::new(false));
    let attempts = Arc::new(AtomicU64::new(0));
    let barrier = Arc::new(std::sync::Barrier::new(workers + 1));
    let mut handles = Vec::with_capacity(workers);
    for worker in 0..workers {
        let worker_stop = Arc::clone(&stop);
        let worker_attempts = Arc::clone(&attempts);
        let worker_barrier = Arc::clone(&barrier);
        let worker_filters = filters.clone();
        handles.push(thread::spawn(move || {
            worker_barrier.wait();
            benchmark_worker(
                batch_size,
                &worker_stop,
                &worker_attempts,
                u64::try_from(worker).expect("worker index fits u64"),
                worker_filters.as_deref(),
            );
        }));
    }
    barrier.wait();
    let started = Instant::now();
    thread::sleep(duration);
    stop.store(true, Ordering::Relaxed);
    for handle in handles {
        ensure!(handle.join().is_ok(), "benchmark worker panicked");
    }
    let elapsed = started.elapsed();
    let attempts = attempts.load(Ordering::Relaxed);
    Ok(Measurement {
        workers,
        batch_size,
        attempts_per_second: attempts as f64 / elapsed.as_secs_f64(),
        elapsed,
        attempts,
    })
}
