//! What the read-ahead buffer did over a few seconds, for the `Timing: frames
//! …` line: request latency, requests in flight and data buffered ahead,
//! counted with atomics (no locks, nothing allocated on the read path).

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering::Relaxed},
};
use std::time::Duration;

/// Four buckets per doubling of the time in microseconds, up to 2^30 µs.
const BUCKETS: usize = 31 * 4;
const MIB: f64 = (1 << 20) as f64;

pub struct ReadMetrics {
    /// Request-to-completion times, log-scaled (see [`bucket_of`]).
    latency: [AtomicU64; BUCKETS],
    latency_max_us: AtomicU64,
    /// Running average of request times (µs), outliers held back: what a
    /// request normally takes right now.
    typical_us: AtomicU64,
    /// Blocks asked for a second time (see `ReadAheadReader::with_hedge`).
    hedged: AtomicU64,
    in_flight_sum: AtomicU64,
    in_flight_samples: AtomicU64,
    /// Finished bytes waiting ahead of the reader, sampled at every read.
    ahead_min: AtomicU64,
    ahead_sum: AtomicU64,
    ahead_samples: AtomicU64,
    /// Bytes the buffer aims to hold ahead right now.
    target: AtomicU64,
    /// The stream's bytes per second as far as known (0: not yet).
    rate_hint: AtomicU64,
}

fn bucket_of(us: u64) -> usize {
    let us = us.max(1);
    let octave = 63 - us.leading_zeros() as usize;
    let sub = if octave >= 2 {
        ((us >> (octave - 2)) & 3) as usize
    } else {
        0
    };
    (octave * 4 + sub).min(BUCKETS - 1)
}

/// The largest time (µs) that falls in `bucket`.
fn bucket_top(bucket: usize) -> u64 {
    let (octave, sub) = (bucket / 4, bucket % 4);
    if octave < 2 {
        return (1 << (octave + 1)) - 1;
    }
    ((5 + sub as u64) << (octave - 2)) - 1
}

impl Default for ReadMetrics {
    fn default() -> Self {
        Self {
            latency: std::array::from_fn(|_| AtomicU64::new(0)),
            latency_max_us: AtomicU64::new(0),
            typical_us: AtomicU64::new(0),
            hedged: AtomicU64::new(0),
            in_flight_sum: AtomicU64::new(0),
            in_flight_samples: AtomicU64::new(0),
            ahead_min: AtomicU64::new(u64::MAX),
            ahead_sum: AtomicU64::new(0),
            ahead_samples: AtomicU64::new(0),
            target: AtomicU64::new(0),
            rate_hint: AtomicU64::new(0),
        }
    }
}

impl ReadMetrics {
    /// A request finished `took` after it was made.
    pub(crate) fn request_done(&self, took: Duration) {
        let us = took.as_micros().min(u64::MAX as u128) as u64;
        self.latency[bucket_of(us)].fetch_add(1, Relaxed);
        self.latency_max_us.fetch_max(us, Relaxed);
        // An eighth of each new time, but a stall counts for at most 3x the
        // average, so one stall does not make slow look normal.
        let old = self.typical_us.load(Relaxed);
        let new = if old == 0 {
            us
        } else {
            old - old / 8 + us.min(old * 3) / 8
        };
        self.typical_us.store(new, Relaxed);
    }

    pub(crate) fn note_hedge(&self) {
        self.hedged.fetch_add(1, Relaxed);
    }

    /// What a request normally takes lately (zero before the first).
    pub(crate) fn typical_latency(&self) -> Duration {
        Duration::from_micros(self.typical_us.load(Relaxed))
    }

    /// The reader looks at its buffer: `in_flight` requests out, `ahead`
    /// finished bytes waiting, `target` bytes wanted.
    pub(crate) fn sample(&self, in_flight: usize, ahead: u64, target: u64) {
        self.in_flight_sum.fetch_add(in_flight as u64, Relaxed);
        self.in_flight_samples.fetch_add(1, Relaxed);
        self.ahead_min.fetch_min(ahead, Relaxed);
        self.ahead_sum.fetch_add(ahead, Relaxed);
        self.ahead_samples.fetch_add(1, Relaxed);
        self.target.store(target, Relaxed);
    }

    /// Tells the reader how fast the stream plays (bytes per second of
    /// media: file size over duration), which sizes its buffer.
    pub fn set_rate_hint(&self, bytes_per_second: u64) {
        self.rate_hint.store(bytes_per_second, Relaxed);
    }

    pub(crate) fn rate_hint(&self) -> u64 {
        self.rate_hint.load(Relaxed)
    }

    /// What happened since the last call (which starts the next window).
    pub fn take(&self) -> ReadWindow {
        let counts: Vec<u64> = self.latency.iter().map(|c| c.swap(0, Relaxed)).collect();
        let requests: u64 = counts.iter().sum();
        let max_ms = self.latency_max_us.swap(0, Relaxed) as f64 / 1e3;
        let percentile = |p: f64| -> f64 {
            if requests == 0 {
                return 0.0;
            }
            let wanted = ((requests as f64 * p).ceil() as u64).clamp(1, requests);
            let mut seen = 0;
            for (bucket, &n) in counts.iter().enumerate() {
                seen += n;
                if seen >= wanted {
                    // A bucket's top can lie above the largest value in it.
                    return (bucket_top(bucket) as f64 / 1e3).min(max_ms);
                }
            }
            max_ms
        };
        let samples = self.in_flight_samples.swap(0, Relaxed);
        let in_flight_sum = self.in_flight_sum.swap(0, Relaxed);
        let ahead_samples = self.ahead_samples.swap(0, Relaxed);
        let ahead_sum = self.ahead_sum.swap(0, Relaxed);
        let ahead_min = self.ahead_min.swap(u64::MAX, Relaxed);
        ReadWindow {
            requests,
            p50_ms: percentile(0.5),
            p99_ms: percentile(0.99),
            max_ms,
            in_flight_mean: (samples > 0).then(|| in_flight_sum as f64 / samples as f64),
            ahead_min_mib: (ahead_samples > 0).then(|| ahead_min as f64 / MIB),
            ahead_mean_mib: (ahead_samples > 0)
                .then(|| ahead_sum as f64 / ahead_samples as f64 / MIB),
            target_mib: self.target.load(Relaxed) as f64 / MIB,
            hedged: self.hedged.swap(0, Relaxed),
        }
    }
}

