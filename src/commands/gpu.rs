//! GPU availability command.

#[cfg(feature = "gpu")]
use std::sync::Arc;

use anyhow::Result;
use clap::Args;
#[cfg(feature = "gpu")]
use onionloom::crypto::keys::ExpandedSecret;
#[cfg(feature = "gpu")]
use onionloom::gpu::GpuEngine;
#[cfg(feature = "gpu")]
use onionloom::search::filter::{FilterSet, Pattern};

#[cfg(feature = "gpu")]
use crate::helpers::{format_count, format_rate};

#[derive(Debug, Args)]
pub(crate) struct GpuArgs {}

#[cfg_attr(not(feature = "gpu"), allow(clippy::unnecessary_wraps))]
pub(crate) fn run(_: GpuArgs) -> Result<()> {
    println!("Onionloom GPU status");
    println!();
    if !onionloom::gpu::compiled() {
        println!("GPU engine: not included in this binary");
        println!("Build the optimized Linux binary with `./build.sh` to include it.");
        return Ok(());
    }

    let devices = onionloom::gpu::devices();
    if devices.is_empty() {
        println!("GPU engine: included, but no usable hardware GPU was found");
        println!("CPU search remains available.");
        return Ok(());
    }

    println!("GPU engine: included");
    println!("Detected hardware");
    for (index, device) in devices.iter().enumerate() {
        let kind = if device.discrete {
            "discrete GPU"
        } else {
            "integrated GPU"
        };
        println!(
            "  {}. {} ({kind}, {}, vendor {:04x}, device {:04x})",
            index + 1,
            device.name,
            device.backend,
            device.vendor_id,
            device.device_id
        );
    }
    #[cfg(feature = "gpu")]
    run_measurement()?;
    Ok(())
}

#[cfg(feature = "gpu")]
fn run_measurement() -> Result<()> {
    println!("Validating exact GPU arithmetic against the CPU...");
    onionloom::gpu::self_test()?;
    println!("GPU arithmetic validation passed.");
    println!("Every GPU match is reconstructed and verified on the CPU before saving.");
    println!();
    println!("Measuring complete GPU candidate generation...");
    println!("  Workgroups  Lanes   Candidates  Seconds  Rate");
    let secret = ExpandedSecret::generate()?;
    let pattern = Pattern::parse("zzzzzzzzzzzzzzzz")?;
    let filters = Arc::new(FilterSet::new(vec![pattern])?);
    let mut best = None;
    for workgroups in [10, 20, 32, 40, 64] {
        let mut engine = GpuEngine::with_workgroups(&secret, Arc::clone(&filters), workgroups)?;
        let measurement = engine.benchmark()?;
        println!(
            "  {workgroups:>10}  {:>5}  {:>11}  {:>7.3}  {}",
            engine.lane_count(),
            format_count(measurement.attempts as f64),
            measurement.duration.as_secs_f64(),
            format_rate(measurement.attempts_per_second)
        );
        if best.as_ref().is_none_or(
            |(_, _, best_measurement): &(_, _, onionloom::gpu::GpuMeasurement)| {
                measurement.attempts_per_second > best_measurement.attempts_per_second
            },
        ) {
            best = Some((engine.device_name().to_owned(), workgroups, measurement));
        }
    }
    let (device_name, workgroups, measurement) = best.expect("GPU workgroup cases are not empty");
    println!("Selected GPU: {device_name}");
    println!("Fastest measured workgroups: {workgroups}");
    println!("Rate confirmation: median of three complete timed samples");
    println!(
        "Selected end-to-end GPU rate: {}",
        format_rate(measurement.attempts_per_second)
    );
    Ok(())
}
