//! Optional GPU discovery and search acceleration.

#[cfg(feature = "gpu")]
mod backend;
#[cfg(feature = "gpu")]
mod engine;

#[cfg(feature = "gpu")]
pub use engine::{GpuEngine, GpuMeasurement, GpuSearchSession, self_test, start_search};

#[cfg(not(feature = "gpu"))]
use crate::search::cpu::FoundKey;
#[cfg(not(feature = "gpu"))]
use crate::search::filter::FilterSet;
#[cfg(not(feature = "gpu"))]
use std::sync::Arc;
#[cfg(not(feature = "gpu"))]
use std::sync::atomic::AtomicBool;

/// Information about one detected GPU compute adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceInfo {
    /// Human-readable adapter name.
    pub name: String,
    /// Graphics API backend.
    pub backend: String,
    /// Device vendor identifier.
    pub vendor_id: u32,
    /// Device identifier assigned by the vendor.
    pub device_id: u32,
    /// Whether this is a discrete GPU.
    pub discrete: bool,
}

/// Returns whether this binary contains the GPU backend.
#[must_use]
pub const fn compiled() -> bool {
    cfg!(feature = "gpu")
}

/// Enumerates usable hardware GPU adapters.
#[must_use]
pub fn devices() -> Vec<DeviceInfo> {
    #[cfg(feature = "gpu")]
    {
        backend::devices()
    }
    #[cfg(not(feature = "gpu"))]
    {
        Vec::new()
    }
}

/// Runs an exact GPU arithmetic test when acceleration is compiled in.
#[cfg(not(feature = "gpu"))]
pub fn self_test() -> anyhow::Result<()> {
    anyhow::bail!("this binary was built without GPU acceleration")
}

/// Placeholder session type for CPU-only binaries.
#[cfg(not(feature = "gpu"))]
pub struct GpuSearchSession;

#[cfg(not(feature = "gpu"))]
impl GpuSearchSession {
    /// Returns zero because no GPU worker can run in this build.
    #[must_use]
    pub const fn attempts(&self) -> u64 {
        0
    }

    /// Returns zero because no GPU was measured.
    #[must_use]
    pub const fn measured_rate(&self) -> f64 {
        0.0
    }

    /// Returns a generic CPU-only device description.
    #[must_use]
    pub const fn device_name(&self) -> &'static str {
        "no GPU"
    }

    /// Returns an unused stop flag.
    #[must_use]
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    /// Returns the CPU-only build error.
    pub fn try_match(&self) -> anyhow::Result<Option<FoundKey>> {
        anyhow::bail!("this binary was built without GPU acceleration")
    }

    /// Returns the CPU-only build error.
    pub fn stop_and_join(self) -> anyhow::Result<u64> {
        anyhow::bail!("this binary was built without GPU acceleration")
    }
}

/// Returns an error in a CPU-only binary.
#[cfg(not(feature = "gpu"))]
pub fn start_search(_: Arc<FilterSet>) -> anyhow::Result<GpuSearchSession> {
    anyhow::bail!("this binary was built without GPU acceleration")
}