/// One window of [`ReadMetrics`].
#[derive(Debug, PartialEq)]
pub struct ReadWindow {
    pub requests: u64,
    pub p50_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
    pub in_flight_mean: Option<f64>,
    pub ahead_min_mib: Option<f64>,
    pub ahead_mean_mib: Option<f64>,
    pub target_mib: f64,
    /// Blocks asked for again because the first request was slow.
    pub hedged: u64,
}

impl std::fmt::Display for ReadWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} requests took {:.0}/{:.0}/{:.0} ms (p50/p99/max), in flight mean ",
            self.requests, self.p50_ms, self.p99_ms, self.max_ms
        )?;
        match self.in_flight_mean {
            Some(n) => write!(f, "{n:.1}")?,
            None => write!(f, "-")?,
        }
        write!(f, ", buffered ahead min ")?;
        match (self.ahead_min_mib, self.ahead_mean_mib) {
            (Some(min), Some(mean)) => write!(f, "{min:.0} mean {mean:.0}")?,
            _ => write!(f, "-")?,
        }
        write!(f, " MiB of {:.0} aimed", self.target_mib)?;
        if self.hedged > 0 {
            write!(f, ", {} slow blocks asked again", self.hedged)?;
        }
        Ok(())
    }
}

/// The metrics of the video now playing, for the timing log: the reader that
/// calls `ReadAheadReader::for_playback` registers itself here.
static PLAYBACK: Mutex<Option<Arc<ReadMetrics>>> = Mutex::new(None);

/// Metrics of the reader opened last for playback, while it is open.
pub fn playback_metrics() -> Option<Arc<ReadMetrics>> {
    PLAYBACK.lock().ok()?.clone()
}

pub(crate) fn register_playback(metrics: &Arc<ReadMetrics>) {
    if let Ok(mut slot) = PLAYBACK.lock() {
        *slot = Some(metrics.clone());
    }
}

/// The reader holding `metrics` closed: forget it unless a newer one took over.
pub(crate) fn unregister_playback(metrics: &Arc<ReadMetrics>) {
    if let Ok(mut slot) = PLAYBACK.lock()
        && slot.as_ref().is_some_and(|m| Arc::ptr_eq(m, metrics))
    {
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_cover_their_values_within_a_quarter_octave() {
        for us in [1u64, 2, 3, 7, 100, 1_000, 12_345, 1_000_000, 900_000_000] {
            let b = bucket_of(us);
            assert!(bucket_top(b) >= us, "{us} above top of bucket {b}");
            if b > 0 {
                assert!(bucket_top(b - 1) < us, "{us} fits an earlier bucket");
            }
            assert!(bucket_top(b) as f64 <= us as f64 * 1.26 + 4.0, "{us}");
        }
    }

    #[test]
    fn typical_latency_follows_the_link_but_not_single_stalls() {
        let m = ReadMetrics::default();
        assert_eq!(m.typical_latency(), Duration::ZERO);
        for _ in 0..40 {
            m.request_done(Duration::from_millis(60));
        }
        m.request_done(Duration::from_millis(5000));
        let after_stall = m.typical_latency();
        assert!(after_stall < Duration::from_millis(100), "{after_stall:?}");
        // A link that stays slow does show.
        for _ in 0..60 {
            m.request_done(Duration::from_millis(400));
        }
        assert!(m.typical_latency() > Duration::from_millis(300));
    }

    #[test]
    fn percentiles_and_max() {
        let m = ReadMetrics::default();
        for _ in 0..98 {
            m.request_done(Duration::from_millis(10));
        }
        m.request_done(Duration::from_millis(400));
        m.request_done(Duration::from_millis(1000));
        let w = m.take();
        assert_eq!(w.requests, 100);
        assert!((10.0..13.0).contains(&w.p50_ms), "{}", w.p50_ms);
        assert!((10.0..13.0).contains(&w.p99_ms) || w.p99_ms >= 400.0);
        assert_eq!(w.max_ms, 1000.0);
        // The next window starts over.
        assert_eq!(m.take().requests, 0);
    }

    #[test]
    fn samples_average_and_a_window_prints_every_figure() {
        let m = ReadMetrics::default();
        m.request_done(Duration::from_millis(20));
        m.sample(4, 8 << 20, 32 << 20);
        m.sample(6, 16 << 20, 32 << 20);
        assert_eq!(
            m.take().to_string(),
            "1 requests took 20/20/20 ms (p50/p99/max), in flight mean 5.0, \
             buffered ahead min 8 mean 12 MiB of 32 aimed"
        );
        assert!(m.take().to_string().contains("in flight mean -"));
    }
}
