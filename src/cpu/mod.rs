//! CPU discovery and user-facing power controls.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::Result;
use clap::ValueEnum;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum PowerMode {
    /// Leave most CPU capacity available for other work.
    Low,
    /// Balance search speed with a responsive computer.
    Medium,
    /// Use every available CPU processing slot without changing clock or voltage settings.
    Full,
}

impl PowerMode {
    pub(crate) const DEFAULT: Self = Self::Full;

    pub(crate) fn worker_limit(self, available: usize) -> usize {
        match self {
            Self::Low => available.div_ceil(4),
            Self::Medium => available.div_ceil(2),
            Self::Full => available,
        }
        .max(1)
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::Full => "full",
        }
    }

    pub(crate) const fn explanation(self) -> &'static str {
        match self {
            Self::Low => "leaves most CPU capacity for other work",
            Self::Medium => "balances search speed and responsiveness",
            Self::Full => "allows every available processing slot without overclocking",
        }
    }
}

#[derive(Debug)]
pub(crate) struct HardwareInfo {
    pub(crate) available_processors: usize,
    model: Option<String>,
    physical_cores: Option<usize>,
    maximum_clock_mhz: Option<u64>,
    cache: Option<String>,
    kernel: Option<String>,
    features: Vec<&'static str>,
}

impl HardwareInfo {
    pub(crate) fn detect() -> Result<Self> {
        let available_processors = std::thread::available_parallelism()?.get();
        let cpuinfo = fs::read_to_string("/proc/cpuinfo").ok();
        Ok(Self {
            available_processors,
            model: cpuinfo.as_deref().and_then(cpu_model),
            physical_cores: cpuinfo.as_deref().and_then(physical_core_count),
            maximum_clock_mhz: maximum_clock_mhz(),
            cache: cpuinfo.as_deref().and_then(cpu_cache),
            kernel: fs::read_to_string("/proc/sys/kernel/osrelease")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            features: cpuinfo.as_deref().map(cpu_features).unwrap_or_default(),
        })
    }

    pub(crate) fn print(&self) {
        println!("Hardware");
        if let Some(model) = self.model.as_deref() {
            println!("  CPU: {model}");
        }
        println!("  Architecture: {}", std::env::consts::ARCH);
        if let Some(physical_cores) = self.physical_cores {
            println!("  Physical CPU cores: {physical_cores}");
        }
        println!(
            "  CPU processing slots available to Onionloom: {}",
            self.available_processors
        );
        if let Some(maximum_clock_mhz) = self.maximum_clock_mhz {
            println!(
                "  Highest reported CPU clock: {}",
                format_clock(maximum_clock_mhz)
            );
        }
        if let Some(cache) = self.cache.as_deref() {
            println!("  Reported CPU cache: {cache}");
        }
        match self.kernel.as_deref() {
            Some(kernel) => println!("  System: {} {kernel}", os_label()),
            None => println!("  System: {}", os_label()),
        }
        if !self.features.is_empty() {
            println!("  Detected CPU features: {}", self.features.join(", "));
        }
    }
}

fn cpu_model(cpuinfo: &str) -> Option<String> {
    ["model name", "Hardware", "Processor"]
        .iter()
        .find_map(|name| first_field(cpuinfo, name))
}

fn cpu_cache(cpuinfo: &str) -> Option<String> {
    let reported = first_field(cpuinfo, "cache size")?;
    let (amount, unit) = reported.split_once(' ')?;
    let amount = amount.parse::<u64>().ok()?;
    if unit.eq_ignore_ascii_case("KB") && amount % 1_024 == 0 {
        Some(format!("{} MiB", amount / 1_024))
    } else {
        Some(reported)
    }
}

fn first_field(cpuinfo: &str, wanted: &str) -> Option<String> {
    cpuinfo.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == wanted)
            .then(|| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn physical_core_count(cpuinfo: &str) -> Option<usize> {
    let mut cores = BTreeSet::new();
    for processor in cpuinfo.split("\n\n") {
        let physical_id = first_field(processor, "physical id");
        let core_id = first_field(processor, "core id");
        if let (Some(physical_id), Some(core_id)) = (physical_id, core_id) {
            cores.insert((physical_id, core_id));
        }
    }
    if !cores.is_empty() {
        return Some(cores.len());
    }
    first_field(cpuinfo, "cpu cores")?.parse().ok()
}

fn maximum_clock_mhz() -> Option<u64> {
    let root = Path::new("/sys/devices/system/cpu/cpufreq");
    let mut maximum_khz = None;
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path().join("cpuinfo_max_freq");
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(value) = contents.trim().parse::<u64>() else {
            continue;
        };
        maximum_khz = Some(maximum_khz.map_or(value, |current: u64| current.max(value)));
    }
    maximum_khz.map(|value| value / 1_000)
}

fn cpu_features(cpuinfo: &str) -> Vec<&'static str> {
    let flags = first_field(cpuinfo, "flags")
        .or_else(|| first_field(cpuinfo, "Features"))
        .unwrap_or_default();
    let flags: BTreeSet<_> = flags.split_whitespace().collect();
    [
        ("avx2", "AVX2"),
        ("avx512f", "AVX-512"),
        ("bmi2", "BMI2"),
        ("sha_ni", "SHA instructions"),
        ("asimd", "NEON"),
        ("sha3", "SHA3 instructions"),
    ]
    .iter()
    .filter_map(|(flag, label)| flags.contains(flag).then_some(*label))
    .collect()
}

fn format_clock(mhz: u64) -> String {
    if mhz >= 1_000 {
        format!("{:.2} GHz", mhz as f64 / 1_000.0)
    } else {
        format!("{mhz} MHz")
    }
}

fn os_label() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "macOS",
        "windows" => "Windows",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{PowerMode, cpu_cache, cpu_features, cpu_model, format_clock, physical_core_count};

    const SAMPLE: &str = "\
processor : 0
physical id : 0
core id : 0
model name : Example CPU
flags : sse avx2 bmi2

processor : 1
physical id : 0
core id : 0
model name : Example CPU
flags : sse avx2 bmi2

processor : 2
physical id : 0
core id : 1
model name : Example CPU
flags : sse avx2 bmi2
";

    #[test]
    fn parses_linux_cpu_information() {
        assert_eq!(cpu_model(SAMPLE).as_deref(), Some("Example CPU"));
        assert_eq!(physical_core_count(SAMPLE), Some(2));
        assert_eq!(cpu_features(SAMPLE), ["AVX2", "BMI2"]);
        assert_eq!(format_clock(4_400), "4.40 GHz");
        assert_eq!(cpu_cache("cache size : 12288 KB"), Some("12 MiB".into()));
        assert_eq!(PowerMode::DEFAULT, PowerMode::Full);
    }
}
