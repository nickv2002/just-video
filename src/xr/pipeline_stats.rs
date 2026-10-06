//! What the read and decode side did over a few seconds, for the `Timing:
//! frames …` line: the decode thread and the frame loop share one
//! [`PipelineStats`] of atomics (no locks, nothing allocated per picture) and
//! the report takes a [`PipelineWindow`] from it.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::Duration;

pub struct PipelineStats {
    pictures: AtomicU64,
    /// Time per picture in the decoder, reads not counted (ns).
    decode_ns: AtomicU64,
    decode_max_ns: AtomicU64,
    /// Time the demuxer waited for the network (µs), and the longest wait.
    wait_us: AtomicU64,
    wait_max_us: AtomicU64,
    bytes: AtomicU64,
    /// Decoded pictures in the queue to the frame loop now.
    queued: AtomicUsize,
    depth_min: AtomicUsize,
    depth_sum: AtomicU64,
    depth_samples: AtomicU64,
    /// Copies of a picture out of the driver's buffer (see `Frame::detach`).
    detached: AtomicU64,
    detach_ns: AtomicU64,
    detach_max_ns: AtomicU64,
}

impl Default for PipelineStats {
    fn default() -> Self {
        Self {
            pictures: AtomicU64::new(0),
            decode_ns: AtomicU64::new(0),
            decode_max_ns: AtomicU64::new(0),
            wait_us: AtomicU64::new(0),
            wait_max_us: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
            queued: AtomicUsize::new(0),
            depth_min: AtomicUsize::new(usize::MAX),
            depth_sum: AtomicU64::new(0),
            depth_samples: AtomicU64::new(0),
            detached: AtomicU64::new(0),
            detach_ns: AtomicU64::new(0),
            detach_max_ns: AtomicU64::new(0),
        }
    }
}

impl PipelineStats {
    /// A picture came out of the decoder after `elapsed` in the call,
    /// `waited` of it blocked on reads.
    pub fn picture(&self, elapsed: Duration, waited: Duration) {
        let ns = elapsed.saturating_sub(waited).as_nanos() as u64;
        self.pictures.fetch_add(1, Relaxed);
        self.decode_ns.fetch_add(ns, Relaxed);
        self.decode_max_ns.fetch_max(ns, Relaxed);
    }

    /// A picture was copied out of the driver's buffer in `elapsed`.
    pub fn detach(&self, elapsed: Duration) {
        let ns = elapsed.as_nanos() as u64;
        self.detached.fetch_add(1, Relaxed);
        self.detach_ns.fetch_add(ns, Relaxed);
        self.detach_max_ns.fetch_max(ns, Relaxed);
    }

    /// The demuxer read `bytes`, waiting `wait_us` in total; its longest
    /// single wait since the last call was `longest_us`.
    pub fn io(&self, bytes: u64, wait_us: u64, longest_us: u64) {
        self.bytes.fetch_add(bytes, Relaxed);
        self.wait_us.fetch_add(wait_us, Relaxed);
        self.wait_max_us.fetch_max(longest_us, Relaxed);
    }

    /// A message went into the queue to the frame loop…
    pub fn enqueued(&self) {
        self.queued.fetch_add(1, Relaxed);
    }

    /// …and one came out.
    pub fn dequeued(&self) {
        let _ = self
            .queued
            .fetch_update(Relaxed, Relaxed, |n| n.checked_sub(1));
    }

    /// The frame loop looks at the queue.
    pub fn sample_depth(&self) {
        let depth = self.queued.load(Relaxed);
        self.depth_min.fetch_min(depth, Relaxed);
        self.depth_sum.fetch_add(depth as u64, Relaxed);
        self.depth_samples.fetch_add(1, Relaxed);
    }

