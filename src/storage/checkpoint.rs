//! Permission-restricted resumable search checkpoints.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::crypto::keys::ExpandedSecret;

/// Current checkpoint schema version.
const VERSION: u32 = 2;

/// Persistent state needed to resume worker ranges.
#[derive(Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    version: u32,
    patterns: Vec<String>,
    workers: usize,
    batch_size: usize,
    base_secret: Vec<u8>,
    next_offsets: Vec<u64>,
}

impl Checkpoint {
    /// Creates a checkpoint snapshot.
    #[must_use]
    pub fn new(
        patterns: Vec<String>,
        workers: usize,
        batch_size: usize,
        base_secret: &ExpandedSecret,
        next_offsets: Vec<u64>,
    ) -> Self {
        Self {
            version: VERSION,
            patterns,
            workers,
            batch_size,
            base_secret: base_secret.expose_bytes().to_vec(),
            next_offsets,
        }
    }

    /// Returns the normalized patterns.
    #[must_use]
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Returns the saved worker count.
    #[must_use]
    pub fn workers(&self) -> usize {
        self.workers
    }

    /// Returns the saved batch size.
    #[must_use]
    pub fn batch_size(&self) -> usize {
        self.batch_size
    }

    /// Reconstructs the protected base secret.
    pub fn base_secret(&self) -> Result<ExpandedSecret> {
        let bytes: [u8; 64] = self
            .base_secret
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("checkpoint base secret has the wrong length"))?;
        ExpandedSecret::from_bytes(bytes)
    }

    /// Returns the next offset for each worker.
    #[must_use]
    pub fn next_offsets(&self) -> &[u64] {
        &self.next_offsets
    }

    /// Validates structural invariants.
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == VERSION, "unsupported checkpoint version");
        ensure!(self.workers > 0, "checkpoint worker count is zero");
        ensure!(
            self.next_offsets.len() == self.workers,
            "checkpoint worker offset count does not match worker count"
        );
        ensure!(
            crate::search::cpu::BatchSize::parse(self.batch_size).is_ok(),
            "checkpoint batch size is unsupported"
        );
        self.base_secret()?;
        Ok(())
    }
}

impl Drop for Checkpoint {
    fn drop(&mut self) {
        self.base_secret.zeroize();
    }
}

/// Loads and validates a checkpoint.
pub fn load(path: &Path) -> Result<Checkpoint> {
    let input =
        fs::read(path).with_context(|| format!("failed to read checkpoint {}", path.display()))?;
    let checkpoint: Checkpoint = serde_json::from_slice(&input)
        .with_context(|| format!("failed to parse checkpoint {}", path.display()))?;
    checkpoint.validate()?;
    Ok(checkpoint)
}

/// Atomically saves a checkpoint where the platform supports replacement rename.
pub fn save(path: &Path, checkpoint: &Checkpoint) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create checkpoint directory {}", parent.display()))?;
    let temporary = temporary_path(path);
    let bytes = serde_json::to_vec(checkpoint)?;
    write_private_file(&temporary, &bytes)?;

    if let Err(error) = fs::rename(&temporary, path) {
        if path.exists() {
            fs::remove_file(path)
                .with_context(|| format!("failed to replace checkpoint {}", path.display()))?;
            fs::rename(&temporary, path)
                .with_context(|| format!("failed to install checkpoint {}", path.display()))?;
        } else {
            return Err(error)
                .with_context(|| format!("failed to install checkpoint {}", path.display()));
        }
    }
    Ok(())
}

fn temporary_path(path: &Path) -> PathBuf {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(temporary)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if path.exists() {
        fs::remove_file(path)
            .with_context(|| format!("failed to remove stale file {}", path.display()))?;
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
