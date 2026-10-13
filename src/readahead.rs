//! Blocking `Read + Seek` with pipelined read-ahead over any async block source.
//!
//! The demuxer reads synchronously; behind it, a window of large block reads is
//! kept in flight so network latency overlaps decode instead of adding to it.

use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    io::{self, Read, Seek, SeekFrom},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{runtime::Runtime, task::JoinHandle};

use crate::read_metrics::{self, ReadMetrics};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Random-access byte source, e.g. an open SMB file.
pub trait BlockSource: Send + Sync + 'static {
    /// Reads exactly `len` bytes at `offset` (the reader never asks past the end).
    fn fetch(self: Arc<Self>, offset: u64, len: usize) -> BoxFuture<'static, io::Result<Vec<u8>>>;

    /// Independent paths (connections) requests are spread over now. A read
    /// left waiting can be asked again only if there is more than one.
    fn lanes(&self) -> usize {
        1
    }

    /// Releases the remote resource; called once when the reader drops.
    fn close(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

/// Read-ahead policy. Defaults target high-bitrate 8K VR over Wi-Fi: 1 MiB
/// reads (within every SMB2.1+ server's max read size) with a 32 MiB window.
#[derive(Clone, Copy, Debug)]
pub struct ReadAhead {
    pub block_size: usize,
    pub blocks_ahead: usize,
}

impl Default for ReadAhead {
    fn default() -> Self {
        Self {
            block_size: 1 << 20,
            blocks_ahead: 32,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct ReadStats {
    pub bytes_delivered: u64,
    pub bytes_fetched: u64,
    pub requests: u64,
    /// Time the demuxer spent blocked waiting on the network.
    pub stall_seconds: f64,
    pub discarded_blocks: u64,
    /// Blocks asked for a second time because the first request was slow.
    pub hedged: u64,
}

/// How the buffer sizes itself (see [`ReadAheadReader::with_adaptive`]).
#[derive(Clone, Copy, Debug)]
pub struct Adaptive {
    /// Seconds of the stream to keep buffered ahead.
    pub seconds: f64,
    /// The buffer never aims below or above this many bytes.
    pub min_bytes: usize,
    pub max_bytes: usize,
    /// Most reads in flight at a time once the buffer has grown.
    pub in_flight: usize,
}

impl Default for Adaptive {
    fn default() -> Self {
        Self {
            seconds: ADAPTIVE_SECONDS,
            min_bytes: 16 << 20,
            max_bytes: ADAPTIVE_MAX_BYTES,
            in_flight: ADAPTIVE_IN_FLIGHT,
        }
    }
}

/// Seconds ahead to buffer: Wi-Fi stalls of 1-3 s were seen on the headset,
/// and 10 s ride out several in a row while the link refills the buffer.
const ADAPTIVE_SECONDS: f64 = 10.0;
/// Memory cap of the buffer ahead (8K60 HEVC at ~13 MiB/s: ~130 MiB).
const ADAPTIVE_MAX_BYTES: usize = 160 << 20;
/// Same as `MAX_IN_FLIGHT`: over the headset's link 3, 4, 6 and 8 in flight
/// carried the same (medians 201, 217, 202, 209 Mbit/s over 4 connections,
/// noise ~15 %) while each request's latency grew with the count, and a read
/// can't be taken back when the reader is left or jumps (4 in flight at once
/// after a jump measured ~10 % slower jumps, 4 runs each).
const ADAPTIVE_IN_FLIGHT: usize = 3;
/// Least time a read waits for a block before asking again (see
/// `ReadAheadReader::with_hedge`; the time is longer on a slow link).
pub const HEDGE_FLOOR: Duration = Duration::from_millis(200);

/// [`HEDGE_FLOOR`], or what `JUST_VIDEO_HEDGE_MS` says (0: never ask again),
/// for comparing on the headset.
pub fn hedge_floor() -> Option<Duration> {
    match std::env::var("JUST_VIDEO_HEDGE_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        Some(0) => None,
        Some(ms) => Some(Duration::from_millis(ms)),
        None => Some(HEDGE_FLOOR),
    }
}
/// A read asks again for a block that took this many times longer than
/// requests do lately (so a slow link does not double its own load).
const HEDGE_FACTOR: f64 = 3.0;
/// A read waiting this long for its block counts as a stall.
const STALL_SEEN: Duration = Duration::from_millis(150);
/// Each stall seen in steady reading makes the buffer this much larger…
const STALL_GROWTH: f64 = 1.5;
/// …up to this factor, shrinking back by half this often.
const BOOST_MAX: f64 = 3.0;
const BOOST_HALF_LIFE: Duration = Duration::from_secs(60);

enum Block {
    Pending(JoinHandle<io::Result<Vec<u8>>>),
    Ready(Vec<u8>),
}

/// What the reader holds, for a buffer indicator: the read position, the
/// file length and the byte ranges already fetched (sorted, disjoint).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Buffered {
    pub position: u64,
    pub len: u64,
    pub ranges: Vec<(u64, u64)>,
}

/// A live view of a reader's [`Buffered`] state, shared with the UI.
pub type BufferWatch = Arc<Mutex<Buffered>>;

/// How often a read waiting for its block logs (the request itself keeps
/// running: cancelling it would leak SMB credits).
const BLOCK_WAIT: Duration = Duration::from_secs(10);
/// How long a read may wait in total before failing.
const STALL_LIMIT: Duration = Duration::from_secs(45);
/// Failed block reads are retried this many times.
const READ_RETRIES: u32 = 3;
/// How long closing waits for outstanding reads.
const CLOSE_WAIT: Duration = Duration::from_secs(10);
/// Blocks requested right after a jump; the window doubles with each block
/// read straight on. Reads can't be cancelled (see `detached`), so a full
/// window requested just before the demuxer jumps elsewhere (probing a file's
/// index at its end, a seek) would delay the new position until all of it
/// arrived: ~0.25 s for 32 MiB over the headset's link.
const INITIAL_WINDOW: usize = 4;
/// Bytes of finished blocks kept after the reader moved away: for demuxers
/// that come back (an MP4 index at the end of the file, then the start of the
/// data), and for jumps back into what was just played (~15 s of 6K60).
const KEPT_BYTES: usize = 192 << 20;
/// With the stream's rate known, only this much of it is kept behind the
/// reader (at most `KEPT_BYTES`): a low-bitrate file's 192 MiB would be many
/// minutes of video, far more than a jump back or an A-B loop revisits.
const KEPT_SECONDS: f64 = 60.0;
/// ...but never less than this: the blocks around a keyframe, the index.
const KEPT_MIN_BYTES: usize = 32 << 20;
/// Reads in flight at most, however large the window: they can't be
/// cancelled, so after a jump the new position's data arrives only after
/// them. Over the headset's Wi-Fi 3 keep 8K (~50 Mbit/s) fed with no stall,
/// and a jump waits about half as long as with 6 (8K: ~145 vs ~260 ms);
/// 32 made a jump right after another wait up to 1.5 s.
const MAX_IN_FLIGHT: usize = 3;

pub struct ReadAheadReader<S: BlockSource> {
    runtime: Arc<Runtime>,
    source: Arc<S>,
    len: u64,
    pos: u64,
    options: ReadAhead,
    blocks: BTreeMap<u64, Block>,
    /// Recently left finished blocks, oldest first (see `KEPT_BYTES`).
    cache: VecDeque<(u64, Vec<u8>)>,
    /// Blocks to keep requested ahead now (grows to `blocks_ahead`).
    window: usize,
    /// The block the last read came from.
    last_index: Option<u64>,
    /// Reads no longer needed, left to finish. In-flight requests are never
    /// aborted: smb-rs returns a request's credits only when its response is
    /// received, so cancelled reads would leak credits until the connection
    /// can send nothing at all.
    detached: Vec<JoinHandle<io::Result<Vec<u8>>>>,
    in_flight: Arc<AtomicUsize>,
    /// Per requested block: true while its request counts in `in_flight`
    /// (a request given up on by hedging stops counting at once).
    counting: BTreeMap<u64, Arc<AtomicBool>>,
    stats: ReadStats,
    /// Asked before a read and while it waits for a block: true makes the
    /// read fail at once (see [`ReadAheadReader::with_cancel`]).
    cancel: Option<Cancel>,
    /// Set when a read failed for `cancel`: no more requests are issued, and
    /// dropping does not wait for those in flight.
    cancelled: bool,
    max_in_flight: usize,
    metrics: Arc<ReadMetrics>,
    /// Registered as the playing video's reader (see `for_playback`).
    playback: bool,
    adaptive: Option<Adaptive>,
    /// Stalls seen in steady reading make the buffer larger (1.0-`BOOST_MAX`).
    boost: f64,
    boost_at: Instant,
    /// Blocks aimed at ahead, as of the last read (see `target_blocks`).
    target: usize,
    /// Bytes read and since when, for the rate when the stream's is unknown.
    measured: (Instant, u64),
    measured_rate: f64,
    /// A read waiting this long (at least; see `hedge_wait`) for a block asks
    /// again (see `with_hedge`).
    hedge_after: Option<Duration>,
    watch: BufferWatch,
}

/// See [`ReadAheadReader::with_cancel`].
pub type Cancel = Arc<dyn Fn() -> bool + Send + Sync>;

/// How often a read waiting for a block looks at `cancel`.
const CANCEL_POLL: Duration = Duration::from_millis(10);

impl<S: BlockSource> ReadAheadReader<S> {
    pub fn new(runtime: Arc<Runtime>, source: S, len: u64, options: ReadAhead) -> Self {
        assert!(options.block_size > 0 && options.blocks_ahead > 0);
        Self {
            runtime,
            source: Arc::new(source),
            len,
            pos: 0,
            options,
            blocks: BTreeMap::new(),
            cache: VecDeque::new(),
            window: INITIAL_WINDOW.min(options.blocks_ahead),
            last_index: None,
            detached: Vec::new(),
            in_flight: Arc::new(AtomicUsize::new(0)),
            counting: BTreeMap::new(),
            stats: ReadStats::default(),
            cancel: None,
            cancelled: false,
            max_in_flight: MAX_IN_FLIGHT,
            metrics: Arc::new(ReadMetrics::default()),
            playback: false,
            adaptive: None,
            boost: 1.0,
            boost_at: Instant::now(),
            target: options.blocks_ahead,
            measured: (Instant::now(), 0),
            measured_rate: 0.0,
            hedge_after: None,
            watch: Arc::new(Mutex::new(Buffered {
                position: 0,
                len,
                ranges: Vec::new(),
            })),
        }
    }

    /// The reader's buffer state, kept up to date as it reads.
    pub fn watch(&self) -> BufferWatch {
        self.watch.clone()
    }

    /// Publishes the fetched ranges (contiguous runs of ready blocks).
    fn publish(&self) {
        let size = self.options.block_size as u64;
        // Ready blocks, and those kept after the reader moved away.
        let mut ready: Vec<u64> = self
            .blocks
            .iter()
            .filter(|(_, block)| matches!(block, Block::Ready(_)))
            .map(|(&index, _)| index)
            .chain(self.cache.iter().map(|(index, _)| *index))
            .collect();
        ready.sort_unstable();
        ready.dedup();
        let mut ranges: Vec<(u64, u64)> = Vec::new();
        for index in ready {
            let (start, end) = (index * size, ((index + 1) * size).min(self.len));
            match ranges.last_mut() {
                Some(last) if last.1 == start => last.1 = end,
                _ => ranges.push((start, end)),
            }
        }
        let state = Buffered {
            position: self.pos,
            len: self.len,
            ranges,
        };
        if let Ok(mut shared) = self.watch.lock()
            && *shared != state
        {
            *shared = state;
        }
    }

    /// A read left waiting for its block asks for it again, after `after` or
    /// [`HEDGE_FACTOR`] times what a request normally takes (the longer) (the source
    /// sends the second request over another connection when it has one) and
    /// takes whichever answer comes first: one flow stalled by loss or a
    /// timeout no longer holds the video up. Costs one extra block per slow
    /// block; the slower request is left to finish (never cancelled).
    pub fn with_hedge(mut self, after: Option<Duration>) -> Self {
        self.hedge_after = after;
        self
    }

    /// Sizes the buffer by time instead of a count of blocks: it aims to
    /// hold `adaptive.seconds` of the stream ahead (from its bytes per
    /// second, see [`ReadMetrics::set_rate_hint`]), within the byte limits,
    /// larger after stalls, and allows more reads in flight once it has grown
    /// (after a jump it starts small again, see `INITIAL_WINDOW`).
    pub fn with_adaptive(mut self, adaptive: Adaptive) -> Self {
        self.adaptive = Some(adaptive);
        self
    }

    /// Makes this the reader the timing log reports on (see
    /// [`read_metrics::playback_metrics`]).
    pub fn for_playback(mut self) -> Self {
        self.playback = true;
        read_metrics::register_playback(&self.metrics);
        self
    }

    pub fn metrics(&self) -> Arc<ReadMetrics> {
        self.metrics.clone()
    }

    /// Reads fail with `ErrorKind::Interrupted` as soon as `cancel` says so,
    /// also while waiting for data, and no read is requested after that.
    /// (Requests already sent can't be taken back, see `detached`; dropping
    /// the reader then leaves them to finish in the background.)
    pub fn with_cancel(mut self, cancel: Cancel) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Most reads in flight at a time (default [`MAX_IN_FLIGHT`]): less data
    /// still on the link when the reader is abandoned.
    pub fn with_max_in_flight(mut self, reads: usize) -> Self {
        self.max_in_flight = reads.max(1);
        self
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn stats(&self) -> ReadStats {
        self.stats
    }

    /// Blocks to aim to hold ahead: the policy's fixed count, or, with
    /// [`ReadAheadReader::with_adaptive`], the time wanted at the stream's
    /// rate (its own if known, else what the reader sees consumed).
    fn target_blocks(&mut self) -> usize {
        let Some(a) = self.adaptive else {
            return self.options.blocks_ahead;
        };
        let (since, bytes) = self.measured;
        if since.elapsed() >= Duration::from_secs(2) {
            let sample = bytes as f64 / since.elapsed().as_secs_f64();
            // Quick to rise, slow to fall: a stall must not shrink the buffer.
            self.measured_rate = sample.max(self.measured_rate * 0.9);
            self.measured = (Instant::now(), 0);
        }
        if self.boost > 1.0 && self.boost_at.elapsed() >= BOOST_HALF_LIFE {
            self.boost = 1.0 + (self.boost - 1.0) / 2.0;
            self.boost_at = Instant::now();
        }
        let hint = self.metrics.rate_hint() as f64;
        let rate = if hint > 0.0 { hint } else { self.measured_rate };
        let wanted = (rate * a.seconds * self.boost) as usize;
        wanted
            .clamp(a.min_bytes, a.max_bytes.max(a.min_bytes))
            .div_ceil(self.options.block_size)
            .max(1)
    }

    /// A read waited for data. In steady reading (the window has grown to
    /// what is wanted) that means the buffer was too small.
    fn note_stall(&mut self) {
        if self.adaptive.is_some() && self.window >= self.target {
            self.boost = (self.boost * STALL_GROWTH).min(BOOST_MAX);
            self.boost_at = Instant::now();
        }
    }

    /// Finished bytes waiting from block `current` on.
    fn ahead_bytes(&self, current: u64) -> u64 {
        self.blocks
            .range(current..)
            .filter(|(_, b)| match b {
                Block::Ready(_) => true,
                Block::Pending(h) => h.is_finished(),
            })
            .map(|(&i, _)| {
                (self.options.block_size as u64).min(self.len - i * self.options.block_size as u64)
            })
            .sum()
    }

    /// Blocks kept behind the reader: [`KEPT_SECONDS`] of the stream at its
    /// rate (its own if known, else what the reader sees consumed), between
    /// `KEPT_MIN_BYTES` and `KEPT_BYTES`; the most while the rate is unknown.
    fn kept_blocks(&self) -> usize {
        let hint = self.metrics.rate_hint() as f64;
        let rate = if hint > 0.0 { hint } else { self.measured_rate };
        let bytes = if self.adaptive.is_some() && rate > 0.0 {
            ((rate * KEPT_SECONDS) as usize).clamp(KEPT_MIN_BYTES, KEPT_BYTES)
        } else {
            KEPT_BYTES
        };
        (bytes / self.options.block_size).max(1)
    }

    fn spawn(&mut self, index: u64) {
        if let Some(at) = self.cache.iter().position(|(i, _)| *i == index) {
            let (_, data) = self.cache.remove(at).expect("cached block");
            self.blocks.insert(index, Block::Ready(data));
            return;
        }
        let task = self.start(index);
        self.blocks.insert(index, Block::Pending(task));
    }

    /// Requests block `index` from the source now.
    /// How long a read waits for its block before asking again.
    fn hedge_wait(&self, floor: Duration) -> Duration {
        floor.max(self.metrics.typical_latency().mul_f64(HEDGE_FACTOR))
    }

    fn start(&mut self, index: u64) -> JoinHandle<io::Result<Vec<u8>>> {
        let size = self.options.block_size as u64;
        let offset = index * size;
        let length = size.min(self.len - offset) as usize;
        self.stats.requests += 1;
        self.stats.bytes_fetched += length as u64;
        let fetch = self.source.clone().fetch(offset, length);
        let in_flight = self.in_flight.clone();
        in_flight.fetch_add(1, Ordering::SeqCst);
        let counting = Arc::new(AtomicBool::new(true));
        self.counting.insert(index, counting.clone());
        let metrics = self.metrics.clone();
        let requested = Instant::now();
        self.runtime.spawn(async move {
            let result = fetch.await;
            metrics.request_done(requested.elapsed());
            if counting.swap(false, Ordering::SeqCst) {
                in_flight.fetch_sub(1, Ordering::SeqCst);
            }
            result
        })
    }

    fn in_flight_cap(&self) -> usize {
        match self.adaptive {
            Some(a) => a.in_flight.max(1),
            None => self.max_in_flight,
        }
    }

    /// Keeps `[current, current + window)` requested and drops the rest,
    /// except one block behind, which absorbs small backward seeks by demuxers.
    fn schedule(&mut self, current: u64) {
        let target = self.target_blocks();
        self.target = target;
        self.window = next_window(
            self.window,
            self.last_index,
            current,
            // Kept from before counts: the reader came back to it.
            self.blocks.contains_key(&current) || self.cache.iter().any(|(i, _)| *i == current),
            target,
        );
        self.last_index = Some(current);
        let block_count = self.len.div_ceil(self.options.block_size as u64);
        let end = (current + self.window as u64).min(block_count);
        let keep_from = current.saturating_sub(1);
        // Only blocks beyond the full window are dropped ahead: a window that
        // is still growing keeps what it already asked for.
        let keep_until = (current + target.max(self.window) as u64).min(block_count);
        let stale: Vec<u64> = self
            .blocks
            .keys()
            .copied()
            .filter(|&i| i < keep_from || i >= keep_until)
            .collect();
        for index in stale {
            match self.blocks.remove(&index) {
                Some(Block::Pending(handle)) => self.detached.push(handle),
                Some(Block::Ready(data)) => {
                    let kept = self.kept_blocks();
                    while self.cache.len() >= kept {
                        self.cache.pop_front();
                    }
                    self.cache.push_back((index, data));
                }
                None => {}
            }
            self.stats.discarded_blocks += 1;
        }
        self.detached.retain(|h| !h.is_finished());
        self.counting.retain(|i, _| self.blocks.contains_key(i));
        // Detached reads count against the window, so seeking around cannot
        // pile up requests; the block needed now is always requested.
        self.metrics.sample(
            self.in_flight.load(Ordering::SeqCst),
            self.ahead_bytes(current),
            (target * self.options.block_size) as u64,
        );
        for index in current..end {
            if !self.blocks.contains_key(&index) {
                let limit = self.window.min(self.in_flight_cap());
                if index != current && self.in_flight.load(Ordering::SeqCst) >= limit {
                    break;
                }
                self.spawn(index);
            }
        }
    }
}

/// The read-ahead window for a read from block `current` (see `INITIAL_WINDOW`):
/// it doubles as reading moves on, stays while reading back within what is
/// already requested (demuxers going between audio and video), and starts
/// small again after a jump to blocks not requested.
fn next_window(
    window: usize,
    last: Option<u64>,
    current: u64,
    requested: bool,
    max: usize,
) -> usize {
    let initial = INITIAL_WINDOW.min(max);
    match last {
        Some(last) if current == last => window,
        Some(last) if current > last && requested => (window * 2).min(max),
        Some(_) if requested => window,
        _ => initial,
    }
}

fn interrupted() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "Read cancelled")
}

impl<S: BlockSource> Read for ReadAheadReader<S> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.pos >= self.len {
            return Ok(0);
        }
        if self.cancelled || self.cancel.as_ref().is_some_and(|c| c()) {
            self.cancelled = true;
            return Err(interrupted());
        }
        let size = self.options.block_size as u64;
        let index = self.pos / size;
        self.schedule(index);
        let mut retries = 0;
        let mut waited = Duration::ZERO;
        let mut logged = Duration::ZERO;
        // The second request for this block, once it waited long enough.
        let mut hedge: Option<JoinHandle<io::Result<Vec<u8>>>> = None;
        let mut hedged = false;
        loop {
            // Only while there is another connection to ask on.
            let after = self
                .hedge_after
                .filter(|_| self.source.lanes() > 1)
                .map(|floor| self.hedge_wait(floor));
            if let Some(after) = after
                && !hedged
                && waited >= after
                && matches!(self.blocks.get(&index), Some(Block::Pending(_)))
            {
                hedged = true;
                self.stats.hedged += 1;
                self.metrics.note_hedge();
                // The slow request no longer holds a place among those in
                // flight (it is left to finish): the new one takes it.
                if let Some(slow) = self.counting.get(&index)
                    && slow.swap(false, Ordering::SeqCst)
                {
                    self.in_flight.fetch_sub(1, Ordering::SeqCst);
                }
                hedge = Some(self.start(index));
            }
            let block = self.blocks.get_mut(&index).expect("scheduled block");
            let Block::Pending(handle) = block else { break };
            let started = Instant::now();
            // Short slices while cancellable, to look at `cancel` between.
            let mut slice = if self.cancel.is_some() {
                CANCEL_POLL
            } else {
                BLOCK_WAIT
            };
            if let Some(after) = after
                && !hedged
            {
                slice = slice.min(after.saturating_sub(waited).max(Duration::from_millis(1)));
            }
            let result = self.runtime.block_on(async {
                tokio::time::timeout(slice, async {
                    match hedge.as_mut() {
                        Some(second) => match futures_util::future::select(&mut *handle, second)
                            .await
                        {
                            futures_util::future::Either::Left((r, _)) => (r, false),
                            futures_util::future::Either::Right((r, _)) => (r, true),
                        },
                        None => ((&mut *handle).await, false),
                    }
                })
                .await
            });
            waited += started.elapsed();
            self.stats.stall_seconds += started.elapsed().as_secs_f64();
            if result.is_err() {
                if self.cancel.as_ref().is_some_and(|c| c()) {
                    self.cancelled = true;
                    self.detached.extend(hedge.take());
                    return Err(interrupted());
                }
                // Woke up to ask again (at the top of the loop), or to look at `cancel`.
                let hedge_pending = !hedged && self.hedge_after.is_some();
                if hedge_pending || (waited < logged + BLOCK_WAIT && waited < STALL_LIMIT) {
                    continue;
                }
                logged = waited;
            }
            match result {
                Ok((Ok(Ok(data)), from_hedge)) => {
                    let first = std::mem::replace(block, Block::Ready(data));
                    // The slower request is left to finish.
                    if from_hedge && let Block::Pending(slow) = first {
                        self.detached.push(slow);
                    } else {
                        self.detached.extend(hedge.take());
                    }
                }
                // One of two requests failed: the other may still answer.
                Ok((failed, from_hedge)) if hedge.is_some() => {
                    let e = match failed {
                        Ok(e) => e.err().map(|e| e.to_string()).unwrap_or_default(),
                        Err(e) => e.to_string(),
                    };
                    eprintln!("Read-ahead: a request at {} failed ({e}); the other goes on", self.pos);
                    let other = hedge.take().expect("second request");
                    if !from_hedge {
                        // The first failed: the second takes its place.
                        *block = Block::Pending(other);
                    }
                    // (A failed second request is just dropped.)
                }
                // Keep the request running (see `detached`) and keep waiting:
                // a slow server is better than a broken video.
                Err(_) if waited < STALL_LIMIT => {
                    eprintln!(
                        "Read-ahead: waited {:.0}s for data at {}",
                        waited.as_secs_f64(),
                        self.pos
                    );
                }
                Err(_) => {
                    self.detached.extend(hedge.take());
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "The server stopped sending data",
                    ));
                }
                Ok((failed, _)) => {
                    self.blocks.remove(&index);
                    let error = match failed {
                        Ok(e) => e.err().unwrap_or_else(|| io::Error::other("read failed")),
                        Err(e) => io::Error::other(e),
                    };
                    // A failed request is retried; a truncated packet would
                    // corrupt or stop the video.
                    if retries >= READ_RETRIES {
                        return Err(error);
                    }
                    retries += 1;
                    eprintln!("Read-ahead: retrying read at {} ({error})", self.pos);
                    std::thread::sleep(Duration::from_millis(200 * retries as u64));
                    self.spawn(index);
                }
            }
        }
        self.detached.extend(hedge.take());
        if waited >= STALL_SEEN {
            self.note_stall();
        }
        let block = self.blocks.get_mut(&index).expect("scheduled block");
        let Block::Ready(data) = block else {
            unreachable!()
        };
        let start = (self.pos - index * size) as usize;
        let n = out.len().min(data.len() - start);
        out[..n].copy_from_slice(&data[start..start + n]);
        self.pos += n as u64;
        self.stats.bytes_delivered += n as u64;
        self.measured.1 += n as u64;
        self.publish();
        Ok(n)
    }
}