    /// What happened since the last call (which starts the next window).
    pub fn take(&self) -> PipelineWindow {
        let samples = self.depth_samples.swap(0, Relaxed);
        let depth_sum = self.depth_sum.swap(0, Relaxed);
        let depth_min = self.depth_min.swap(usize::MAX, Relaxed);
        PipelineWindow {
            pictures: self.pictures.swap(0, Relaxed),
            decode_ns: self.decode_ns.swap(0, Relaxed),
            decode_max_ns: self.decode_max_ns.swap(0, Relaxed),
            wait_us: self.wait_us.swap(0, Relaxed),
            wait_max_us: self.wait_max_us.swap(0, Relaxed),
            bytes: self.bytes.swap(0, Relaxed),
            detached: self.detached.swap(0, Relaxed),
            detach_ns: self.detach_ns.swap(0, Relaxed),
            detach_max_ns: self.detach_max_ns.swap(0, Relaxed),
            depth_min: (samples > 0).then_some(depth_min),
            depth_mean: (samples > 0).then(|| depth_sum as f64 / samples as f64),
            reads: crate::read_metrics::playback_metrics().map(|m| m.take()),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct PipelineWindow {
    pub pictures: u64,
    decode_ns: u64,
    decode_max_ns: u64,
    pub wait_us: u64,
    pub wait_max_us: u64,
    pub bytes: u64,
    detached: u64,
    detach_ns: u64,
    detach_max_ns: u64,
    pub depth_min: Option<usize>,
    pub depth_mean: Option<f64>,
    /// The playing video's read-ahead buffer, if there is one.
    pub reads: Option<crate::read_metrics::ReadWindow>,
}

impl PipelineWindow {
    pub fn decode_mean_ms(&self) -> f64 {
        if self.pictures == 0 {
            0.0
        } else {
            self.decode_ns as f64 / self.pictures as f64 / 1e6
        }
    }

    pub fn decode_max_ms(&self) -> f64 {
        self.decode_max_ns as f64 / 1e6
    }
}

impl std::fmt::Display for PipelineWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} pictures decoded, decode {:.1}/{:.1} ms, read {:.1} MiB, \
             read wait {:.0} ms total / {:.0} ms longest, queue depth min ",
            self.pictures,
            self.decode_mean_ms(),
            self.decode_max_ms(),
            self.bytes as f64 / (1 << 20) as f64,
            self.wait_us as f64 / 1e3,
            self.wait_max_us as f64 / 1e3,
        )?;
        match (self.depth_min, self.depth_mean) {
            (Some(min), Some(mean)) => write!(f, "{min} mean {mean:.1}")?,
            _ => write!(f, "-")?,
        }
        if self.detached > 0 {
            write!(
                f,
                ", {} pictures copied out of the driver's buffers in {:.1}/{:.1} ms",
                self.detached,
                self.detach_ns as f64 / self.detached as f64 / 1e6,
                self.detach_max_ns as f64 / 1e6,
            )?;
        }
        if let Some(reads) = &self.reads {
            write!(f, ", {reads}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn decode_time_leaves_out_the_wait_for_reads() {
        let s = PipelineStats::default();
        s.picture(30 * MS, 10 * MS);
        s.picture(50 * MS, Duration::ZERO);
        // A wait longer than the call (clock skew between two counters) is not negative.
        s.picture(5 * MS, 9 * MS);
        let w = s.take();
        assert_eq!(w.pictures, 3);
        assert!((w.decode_mean_ms() - 70.0 / 3.0).abs() < 1e-9);
        assert!((w.decode_max_ms() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn io_sums_waits_and_keeps_the_longest() {
        let s = PipelineStats::default();
        s.io(1 << 20, 4_000, 3_000);
        s.io(2 << 20, 90_000, 80_000);
        s.io(0, 0, 0);
        let w = s.take();
        assert_eq!(
            (w.bytes, w.wait_us, w.wait_max_us),
            (3 << 20, 94_000, 80_000)
        );
    }

    #[test]
    fn queue_depth_tracks_sends_and_receives() {
        let s = PipelineStats::default();
        s.sample_depth(); // 0
        s.enqueued();
        s.enqueued();
        s.sample_depth(); // 2
        s.dequeued();
        s.sample_depth(); // 1
        s.dequeued();
        s.dequeued(); // more receives than sends never wraps
        s.sample_depth(); // 0
        let w = s.take();
        assert_eq!(w.depth_min, Some(0));
        assert_eq!(w.depth_mean, Some(0.75));
    }

    #[test]
    fn a_window_starts_over_and_an_empty_one_prints_dashes() {
        let s = PipelineStats::default();
        s.picture(MS, MS);
        s.sample_depth();
        s.take();
        let w = s.take();
        assert_eq!(w.pictures, 0);
        assert_eq!((w.depth_min, w.depth_mean), (None, None));
        assert_eq!(w.decode_mean_ms(), 0.0);
        assert!(w.to_string().ends_with("queue depth min -"));
    }

    #[test]
    fn the_line_names_every_figure() {
        let s = PipelineStats::default();
        s.picture(20 * MS, Duration::ZERO);
        s.io(2 << 20, 5_000, 5_000);
        s.sample_depth();
        assert_eq!(
            s.take().to_string(),
            "1 pictures decoded, decode 20.0/20.0 ms, read 2.0 MiB, \
             read wait 5 ms total / 5 ms longest, queue depth min 0 mean 0.0"
        );
    }
}
