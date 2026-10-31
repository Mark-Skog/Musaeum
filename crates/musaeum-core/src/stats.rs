//! Measurements collected by the render pool.

use serde::Serialize;

/// Samples kept per metric. Enough for many minutes of rendering at full speed, and it
/// bounds memory if a session stays open for days.
const MAX_SAMPLES: usize = 1 << 20;

/// Distribution of a metric, in the unit of its samples.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// Number of samples.
    pub count: u64,
    /// Median.
    pub p50: u32,
    /// 95th percentile.
    pub p95: u32,
    /// 99th percentile.
    pub p99: u32,
    /// Largest sample.
    pub max: u32,
}

impl Summary {
    /// Summarizes samples using the nearest-rank percentile definition.
    pub fn of(samples: &[u32]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let rank = |percent: usize| {
            let index = (percent * sorted.len()).div_ceil(100);
            sorted[index.saturating_sub(1)]
        };
        Self {
            count: sorted.len() as u64,
            p50: rank(50),
            p95: rank(95),
            p99: rank(99),
            max: sorted[sorted.len() - 1],
        }
    }
}

/// A snapshot of everything the pool has measured.
#[derive(Clone, Debug, Default, Serialize)]
pub struct PoolStats {
    /// Valid demand lists received.
    pub demands: u64,
    /// Demand lists rejected by validation.
    pub invalid_demands: u64,
    /// Tiles rendered to completion.
    pub tiles_done: u64,
    /// Cancellations asked of workers because a job fell off the demand list.
    pub cancels_requested: u64,
    /// Jobs a worker stopped before finishing, because of a cancellation.
    pub tiles_cancelled: u64,
    /// Results finished by a worker before its cancellation took effect, then thrown away.
    pub results_discarded: u64,
    /// Jobs that failed.
    pub tiles_failed: u64,
    /// Workers replaced after dying.
    pub worker_restarts: u32,
    /// Workers killed for sending a report that contradicts the host's state.
    pub protocol_violations: u32,
    /// Microseconds to map and open the document, in the first worker.
    pub open_us: u64,
    /// Microseconds to read every page's size without rasterizing anything.
    pub geometry_us: u64,
    /// Microseconds per finished tile, including page load.
    pub render_us: Summary,
    /// The page-load portion of `render_us`.
    pub load_us: Summary,
    /// Microseconds from requesting a cancellation to the worker confirming it.
    pub cancel_latency_us: Summary,
    /// Microseconds to copy a batch out of the slab (Path A only).
    pub batch_copy_us: Summary,
}

/// Collects raw samples on the pool's thread.
#[derive(Debug, Default)]
pub(crate) struct StatsCollector {
    pub(crate) totals: PoolStats,
    render_us: Vec<u32>,
    load_us: Vec<u32>,
    cancel_latency_us: Vec<u32>,
    batch_copy_us: Vec<u32>,
}

fn record(samples: &mut Vec<u32>, value: u32) {
    if samples.len() < MAX_SAMPLES {
        samples.push(value);
    }
}

impl StatsCollector {
    pub(crate) fn tile_done(&mut self, render_us: u32, load_us: u32) {
        self.totals.tiles_done += 1;
        record(&mut self.render_us, render_us);
        record(&mut self.load_us, load_us);
    }

    pub(crate) fn cancel_confirmed(&mut self, latency_us: u32) {
        record(&mut self.cancel_latency_us, latency_us);
    }

    pub(crate) fn batch_copied(&mut self, micros: u32) {
        record(&mut self.batch_copy_us, micros);
    }

    pub(crate) fn snapshot(&self) -> PoolStats {
        PoolStats {
            render_us: Summary::of(&self.render_us),
            load_us: Summary::of(&self.load_us),
            cancel_latency_us: Summary::of(&self.cancel_latency_us),
            batch_copy_us: Summary::of(&self.batch_copy_us),
            ..self.totals.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_gives_zeroes() {
        assert_eq!(Summary::of(&[]), Summary::default());
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let samples: Vec<u32> = (1..=100).rev().collect();
        assert_eq!(
            Summary::of(&samples),
            Summary {
                count: 100,
                p50: 50,
                p95: 95,
                p99: 99,
                max: 100
            }
        );
        assert_eq!(Summary::of(&[7]).p99, 7);
    }

    #[test]
    fn sample_storage_is_bounded() {
        let mut collector = StatsCollector::default();
        for _ in 0..MAX_SAMPLES + 10 {
            collector.tile_done(1, 1);
        }
        assert_eq!(collector.render_us.len(), MAX_SAMPLES);
        assert_eq!(collector.totals.tiles_done, MAX_SAMPLES as u64 + 10);
    }
}
