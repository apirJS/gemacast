use std::time::{Duration, Instant};

/// Application delivery timing; deliberately not labelled as wire latency.
pub(crate) struct ReceiveDiagnostics {
    window_start: Instant,
    last_datagram: Option<Instant>,
    datagrams: u64,
    bytes: u64,
    timeouts: u64,
    max_gap_us: u64,
}

impl ReceiveDiagnostics {
    pub fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            last_datagram: None,
            datagrams: 0,
            bytes: 0,
            timeouts: 0,
            max_gap_us: 0,
        }
    }

    pub fn received(&mut self, bytes: usize, now: Instant) {
        if let Some(last) = self.last_datagram {
            self.max_gap_us = self
                .max_gap_us
                .max(now.duration_since(last).as_micros() as u64);
        }
        self.last_datagram = Some(now);
        self.datagrams += 1;
        self.bytes += bytes as u64;
    }

    pub fn timeout(&mut self) {
        self.timeouts += 1;
    }

    pub fn report(&mut self, now: Instant, completed_pcm_frames: u64, expired_pcm_frames: u64) {
        if now.duration_since(self.window_start) < Duration::from_secs(1) {
            return;
        }
        tracing::info!(
            datagrams = self.datagrams,
            bytes = self.bytes,
            timeouts = self.timeouts,
            max_delivery_gap_us = self.max_gap_us,
            completed_pcm_frames,
            expired_pcm_frames,
            window_ms = now.duration_since(self.window_start).as_millis() as u64,
            "[AudioReceive] Window"
        );
        self.window_start = now;
        self.datagrams = 0;
        self.bytes = 0;
        self.timeouts = 0;
        self.max_gap_us = 0;
    }
}
