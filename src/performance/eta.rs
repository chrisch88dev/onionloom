//! Probability and ETA calculations for geometric search.

use std::time::Duration;

/// Time estimates for the next match.
#[derive(Clone, Copy, Debug)]
pub struct Estimates {
    /// Mean time until the next match.
    pub expected: Duration,
    /// Median time until the next match.
    pub median: Duration,
    /// Time by which 95 percent of searches find a match.
    pub p95: Duration,
}

/// Calculates geometric-distribution estimates from measured throughput.
#[must_use]
pub fn estimates(probability: f64, attempts_per_second: f64) -> Estimates {
    let denominator = attempts_per_second.max(f64::MIN_POSITIVE);
    let log_failure = (-probability).ln_1p();
    let expected_attempts = 1.0 / probability;
    let median_attempts = 0.5_f64.ln() / log_failure;
    let p95_attempts = 0.05_f64.ln() / log_failure;
    Estimates {
        expected: seconds(expected_attempts / denominator),
        median: seconds(median_attempts / denominator),
        p95: seconds(p95_attempts / denominator),
    }
}

fn seconds(value: f64) -> Duration {
    if !value.is_finite() || value >= Duration::MAX.as_secs_f64() {
        Duration::MAX
    } else {
        Duration::from_secs_f64(value.max(0.0))
    }
}

/// Formats a duration for compact terminal output.
#[must_use]
pub fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs_f64();
    if duration == Duration::MAX {
        return "over platform duration limit".to_owned();
    }
    if seconds < 0.001 {
        return format!("{:.0} us", seconds * 1_000_000.0);
    }
    if seconds < 1.0 {
        let milliseconds = seconds * 1_000.0;
        if milliseconds < 10.0 {
            return format!("{milliseconds:.2} ms");
        }
        if milliseconds < 100.0 {
            return format!("{milliseconds:.1} ms");
        }
        return format!("{milliseconds:.0} ms");
    }
    if seconds < 60.0 {
        return format!("{seconds:.1} s");
    }
    if seconds < 3_600.0 {
        return format!("{:.1} min", seconds / 60.0);
    }
    if seconds < 86_400.0 {
        return format!("{:.1} h", seconds / 3_600.0);
    }
    if seconds < 31_557_600.0 {
        return format!("{:.1} d", seconds / 86_400.0);
    }
    format!("{:.1} y", seconds / 31_557_600.0)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{estimates, format_duration};

    #[test]
    fn geometric_quantiles_are_ordered() {
        let estimate = estimates(1.0 / 32.0_f64.powi(5), 1_000_000.0);
        assert!(estimate.median < estimate.expected);
        assert!(estimate.expected < estimate.p95);
    }

    #[test]
    fn short_times_do_not_round_to_zero() {
        assert_eq!(format_duration(Duration::from_micros(4)), "4 us");
        assert_eq!(format_duration(Duration::from_micros(4_250)), "4.25 ms");
        assert_eq!(format_duration(Duration::from_millis(42)), "42.0 ms");
    }
}