impl<S: BlockSource> Seek for ReadAheadReader<S> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = target.ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        Ok(self.pos)
    }
}

impl<S: BlockSource> Drop for ReadAheadReader<S> {
    fn drop(&mut self) {
        if self.playback {
            read_metrics::unregister_playback(&self.metrics);
        }
        let runtime = self.runtime.clone();
        // Sources may spawn cleanup work on drop (smb-rs does).
        let _guard = runtime.enter();
        let mut pending = std::mem::take(&mut self.detached);
        for (_, block) in std::mem::take(&mut self.blocks) {
            if let Block::Pending(handle) = block {
                pending.push(handle);
            }
        }
        if self.cancelled {
            // Abandoned: whoever dropped us goes on; the reads finish (never
            // aborted; see `detached`) and the file closes behind them.
            let source = self.source.clone();
            runtime.spawn(async move {
                let _ = tokio::time::timeout(CLOSE_WAIT, async {
                    for handle in pending {
                        let _ = handle.await;
                    }
                })
                .await;
                source.close().await;
            });
            return;
        }
        // Let outstanding reads finish (never abort them; see `detached`).
        runtime.block_on(async {
            let _ = tokio::time::timeout(CLOSE_WAIT, async {
                for handle in pending {
                    let _ = handle.await;
                }
            })
            .await;
        });
        runtime.block_on(self.source.close());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// In-memory source with fixed per-request latency, like a network round trip.
    struct Slow {
        data: Vec<u8>,
        latency: Duration,
        completed: Arc<AtomicUsize>,
    }

    impl BlockSource for Slow {
        fn fetch(
            self: Arc<Self>,
            offset: u64,
            len: usize,
        ) -> BoxFuture<'static, io::Result<Vec<u8>>> {
            Box::pin(async move {
                tokio::time::sleep(self.latency).await;
                self.completed.fetch_add(1, Ordering::SeqCst);
                let start = offset as usize;
                Ok(self.data[start..start + len].to_vec())
            })
        }
    }

