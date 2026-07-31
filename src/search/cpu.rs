//! Batched multi-threaded Ed25519 vanity search engine.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use anyhow::{Result, bail, ensure};

use crate::crypto::keys::ExpandedSecret;
use crate::search::filter::FilterSet;
use crate::search::symmetry::{OffsetTable, SymmetryEngine};

/// Supported paired-coordinate batch sizes.
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
    /// 2048 paired candidates.
    N2048,
    /// 4096 paired candidates.
    N4096,
    /// 8192 paired candidates.
    N8192,
}

impl BatchSize {
    /// All sizes considered by automatic tuning.
    pub const ALL: [Self; 8] = [
        Self::N64,
        Self::N128,
        Self::N256,
        Self::N512,
        Self::N1024,
        Self::N2048,
        Self::N4096,
        Self::N8192,
    ];

    /// Parses a supported size.
    pub fn parse(value: usize) -> Result<Self> {
        match value {
            64 => Ok(Self::N64),
            128 => Ok(Self::N128),
            256 => Ok(Self::N256),
            512 => Ok(Self::N512),
            1024 => Ok(Self::N1024),
            2048 => Ok(Self::N2048),
            4096 => Ok(Self::N4096),
            8192 => Ok(Self::N8192),
            _ => bail!("batch size must be one of 64, 128, 256, 512, 1024, 2048, 4096, or 8192"),
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
            Self::N2048 => 2048,
            Self::N4096 => 4096,
            Self::N8192 => 8192,
        }
    }

