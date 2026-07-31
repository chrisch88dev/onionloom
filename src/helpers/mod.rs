//! Shared command parsing and output helpers.

use std::sync::Arc;

use anyhow::Result;

use onionloom::gpu::start_search as start_gpu_search;
use onionloom::performance::eta::{Estimates, format_duration};
use onionloom::search::filter::{FilterSet, Pattern};

use crate::cpu::PowerMode;

const AUTO_GPU_MIN_DIFFICULTY: f64 = 10_000_000.0;
const AUTO_GPU_MIN_CPU_EXPECTED_SECONDS: f64 = 5.0;
const AUTO_GPU_MIN_CPU_RATE_FRACTION: f64 = 0.05;

pub(crate) struct GpuEstimate {
    pub(crate) device_name: String,
    pub(crate) attempts_per_second: f64,
}

pub(crate) fn automatic_gpu_is_worth_starting(difficulty: f64, cpu_rate: f64) -> bool {
    difficulty >= AUTO_GPU_MIN_DIFFICULTY
        && difficulty / cpu_rate >= AUTO_GPU_MIN_CPU_EXPECTED_SECONDS
}

pub(crate) fn automatic_gpu_rate_is_useful(cpu_rate: f64, gpu_rate: f64) -> bool {
    gpu_rate >= cpu_rate * AUTO_GPU_MIN_CPU_RATE_FRACTION
}

pub(crate) fn measure_available_gpu(filters: Arc<FilterSet>) -> Option<GpuEstimate> {
    if !onionloom::gpu::compiled() {
        return None;
    }
    let session = start_gpu_search(filters).ok()?;
    let measurement = GpuEstimate {
        device_name: session.device_name().to_owned(),
        attempts_per_second: session.measured_rate(),
    };
    session.stop_and_join().ok()?;
    Some(measurement)
}

pub(crate) fn parse_filters(patterns: &[String]) -> Result<Arc<FilterSet>> {
    let parsed = patterns
        .iter()
        .map(|pattern| Pattern::parse(pattern))
        .collect::<Result<Vec<_>>>()?;
    Ok(Arc::new(FilterSet::new(parsed)?))
}

pub(crate) fn format_rate(value: f64) -> String {
    if value >= 1_000_000_000.0 {
        format!(
            "{:.2} billion possible addresses/second",
            value / 1_000_000_000.0
        )
    } else if value >= 1_000_000.0 {
        format!(
            "{:.2} million possible addresses/second",
            value / 1_000_000.0
        )
    } else if value >= 1_000.0 {
        format!("{:.2} thousand possible addresses/second", value / 1_000.0)
    } else {
        format!("{value:.0} possible addresses/second")
    }
}

pub(crate) fn format_count(value: f64) -> String {
    if !value.is_finite() || value > u64::MAX as f64 {
        return format!("{value:.3e}");
    }
    let digits = format!("{:.0}", value.max(0.0));
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    let first_group = digits.len() % 3;
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && index % 3 == first_group {
            grouped.push(',');
        }
        grouped.push(character);
    }
    grouped
}

pub(crate) fn print_power_mode(power: PowerMode, available: usize) {
    let limit = power.worker_limit(available);
    println!(
        "Power mode: {} (up to {limit} parallel search tasks; {})",
        power.label(),
        power.explanation()
    );
}

pub(crate) fn print_power_mode_stderr(power: PowerMode, available: usize) {
    let limit = power.worker_limit(available);
    eprintln!(
        "Power mode: {} (up to {limit} parallel search tasks; {})",
        power.label(),
        power.explanation()
    );
}

pub(crate) fn print_time_estimates(estimate: Estimates) {
    println!("Time estimate");
    println!("  Average time: {}", format_duration(estimate.expected));
    println!("  50% chance by: {}", format_duration(estimate.median));
    println!("  95% chance by: {}", format_duration(estimate.p95));
}

pub(crate) fn print_time_estimates_stderr(estimate: Estimates) {
    eprintln!("Time estimate for the next match");
    eprintln!("  Average time: {}", format_duration(estimate.expected));
    eprintln!("  50% chance by: {}", format_duration(estimate.median));
    eprintln!("  95% chance by: {}", format_duration(estimate.p95));
    eprintln!("These are probability estimates, not deadlines.");
}

#[cfg(test)]
mod tests {
    use super::{automatic_gpu_is_worth_starting, automatic_gpu_rate_is_useful, format_count};

    #[test]
    fn counts_are_grouped_for_readability() {
        assert_eq!(format_count(1_048_576.0), "1,048,576");
        assert_eq!(format_count(32.0), "32");
    }

    #[test]
    fn automatic_gpu_policy_requires_enough_work_to_repay_startup() {
        assert!(!automatic_gpu_is_worth_starting(33_554_432.0, 80_000_000.0));
        assert!(automatic_gpu_is_worth_starting(
            1_073_741_824.0,
            80_000_000.0
        ));
        assert!(!automatic_gpu_rate_is_useful(80_000_000.0, 3_000_000.0));
        assert!(automatic_gpu_rate_is_useful(80_000_000.0, 4_000_000.0));
    }
}
