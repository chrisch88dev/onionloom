//! Batched multi-threaded Ed25519 vanity search engine.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use anyhow::{Result, bail, ensure};
use curve25519_dalek::{EdwardsPoint, Scalar};

use crate::filter::FilterSet;
use crate::keys::ExpandedSecret;

/// Supported compile-time batch sizes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchSize {
    /// 64 candidates.
    N64,
    /// 128 candidates.
    N128,
    /// 256 candidates.
    N256,
    /// 512 candidates.
    N512,
    /// 1024 candidates.
    N1024,
}

impl BatchSize {
    /// All sizes considered by automatic tuning.
    pub const ALL: [Self; 5] = [Self::N64, Self::N128, Self::N256, Self::N512, Self::N1024];

    /// Parses a supported size.
    pub fn parse(value: usize) -> Result<Self> {
        match value {
            64 => Ok(Self::N64),
            128 => Ok(Self::N128),
            256 => Ok(Self::N256),
            512 => Ok(Self::N512),
            1024 => Ok(Self::N1024),
            _ => bail!("batch size must be one of 64, 128, 256, 512, or 1024"),
        }
    }

    /// Returns the numeric size.
    #[must_use]
    pub const fn get(self) -> usize {
        match self {
            Self::N64 => 64,
            Self::N128 => 128,
            Self::N256 => 256,
            Self::N512 => 512,
            Self::N1024 => 1024,
        }
    }
}

/// A fully verified candidate returned by a worker.
pub struct FoundKey {
    /// Expanded secret key.
    pub secret: ExpandedSecret,
    /// Compressed public key.
    pub public: [u8; 32],
    /// Offset from the random base scalar.
    pub offset: u64,
}

/// Live state for a running search.
pub struct SearchSession {
    /// Match receiver.
    pub matches: mpsc::Receiver<FoundKey>,
    /// Cooperative stop flag.
    pub stop: Arc<AtomicBool>,
    attempts: Arc<AtomicU64>,
    next_offsets: Arc<Vec<AtomicU64>>,
    handles: Vec<JoinHandle<()>>,
}

impl SearchSession {
    /// Returns the completed candidate count.
    #[must_use]
    pub fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::Relaxed)
    }

    /// Takes a consistent-enough checkpoint snapshot.
    #[must_use]
    pub fn next_offsets(&self) -> Vec<u64> {
        self.next_offsets
            .iter()
            .map(|offset| offset.load(Ordering::Relaxed))
            .collect()
    }

    /// Requests shutdown and joins every worker.
    pub fn stop_and_join(mut self) -> Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        for handle in self.handles.drain(..) {
            ensure!(handle.join().is_ok(), "search worker panicked");
        }
        Ok(())
    }
}