    fn reader(len: usize, latency_ms: u64, options: ReadAhead) -> (ReadAheadReader<Slow>, Vec<u8>) {
        let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_time()
                .build()
                .unwrap(),
        );
        let source = Slow {
            data: data.clone(),
            latency: Duration::from_millis(latency_ms),
            completed: Arc::new(AtomicUsize::new(0)),
        };
        (
            ReadAheadReader::new(runtime, source, len as u64, options),
            data,
        )
    }

    #[test]
    fn cancel_fails_a_read_waiting_for_data_and_requests_nothing_more() {
        let options = ReadAhead {
            block_size: 1000,
            blocks_ahead: 8,
        };
        let (r, _) = reader(100_000, 400, options);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let mut r = r.with_cancel(Arc::new(move || flag.load(Ordering::SeqCst)));
        let completed = r.source.completed.clone();
        let mut buf = [0u8; 10];
        let flag = stop.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let error = r.read(&mut buf).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_millis(250));
        stopper.join().unwrap();
        let requests = r.stats().requests;
        // Failed for good, with nothing new requested, also after a seek.
        r.seek(SeekFrom::Start(50_000)).unwrap();
        assert!(r.read(&mut buf).is_err());
        assert_eq!(r.stats().requests, requests);
        // Dropping doesn't wait for the reads in flight, which still finish.
        let _runtime = r.runtime.clone();
        let dropping = Instant::now();
        drop(r);
        assert!(dropping.elapsed() < Duration::from_millis(200));
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(completed.load(Ordering::SeqCst) as u64, requests);
    }

    #[test]
    fn max_in_flight_limits_read_ahead() {
        let options = ReadAhead {
            block_size: 1000,
            blocks_ahead: 8,
        };
        let (r, _) = reader(100_000, 50, options);
        let mut r = r.with_max_in_flight(1);
        let mut buf = [0u8; 10];
        r.read_exact(&mut buf).unwrap();
        assert_eq!(r.stats().requests, 1);
    }

    #[test]
    fn watch_reports_fetched_ranges() {
        let (mut reader, _) = reader(
            10 * 1024,
            1,
            ReadAhead {
                block_size: 1024,
                blocks_ahead: 4,
            },
        );
        let watch = reader.watch();
        assert_eq!(watch.lock().unwrap().len, 10 * 1024);
        let mut buf = [0u8; 100];
        reader.read_exact(&mut buf).unwrap();
        let state = watch.lock().unwrap().clone();
        assert_eq!(state.position, 100);
        assert_eq!(state.ranges.first().map(|r| r.0), Some(0));
        // Reading on waits for the rest of the window, which becomes one run.
        std::thread::sleep(Duration::from_millis(30));
        reader.seek(SeekFrom::Start(3000)).unwrap();
        reader.read_exact(&mut buf).unwrap();
        let state = watch.lock().unwrap().clone();
        assert!(
            state.ranges.iter().any(|&(a, b)| a <= 3000 && b >= 4096),
            "{state:?}"
        );
    }

    #[test]
    fn sequential_read_matches_source() {
        let options = ReadAhead {
            block_size: 4096,
            blocks_ahead: 8,
        };
        let (mut r, data) = reader(100_003, 0, options);
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
        assert_eq!(r.stats().bytes_delivered, data.len() as u64);
    }

    #[test]
    fn seeks_land_on_the_right_bytes() {
        let options = ReadAhead {
            block_size: 1000,
            blocks_ahead: 4,
        };
        let (mut r, data) = reader(50_000, 0, options);
        let mut buf = [0u8; 1500];
        for &(from, expect) in &[
            (SeekFrom::End(-1500), 48_500u64),
            (SeekFrom::Start(0), 0),
            (SeekFrom::Start(31_337), 31_337),
            (SeekFrom::Current(-2000), 30_837),
        ] {
            assert_eq!(r.seek(from).unwrap(), expect);
            r.read_exact(&mut buf).unwrap();
            let at = expect as usize;
            assert_eq!(&buf[..], &data[at..at + 1500]);
        }
        r.seek(SeekFrom::Start(50_000)).unwrap();
        assert_eq!(r.read(&mut buf).unwrap(), 0);
        assert!(r.seek(SeekFrom::Current(-60_000)).is_err());
    }

    /// Regression: aborting SMB reads leaked credits and hung the connection.
    #[test]
    fn reads_are_never_cancelled() {
        let options = ReadAhead {
            block_size: 1000,
            blocks_ahead: 8,
        };
        let (mut r, data) = reader(100_000, 20, options);
        let completed = r.source.completed.clone();
        let mut buf = [0u8; 10];
        r.read_exact(&mut buf).unwrap();
        // Jump far away: the first window's reads must still complete.
        r.seek(SeekFrom::Start(90_000)).unwrap();
        r.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, &data[90_000..90_010]);
        let requests = r.stats().requests;
        assert!(requests <= 16 + 1, "in-flight cap exceeded: {requests}");
        drop(r);
        assert_eq!(completed.load(Ordering::SeqCst) as u64, requests);
    }

    #[test]
    fn jumping_back_into_what_was_played_reads_nothing_again() {
        // 64 KiB blocks: 200 of them played (12.5 MiB), well within what's kept.
        let options = ReadAhead {
            block_size: 64 * 1024,
            blocks_ahead: 4,
        };
        let (mut r, data) = reader(220 * 64 * 1024, 0, options);
        let mut buf = vec![0u8; 200 * 64 * 1024];
        r.read_exact(&mut buf).unwrap();
        let requests = r.stats().requests;
        // Back to the start, and to the middle: no new reads.
        for at in [0usize, 100 * 64 * 1024 + 5] {
            r.seek(SeekFrom::Start(at as u64)).unwrap();
            let mut part = [0u8; 1000];
            r.read_exact(&mut part).unwrap();
            assert_eq!(&part[..], &data[at..at + 1000]);
        }
        assert_eq!(r.stats().requests, requests);
    }

    /// Fails every block's first request.
    struct Flaky {
        data: Vec<u8>,
        failed: std::sync::Mutex<std::collections::HashSet<u64>>,
    }

    impl BlockSource for Flaky {
        fn fetch(
            self: Arc<Self>,
            offset: u64,
            len: usize,
        ) -> BoxFuture<'static, io::Result<Vec<u8>>> {
            Box::pin(async move {
                if self.failed.lock().unwrap().insert(offset) {
                    return Err(io::Error::other("network hiccup"));
                }
                Ok(self.data[offset as usize..offset as usize + len].to_vec())
            })
        }
    }

    #[test]
    fn failed_reads_are_retried() {
        let data: Vec<u8> = (0..20_000).map(|i| (i % 251) as u8).collect();
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_time()
                .build()
                .unwrap(),
        );
        let source = Flaky {
            data: data.clone(),
            failed: Default::default(),
        };
        let options = ReadAhead {
            block_size: 4096,
            blocks_ahead: 2,
        };
        let mut r = ReadAheadReader::new(runtime, source, data.len() as u64, options);
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn window_grows_and_restarts_after_jumps() {
        // First read, then straight on: 4, 8, 16, 32 (the maximum).
        assert_eq!(next_window(32, None, 0, false, 32), 4);
        assert_eq!(next_window(4, Some(0), 1, true, 32), 8);
        assert_eq!(next_window(8, Some(1), 2, true, 32), 16);
        assert_eq!(next_window(32, Some(2), 3, true, 32), 32);
        // More reads from the same block keep it; a jump starts over.
        assert_eq!(next_window(16, Some(5), 5, true, 32), 16);
        assert_eq!(next_window(32, Some(5), 900, false, 32), 4);
        assert_eq!(next_window(32, Some(5), 2, false, 32), 4);
        // Back into blocks still held (interleaved audio and video) keeps it.
        assert_eq!(next_window(32, Some(5), 4, true, 32), 32);
        // Small read-aheads never exceed their own size.
        assert_eq!(next_window(2, None, 0, false, 2), 2);
    }

    /// A network link: one read at a time at a fixed speed, so later reads
    /// wait for earlier ones (unlike `Slow`, whose requests overlap freely).
    struct Link {
        data: Vec<u8>,
        per_byte: Duration,
        /// Added to every read after its turn on the link (not queued).
        rtt: Duration,
        busy_until: std::sync::Mutex<Instant>,
        /// Offsets fetched, in order.
        fetched: std::sync::Mutex<Vec<u64>>,
    }

    impl BlockSource for Link {
        fn fetch(
            self: Arc<Self>,
            offset: u64,
            len: usize,
        ) -> BoxFuture<'static, io::Result<Vec<u8>>> {
            let done = {
                let mut busy = self.busy_until.lock().unwrap();
                *busy = (*busy).max(Instant::now()) + self.per_byte * len as u32;
                *busy + self.rtt
            };
            self.fetched.lock().unwrap().push(offset);
            Box::pin(async move {
                tokio::time::sleep_until(done.into()).await;
                let start = offset as usize;
                Ok(self.data[start..start + len].to_vec())
            })
        }
    }

    fn link_reader(len: usize, ms_per_block: u64, options: ReadAhead) -> ReadAheadReader<Link> {
        link_reader_rtt(len, ms_per_block, 0, options)
    }

    fn link_reader_rtt(
        len: usize,
        ms_per_block: u64,
        rtt_ms: u64,
        options: ReadAhead,
    ) -> ReadAheadReader<Link> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_time()
                .build()
                .unwrap(),
        );
        let source = Link {
            data: (0..len).map(|i| (i % 251) as u8).collect(),
            per_byte: Duration::from_millis(ms_per_block) / options.block_size as u32,
            rtt: Duration::from_millis(rtt_ms),
            busy_until: std::sync::Mutex::new(Instant::now()),
            fetched: Default::default(),
        };
        ReadAheadReader::new(runtime, source, len as u64, options)
    }

    /// A demuxer alternating between streams stored a few blocks apart must
    /// keep the full window (reading ahead), not start it over each time.
    #[test]
    fn interleaved_reads_keep_the_window() {
        let options = ReadAhead {
            block_size: 1000,
            blocks_ahead: 32,
        };
        let mut r = link_reader_rtt(204_000, 1, 20, options);
        let mut buf = [0u8; 10];
        let started = Instant::now();
        for block in 0..200u64 {
            for at in [block + 3, block] {
                r.seek(SeekFrom::Start(at * 1000)).unwrap();
                r.read_exact(&mut buf).unwrap();
            }
        }
        let elapsed = started.elapsed();
        assert_eq!(r.window, 32);
        // 3 reads per 20 ms round trip: ~1.3 s.
        assert!(elapsed < Duration::from_millis(2000), "took {elapsed:?}");
    }

    /// Probing an MP4 with its index at the end: header, the end, then back.
    #[test]
    fn a_jump_soon_after_opening_does_not_wait_for_a_full_window() {
        let options = ReadAhead {
            block_size: 1000,
            blocks_ahead: 32,
        };
        // 5 ms per block: a full window queued ahead would take 160 ms.
        let mut r = link_reader(200_000, 5, options);
        let mut buf = [0u8; 100];
        let started = Instant::now();
        r.read_exact(&mut buf).unwrap();
        r.seek(SeekFrom::Start(150_000)).unwrap();
        r.read_exact(&mut buf).unwrap();
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_millis(80), "took {elapsed:?}");
        // Back to the start: block 0 is still here, not fetched again.
        r.seek(SeekFrom::Start(10)).unwrap();
        r.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], &r.source.data[10..110]);
        let fetched = r.source.fetched.lock().unwrap().clone();
        assert_eq!(
            fetched.iter().filter(|&&o| o == 0).count(),
            1,
            "{fetched:?}"
        );
    }

    #[test]
    fn pipelining_hides_latency() {
        // 64 blocks at 10 ms each: ~640 ms if serial, ~40 ms with 16 in flight.
        let options = ReadAhead {
            block_size: 8192,
            blocks_ahead: 16,
        };
        let (mut r, _) = reader(64 * 8192, 10, options);
        let started = Instant::now();
        std::io::copy(&mut r, &mut std::io::sink()).unwrap();
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_millis(400), "took {elapsed:?}");
        assert_eq!(r.stats().requests, 64);
    }

    /// 5 ms per request, except blocks `spike` which take `spike_for`: a
    /// Wi-Fi stall that holds some requests back.
    struct Spiky {
        data: Vec<u8>,
        block: usize,
        spike: std::ops::Range<usize>,
        spike_for: Duration,
    }

    impl BlockSource for Spiky {
        fn fetch(
            self: Arc<Self>,
            offset: u64,
            len: usize,
        ) -> BoxFuture<'static, io::Result<Vec<u8>>> {
            Box::pin(async move {
                let index = offset as usize / self.block;
                let wait = if self.spike.contains(&index) {
                    self.spike_for
                } else {
                    Duration::from_millis(5)
                };
                tokio::time::sleep(wait).await;
                Ok(self.data[offset as usize..offset as usize + len].to_vec())
            })
        }
    }

    const BLOCK: usize = 1000;

    fn spiky_reader(blocks: usize, spike: std::ops::Range<usize>) -> ReadAheadReader<Spiky> {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_time()
                .build()
                .unwrap(),
        );
        let source = Spiky {
            data: (0..blocks * BLOCK).map(|i| (i % 251) as u8).collect(),
            block: BLOCK,
            spike,
            spike_for: Duration::from_millis(1000),
        };
        let options = ReadAhead {
            block_size: BLOCK,
            blocks_ahead: 32,
        };
        ReadAheadReader::new(runtime, source, (blocks * BLOCK) as u64, options)
    }

    /// Plays 10 ms per block (100 kB/s) from block 40 on, after a start that
    /// is allowed to wait; returns the longest wait for a block.
    fn longest_wait_playing(mut r: ReadAheadReader<Spiky>, blocks: usize) -> Duration {
        let mut buf = vec![0u8; BLOCK];
        let mut longest = Duration::ZERO;
        for block in 0..blocks {
            let began = Instant::now();
            r.read_exact(&mut buf).unwrap();
            if block >= 40 {
                longest = longest.max(began.elapsed());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        longest
    }

    /// The old fixed read-ahead (3 reads in flight) is out of data when one
    /// second of requests are held back; a buffer of a few seconds is not.
    #[test]
    fn a_large_buffer_rides_out_a_one_second_latency_spike() {
        let blocks = 240;
        let small = spiky_reader(blocks, 100..110).with_max_in_flight(3);
        let stalled = longest_wait_playing(small, blocks);
        assert!(stalled > Duration::from_millis(400), "small: {stalled:?}");

        let big = spiky_reader(blocks, 100..110).with_adaptive(Adaptive {
            seconds: 3.0,
            min_bytes: 4 * BLOCK,
            max_bytes: 400 * BLOCK,
            in_flight: 12,
        });
        big.metrics().set_rate_hint(100_000);
        let metrics = big.metrics();
        let waited = longest_wait_playing(big, blocks);
        assert!(waited < Duration::from_millis(100), "big: {waited:?}");
        // The spike shows in the figures, and the buffer was kept full.
        let window = metrics.take();
        assert!(window.max_ms >= 1000.0, "{window}");
        assert!(window.p50_ms < 100.0, "{window}");
        assert!(window.in_flight_mean.unwrap() > 1.0, "{window}");
        assert!(
            window.target_mib * (1 << 20) as f64 >= 299_000.0,
            "{window}"
        );
    }

    #[test]
    fn the_buffer_never_holds_more_than_its_memory_cap() {
        let blocks = 400;
        let mut r = spiky_reader(blocks, 0..0).with_adaptive(Adaptive {
            seconds: 30.0,
            min_bytes: 4 * BLOCK,
            max_bytes: 50 * BLOCK,
            in_flight: 12,
        });
        // A stream far faster than the cap: 30 s of it would be 3 GB.
        r.metrics().set_rate_hint(100_000_000);
        let mut buf = vec![0u8; BLOCK];
        let mut most = 0;
        for _ in 0..blocks {
            r.read_exact(&mut buf).unwrap();
            // Let the link catch up: the buffer is as full as it gets.
            std::thread::sleep(Duration::from_millis(1));
            most = most.max(r.blocks.len());
        }
        // The cap, and the block read now and the one kept behind it.
        assert!(most <= 50 + 2, "held {most} blocks");
        assert!(most >= 40, "never filled the buffer: {most}");
        assert!(r.cache.len() * BLOCK <= KEPT_BYTES);
    }

    #[test]
    fn kept_blocks_follow_the_stream_rate_within_limits() {
        const MIB: usize = 1 << 20;
        let mut r = spiky_reader(2, 0..0).with_adaptive(Adaptive::default());
        let kept = |r: &ReadAheadReader<Spiky>| r.kept_blocks() * BLOCK;
        let near = |got: usize, want: usize| got.abs_diff(want) < BLOCK;
        // Rate unknown: as much as is allowed.
        assert!(near(kept(&r), KEPT_BYTES));
        // 1 MiB/s: a minute of it.
        r.metrics().set_rate_hint(MIB as u64);
        assert!(near(kept(&r), 60 * MIB));
        // Slow streams keep the minimum, fast ones the cap.
        r.metrics().set_rate_hint(100_000);
        assert!(near(kept(&r), KEPT_MIN_BYTES));
        r.metrics().set_rate_hint(13 * MIB as u64);
        assert!(near(kept(&r), KEPT_BYTES));
        // Without the adaptive buffer (no rate to go by) the cap applies.
        let plain = spiky_reader(2, 0..0);
        plain.metrics().set_rate_hint(MIB as u64);
        assert!(near(kept(&plain), KEPT_BYTES));
    }

    #[test]
    fn the_buffer_follows_the_stream_rate_and_grows_after_stalls() {
        let mut r = spiky_reader(10, 0..0).with_adaptive(Adaptive {
            seconds: 10.0,
            min_bytes: 4 * BLOCK,
            max_bytes: 100 * BLOCK,
            in_flight: 12,
        });
        // Nothing known: the minimum (a 1080p-sized stream stays cheap).
        assert_eq!(r.target_blocks(), 4);
        r.metrics().set_rate_hint(5_000);
        assert_eq!(r.target_blocks(), 50);
        // A stall while the window was still growing does not count…
        r.window = 8;
        r.target = 50;
        r.note_stall();
        assert_eq!(r.target_blocks(), 50);
        // …in steady reading each one grows the buffer, up to the cap.
        r.window = 50;
        r.note_stall();
        assert_eq!(r.target_blocks(), 75);
        r.note_stall();
        r.note_stall();
        assert_eq!(r.target_blocks(), 100);
        // Slow streams stay at the minimum, fast ones are capped.
        r.metrics().set_rate_hint(100);
        r.boost = 1.0;
        assert_eq!(r.target_blocks(), 4);
        r.metrics().set_rate_hint(10_000_000);
        assert_eq!(r.target_blocks(), 100);
    }

    #[test]
    fn a_fixed_reader_keeps_its_count_of_blocks() {
        let mut r = spiky_reader(10, 0..0);
        r.metrics().set_rate_hint(1_000_000);
        assert_eq!(r.target_blocks(), 32);
    }

    /// Two connections; the first request for one block is held back for a
    /// second (a flow stalled by loss), the second request is quick.
    struct Hedgy {
        data: Vec<u8>,
        asked: std::sync::Mutex<Vec<u64>>,
        fail_first: bool,
    }

    impl BlockSource for Hedgy {
        fn lanes(&self) -> usize {
            2
        }

        fn fetch(
            self: Arc<Self>,
            offset: u64,
            len: usize,
        ) -> BoxFuture<'static, io::Result<Vec<u8>>> {
            Box::pin(async move {
                let first = {
                    let mut asked = self.asked.lock().unwrap();
                    asked.push(offset);
                    asked.iter().filter(|&&o| o == offset).count() == 1
                };
                if offset == 5 * BLOCK as u64 && first {
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                    if self.fail_first {
                        return Err(io::Error::other("connection reset"));
                    }
                } else {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Ok(self.data[offset as usize..offset as usize + len].to_vec())
            })
        }
    }

    fn hedgy_reader(hedge: Option<u64>, fail_first: bool) -> (ReadAheadReader<Hedgy>, Vec<u8>) {
        let runtime = Arc::new(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_time()
                .build()
                .unwrap(),
        );
        let data: Vec<u8> = (0..10 * BLOCK).map(|i| (i % 251) as u8).collect();
        let source = Hedgy {
            data: data.clone(),
            asked: Default::default(),
            fail_first,
        };
        let options = ReadAhead {
            block_size: BLOCK,
            blocks_ahead: 4,
        };
        let r = ReadAheadReader::new(runtime, source, data.len() as u64, options)
            .with_hedge(hedge.map(Duration::from_millis));
        (r, data)
    }

    #[test]
    fn a_slow_block_is_asked_again_and_the_first_answer_wins() {
        let (mut r, data) = hedgy_reader(Some(100), false);
        let started = Instant::now();
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        let took = started.elapsed();
        assert_eq!(got, data);
        assert!(took < Duration::from_millis(600), "took {took:?}");
        assert_eq!(r.stats().hedged, 1);

        // Without it the reader waits out the slow request.
        let (mut r, _) = hedgy_reader(None, false);
        let started = Instant::now();
        std::io::copy(&mut r, &mut std::io::sink()).unwrap();
        assert!(started.elapsed() > Duration::from_millis(900));
        assert_eq!(r.stats().hedged, 0);
    }

    #[test]
    fn a_failed_first_request_does_not_spoil_the_second() {
        let (mut r, data) = hedgy_reader(Some(100), true);
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        assert_eq!(got, data);
        assert_eq!(r.stats().hedged, 1);
    }

    #[test]
    fn one_connection_never_hedges() {
        let mut single = spiky_reader(4, 1..2).with_hedge(Some(Duration::from_millis(10)));
        std::io::copy(&mut single, &mut std::io::sink()).unwrap();
        assert_eq!(single.stats().hedged, 0);
    }
}