    /// Returns the paired candidates plus their center point.
    #[must_use]
    pub const fn candidates(self) -> usize {
        self.get() + 1
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
    dropped_matches: Arc<AtomicU64>,
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
    pub fn stop_and_join(self) -> Result<()> {
        self.stop_and_join_with_dropped().map(|_| ())
    }

    /// Requests shutdown, joins every worker, and returns discarded match candidates.
    pub fn stop_and_join_with_dropped(mut self) -> Result<u64> {
        self.stop.store(true, Ordering::Relaxed);
        for handle in self.handles.drain(..) {
            ensure!(handle.join().is_ok(), "search worker panicked");
        }
        Ok(self.dropped_matches.load(Ordering::Relaxed))
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
    let table = OffsetTable::new(batch_size.get())?;
    let size = batch_size.candidates();
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
    let dropped_matches = Arc::new(AtomicU64::new(0));
    let queue_capacity = workers.saturating_mul(2).max(1);
    let (sender, matches) = mpsc::sync_channel(queue_capacity);
    let stride = u64::try_from(workers)
        .and_then(|count| u64::try_from(size).map(|size| count.saturating_mul(size)))
        .expect("worker stride fits u64");

    let mut handles = Vec::with_capacity(workers);
    for (worker, offset) in initial_offsets.iter().copied().enumerate() {
        let worker_secret = secret.clone();
        let worker_filters = Arc::clone(&filters);
        let worker_stop = Arc::clone(&stop);
        let worker_attempts = Arc::clone(&attempts);
        let worker_dropped_matches = Arc::clone(&dropped_matches);
        let worker_offsets = Arc::clone(&next_offsets);
        let worker_sender = sender.clone();
        let worker_table = Arc::clone(&table);
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
                        worker_dropped_matches,
                        worker_offsets,
                        worker,
                        offset,
                        stride,
                        worker_sender,
                        worker_table,
                    );
                })?,
        );
    }
    drop(sender);

    Ok(SearchSession {
        matches,
        stop,
        attempts,
        dropped_matches,
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
    dropped_matches: Arc<AtomicU64>,
    offsets: Arc<Vec<AtomicU64>>,
    worker: usize,
    offset: u64,
    stride: u64,
    sender: mpsc::SyncSender<FoundKey>,
    table: Arc<OffsetTable>,
) {
    debug_assert_eq!(batch_size.get(), table.batch_size());
    search_worker(
        &secret,
        &filters,
        &stop,
        &attempts,
        &dropped_matches,
        &offsets,
        worker,
        offset,
        stride,
        &sender,
        table,
    );
}

#[allow(clippy::too_many_arguments)]
fn search_worker(
    secret: &ExpandedSecret,
    filters: &FilterSet,
    stop: &AtomicBool,
    attempts: &AtomicU64,
    dropped_matches: &AtomicU64,
    offsets: &[AtomicU64],
    worker: usize,
    offset: u64,
    stride: u64,
    sender: &mpsc::SyncSender<FoundKey>,
    table: Arc<OffsetTable>,
) {
    let expected_stride = u64::try_from(offsets.len())
        .and_then(|workers| {
            u64::try_from(table.batch_size() + 1)
                .map(|candidates| workers.saturating_mul(candidates))
        })
        .expect("worker stride fits u64");
    debug_assert_eq!(stride, expected_stride);
    let Ok(mut engine) = SymmetryEngine::new(secret, table, offsets.len(), worker, offset) else {
        stop.store(true, Ordering::Relaxed);
        return;
    };

    while !stop.load(Ordering::Relaxed) {
        offsets[worker].store(engine.block_start(), Ordering::Relaxed);
        let mut disconnected = false;
        let generated = engine.generate(|public_y, candidate_offset| {
            if !disconnected && filters.matches_y_coordinate(&public_y) {
                let Ok(candidate_secret) = secret.with_offset(candidate_offset) else {
                    return;
                };
                let public = candidate_secret.public_key();
                if filters.matches(&public) {
                    match sender.try_send(FoundKey {
                        secret: candidate_secret,
                        public,
                        offset: candidate_offset,
                    }) {
                        Ok(()) => {}
                        Err(mpsc::TrySendError::Full(_)) => {
                            dropped_matches.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => {
                            disconnected = true;
                        }
                    }
                }
            }
        });
        let Ok(generated) = generated else {
            stop.store(true, Ordering::Relaxed);
            return;
        };
        if disconnected {
            return;
        }
        attempts.fetch_add(
            u64::try_from(generated).expect("batch size fits u64"),
            Ordering::Relaxed,
        );
        offsets[worker].store(engine.block_start(), Ordering::Relaxed);
    }
}

/// Runs paired-coordinate generation and filtering until stopped.
pub(crate) fn benchmark_worker(
    stop: &AtomicBool,
    attempts: &AtomicU64,
    seed: u64,
    filters: Option<&FilterSet>,
    table: Arc<OffsetTable>,
) {
    let mut bytes = [0_u8; 64];
    bytes[0] = 0x18;
    bytes[31] = 0x40;
    bytes[8..16].copy_from_slice(&seed.to_le_bytes());
    let secret = ExpandedSecret::from_bytes(bytes).expect("fixed benchmark scalar is valid");
    let mut engine =
        SymmetryEngine::new(&secret, table, 1, 0, 0).expect("benchmark engine initializes");
    let mut local = 0_u64;
    while !stop.load(Ordering::Relaxed) {
        let generated = engine
            .benchmark_block(filters)
            .expect("benchmark symmetry arithmetic remains valid");
        local = local.saturating_add(u64::try_from(generated).expect("batch size fits u64"));
    }
    attempts.fetch_add(local, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use crate::crypto::address::BASE32_ALPHABET;
    use crate::crypto::keys::ExpandedSecret;
    use crate::search::filter::{FilterSet, Pattern};

    use super::{BatchSize, start_search};

    #[test]
    fn bounded_match_queue_drops_excess_candidates() {
        let patterns = BASE32_ALPHABET
            .iter()
            .map(|character| {
                let bytes = [*character];
                let value = std::str::from_utf8(&bytes).expect("base32 character");
                Pattern::parse(value).expect("valid base32 prefix")
            })
            .collect();
        let filters = Arc::new(FilterSet::new(patterns).expect("filter set"));
        let secret = ExpandedSecret::generate().expect("operating-system randomness");
        let session =
            start_search(&secret, filters, 1, BatchSize::N64, None).expect("search session");

        let deadline = Instant::now() + Duration::from_secs(1);
        while session.dropped_matches.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
            std::thread::yield_now();
        }

        assert!(
            session.stop_and_join_with_dropped().expect("workers join") > 0,
            "a full queue must drop candidates instead of retaining unbounded secrets"
        );
    }
}