/// Starts disjoint worker ranges.
pub fn start_search(
    secret: &ExpandedSecret,
    filters: Arc<FilterSet>,
    workers: usize,
    batch_size: BatchSize,
    resume_offsets: Option<Vec<u64>>,
) -> Result<SearchSession> {
    ensure!(workers > 0, "worker count must be greater than zero");
    let size = batch_size.get();
    let initial_offsets = resume_offsets.unwrap_or_else(|| {
        (0..workers)
            .map(|worker| u64::try_from(worker * size).expect("worker offset fits u64"))
            .collect()
    });
    ensure!(
        initial_offsets.len() == workers,
        "resume offset count must match worker count"
    );

    let next_offsets = Arc::new(
        initial_offsets
            .iter()
            .copied()
            .map(AtomicU64::new)
            .collect::<Vec<_>>(),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let attempts = Arc::new(AtomicU64::new(0));
    let (sender, matches) = mpsc::channel();
    let stride = u64::try_from(workers)
        .and_then(|count| u64::try_from(size).map(|size| count.saturating_mul(size)))
        .expect("worker stride fits u64");

    let mut handles = Vec::with_capacity(workers);
    for (worker, offset) in initial_offsets.iter().copied().enumerate() {
        let worker_secret = secret.clone();
        let worker_filters = Arc::clone(&filters);
        let worker_stop = Arc::clone(&stop);
        let worker_attempts = Arc::clone(&attempts);
        let worker_offsets = Arc::clone(&next_offsets);
        let worker_sender = sender.clone();
        handles.push(
            thread::Builder::new()
                .name(format!("onionloom-{worker}"))
                .spawn(move || {
                    dispatch_search(
                        batch_size,
                        worker_secret,
                        worker_filters,
                        worker_stop,
                        worker_attempts,
                        worker_offsets,
                        worker,
                        offset,
                        stride,
                        worker_sender,
                    );
                })?,
        );
    }
    drop(sender);

    Ok(SearchSession {
        matches,
        stop,
        attempts,
        next_offsets,
        handles,
    })
}

#[allow(clippy::too_many_arguments)]
fn dispatch_search(
    batch_size: BatchSize,
    secret: ExpandedSecret,
    filters: Arc<FilterSet>,
    stop: Arc<AtomicBool>,
    attempts: Arc<AtomicU64>,
    offsets: Arc<Vec<AtomicU64>>,
    worker: usize,
    offset: u64,
    stride: u64,
    sender: mpsc::Sender<FoundKey>,
) {
    macro_rules! run {
        ($size:literal) => {
            search_worker::<$size>(
                &secret, &filters, &stop, &attempts, &offsets, worker, offset, stride, &sender,
            )
        };
    }
    match batch_size {
        BatchSize::N64 => run!(64),
        BatchSize::N128 => run!(128),
        BatchSize::N256 => run!(256),
        BatchSize::N512 => run!(512),
        BatchSize::N1024 => run!(1024),
    }
}

#[allow(clippy::too_many_arguments)]
fn search_worker<const N: usize>(
    secret: &ExpandedSecret,
    filters: &FilterSet,
    stop: &AtomicBool,
    attempts: &AtomicU64,
    offsets: &[AtomicU64],
    worker: usize,
    mut offset: u64,
    stride: u64,
    sender: &mpsc::Sender<FoundKey>,
) {
    let step = EdwardsPoint::mul_base(&Scalar::from(8_u64));
    let stride_point = EdwardsPoint::mul_base(&(Scalar::from(stride) * Scalar::from(8_u64)));
    let mut first = secret.point_at_offset(offset);

    while !stop.load(Ordering::Relaxed) {
        offsets[worker].store(offset, Ordering::Relaxed);
        let mut points = [EdwardsPoint::default(); N];
        let mut point = first;
        for candidate in &mut points {
            *candidate = point;
            point += step;
        }
        let compressed = EdwardsPoint::compress_batch(&points);
        for (index, public) in compressed.iter().enumerate() {
            let public = public.to_bytes();
            if filters.matches(&public) {
                let candidate_offset = offset
                    .checked_add(u64::try_from(index).expect("batch index fits u64"))
                    .expect("candidate offset overflow");
                if let Ok(candidate_secret) = secret.with_offset(candidate_offset)
                    && candidate_secret.public_key() == public
                    && sender
                        .send(FoundKey {
                            secret: candidate_secret,
                            public,
                            offset: candidate_offset,
                        })
                        .is_err()
                {
                    return;
                }
            }
        }
        attempts.fetch_add(
            u64::try_from(N).expect("batch size fits u64"),
            Ordering::Relaxed,
        );
        let Some(next) = offset.checked_add(stride) else {
            stop.store(true, Ordering::Relaxed);
            return;
        };
        offset = next;
        offsets[worker].store(offset, Ordering::Relaxed);
        first += stride_point;
    }
}

/// Runs point generation and batch compression until stopped.
pub(crate) fn benchmark_worker(
    batch_size: BatchSize,
    stop: &AtomicBool,
    attempts: &AtomicU64,
    seed: u64,
    filters: Option<&FilterSet>,
) {
    macro_rules! run {
        ($size:literal) => {
            benchmark_worker_const::<$size>(stop, attempts, seed, filters)
        };
    }
    match batch_size {
        BatchSize::N64 => run!(64),
        BatchSize::N128 => run!(128),
        BatchSize::N256 => run!(256),
        BatchSize::N512 => run!(512),
        BatchSize::N1024 => run!(1024),
    }
}

fn benchmark_worker_const<const N: usize>(
    stop: &AtomicBool,
    attempts: &AtomicU64,
    seed: u64,
    filters: Option<&FilterSet>,
) {
    let step = EdwardsPoint::mul_base(&Scalar::from(8_u64));
    let mut point = EdwardsPoint::mul_base(&Scalar::from(seed.saturating_add(1)));
    let mut local = 0_u64;
    while !stop.load(Ordering::Relaxed) {
        let mut points = [EdwardsPoint::default(); N];
        for candidate in &mut points {
            *candidate = point;
            point += step;
        }
        let compressed = EdwardsPoint::compress_batch(&points);
        if let Some(filters) = filters {
            for public in &compressed {
                std::hint::black_box(filters.matches(public.as_bytes()));
            }
        } else {
            std::hint::black_box(compressed);
        }
        local = local.saturating_add(u64::try_from(N).expect("batch size fits u64"));
    }
    attempts.fetch_add(local, Ordering::Relaxed);
}
