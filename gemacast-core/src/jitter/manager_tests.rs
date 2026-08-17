use super::super::consts::OLA_LEN;
use super::*;
use crate::audio::{OPUS_CHANNELS, OPUS_FRAME_SAMPLES, OPUS_SAMPLE_RATE};
use crate::domain::types::LinkPair;
use opus::{Application, Channels, Decoder, Encoder};
use ringbuf::HeapRb;
use std::time::Duration;
use std::time::Instant;

/// MIN_DEPTH = ceil(40ms / MILLIS_PER_FRAME)
const MIN_DEPTH: u32 = 40 / MILLIS_PER_FRAME;
fn test_config() -> JitterConfig {
    JitterConfig {
        min_depth_ms: 40,
        comfort_cap_ms: 200,
        peak_decay_halflife_ms: 1000,
        resume_threshold_pct: 0.5,
        static_target_ms: None,
    }
}

fn setup_env() -> (
    JitterBufferManager,
    Encoder,
    ringbuf::HeapProd<RawPacket>,
    ringbuf::HeapCons<RawPacket>,
) {
    let decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Stereo).unwrap();
    let encoder = Encoder::new(OPUS_SAMPLE_RATE, Channels::Stereo, Application::Audio).unwrap();
    let atomic = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let config_ref = Arc::new(std::sync::RwLock::new(test_config()));
    let is_tcp_mode = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Default to Unknown → default REORDER_TOLERANCE, matching legacy behaviour.
    let manager = JitterBufferManager::new(
        decoder,
        atomic,
        Arc::new(std::sync::atomic::AtomicU32::new(0)),
        config_ref,
        is_tcp_mode,
        NetworkLink::Unknown,
    );
    let rb = HeapRb::<RawPacket>::new(1000);
    let (prod, cons) = rb.split();
    (manager, encoder, prod, cons)
}

fn make_packet(encoder: &mut Encoder, seq: u64, base_time: Instant) -> RawPacket {
    let pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
    let d = encoder.encode_vec_float(&pcm, 1500).unwrap();
    let payload_len = d.len();
    let mut pkt = RawPacket::zeroed();
    pkt.seq_num = seq;
    pkt.payload_data[..payload_len].copy_from_slice(&d);
    pkt.payload_len = payload_len;
    pkt.arrival_time = base_time + std::time::Duration::from_millis(seq * MILLIS_PER_FRAME as u64);
    pkt
}

/// Build a manager that has already completed its first prebuffer (so the
/// startup flush is spent), then starved into a rebuffer pause with the target
/// pinned. Returns it held in the pause, ready for a burst, alongside the
/// `(arrival, seq)` the burst must be stamped with.
///
/// Both of those are load-bearing rather than convenience. `stats.observe`
/// **drops** any packet whose `arrival_time` precedes the last forward arrival,
/// so a burst stamped with a fresh `Instant::now()` lands *behind* the setup
/// packets' synthetic timeline and never reaches the buffer at all — occupancy
/// stays 0 and every assertion downstream reads as "the clamp did nothing".
/// The sequence number is taken from the live playhead for the same reason: a
/// hardcoded one is stale the moment the starvation loop advances past it.
///
/// Pinning `ramp_goal` alongside `effective_target` keeps `advance` inside its
/// dead zone, so the callback under test sees the pinned target rather than a
/// mid-ramp value — same device as
/// `the_resume_threshold_should_depend_only_on_target_resume_threshold_pct_and_min_depth`.
fn rebuffering_at_target(
    target: u32,
) -> (
    JitterBufferManager,
    Encoder,
    ringbuf::HeapProd<RawPacket>,
    ringbuf::HeapCons<RawPacket>,
    Instant,
    u64,
) {
    rebuffering_at_target_after(target, DEFAULT_OUTAGE)
}

/// The outage [`rebuffering_at_target`] resumes from: 300ms of stamp, 100ms of
/// observed gap, 10 frames into the gap window.
///
/// **Chosen so `raw_target` stays below any target a caller pins.** This was
/// once 2s, which was incidental — the burst only had to be *forward* of the
/// setup arrivals. The resume band is now read at `max(target, raw_target)`, so
/// the outage length decides which band is under test, and 2s decides it
/// degenerately: an 1800ms gap is 180 frames, `GAP_CLAMP_FRAMES` saturates it to
/// 120, and `gap_floor` then pins `raw_target` at the 2.4GHz profile's 80-frame
/// comfort cap — above every burst these tests can push, so no clamp fires and
/// `the_rebuffer_exit_*` tests assert against a mechanism that never ran.
///
/// 100ms is also the honest figure for the link these tests model: the 2.4GHz
/// captures measured a *median* delivery gap of 21.6 frames and p90 34.5, so a
/// 2s outage was never a DTIM gap — it was an outage long enough to look like a
/// disconnect.
const DEFAULT_OUTAGE: Duration = Duration::from_millis(300);

/// [`rebuffering_at_target`] with the outage length under the caller's control.
///
/// The last setup packet arrives at `base + 200ms`, so the burst's stamp of
/// `base + outage` is observed by [`JitterStats::observe`] as a delivery gap of
/// **`outage - 200ms`**, which `record_gap` folds into the window as
/// `(outage_ms - 200) / 10` frames. That is the only route by which a test can
/// set `max_gap` — and therefore `raw_target` — to a chosen value, because
/// `record_gap` keeps the *maximum* of its bucket and the burst's own arrival
/// overwrites anything primed by hand beforehand.
///
/// It matters that the length is a parameter rather than the fixed 2s: at 2s the
/// observed gap is 180 frames, which `GAP_CLAMP_FRAMES` (120) saturates and
/// `raw_target` then pins at the profile's 80-frame comfort cap — above any
/// burst a test can push through the ring, so no clamp fires at all and every
/// assertion downstream reads as "the resume did nothing".
fn rebuffering_at_target_after(
    target: u32,
    outage: Duration,
) -> (
    JitterBufferManager,
    Encoder,
    ringbuf::HeapProd<RawPacket>,
    ringbuf::HeapCons<RawPacket>,
    Instant,
    u64,
) {
    let config = JitterConfig::for_link_pair(LinkPair {
        phone: NetworkLink::Wifi2_4Ghz,
        pc: NetworkLink::Wifi2_4Ghz,
    });
    let (mut manager, mut encoder, mut prod, mut cons) =
        setup_env_with(config, NetworkLink::Wifi2_4Ghz);
    let base = Instant::now();
    let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

    // First prebuffer: complete it and spend the startup flush, or the clamp
    // under test is gated off entirely.
    for i in 1..=20u64 {
        assert!(
            prod.try_push(make_tone_packet_at(
                &mut encoder,
                i,
                base + Duration::from_millis(i * 10),
                0.5
            ))
            .is_ok()
        );
    }
    manager.ingest_packets(&mut cons);
    for _ in 0..25 {
        manager.control.effective_target = target;
        manager.control.ramp_goal = target;
        manager.fill_output(&mut output, 1.0);
    }
    assert!(
        !manager.startup_flush_pending,
        "precondition: the startup flush must be spent before the rebuffer \
             clamp can run at all",
    );

    // Starve into the rebuffer pause.
    for _ in 0..=REBUFFER_AFTER {
        manager.control.effective_target = target;
        manager.control.ramp_goal = target;
        manager.fill_output(&mut output, 1.0);
    }
    assert!(
        manager.flow.is_prebuffering,
        "precondition: the manager must be in the rebuffer pause",
    );
    manager.log_window.tally.flush_discards = 0;

    // Forward of the last setup arrival on the stats clock, so the burst is not
    // dropped as a reordered packet, and separated from it by the outage the
    // caller asked for.
    let burst_at = base + outage;
    let burst_seq = manager.buffer.next_play_seq();
    (manager, encoder, prod, cons, burst_at, burst_seq)
}

/// A packet carrying a loud, strongly-periodic tone (200 Hz sine). Two
/// properties matter for the drain tests: RMS is well above the old 0.08
/// gate (so it proves the gate is gone), and the waveform is periodic so the
/// WSOLA correlation gate (0.9) reliably finds a splice and accelerate/expand
/// actually fire.
fn make_loud_packet(encoder: &mut Encoder, seq: u64, base_time: Instant) -> RawPacket {
    let ch = OPUS_CHANNELS as usize;
    let frames = OPUS_FRAME_SAMPLES / ch;
    // Continuous phase across packets so the tone is seamless: sample index
    // is anchored to the absolute frame position (seq * frames).
    let mut pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
    let base_frame = seq * frames as u64;
    for i in 0..frames {
        let t = (base_frame + i as u64) as f32 / OPUS_SAMPLE_RATE as f32;
        let s = (2.0 * std::f32::consts::PI * 200.0 * t).sin() * 0.5;
        for c in 0..ch {
            pcm[i * ch + c] = s;
        }
    }
    let d = encoder.encode_vec_float(&pcm, 1500).unwrap();
    let payload_len = d.len();
    let mut pkt = RawPacket::zeroed();
    pkt.seq_num = seq;
    pkt.payload_data[..payload_len].copy_from_slice(&d);
    pkt.payload_len = payload_len;
    pkt.arrival_time = base_time + std::time::Duration::from_millis(seq * MILLIS_PER_FRAME as u64);
    pkt
}

/// A tone packet with an explicit arrival time and amplitude. The two
/// convenience builders above derive arrival from `seq`, which makes them
/// useless for simulating a DTIM timeline (gap, then a burst that all lands
/// at once). `amp = 0.03` gives rms ≈ 0.021 — inside
/// `SILENCE_RMS..ARTIFACT_MASK_RMS`, i.e. exactly the "quiet but not silent"
/// band where the old code fired its expand click train.
fn make_tone_packet_at(encoder: &mut Encoder, seq: u64, arrival: Instant, amp: f32) -> RawPacket {
    let ch = OPUS_CHANNELS as usize;
    let frames = OPUS_FRAME_SAMPLES / ch;
    let mut pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
    let base_frame = seq * frames as u64;
    for i in 0..frames {
        let t = (base_frame + i as u64) as f32 / OPUS_SAMPLE_RATE as f32;
        let s = (2.0 * std::f32::consts::PI * 200.0 * t).sin() * amp;
        for c in 0..ch {
            pcm[i * ch + c] = s;
        }
    }
    let d = encoder.encode_vec_float(&pcm, 1500).unwrap();
    let payload_len = d.len();
    let mut pkt = RawPacket::zeroed();
    pkt.seq_num = seq;
    pkt.payload_data[..payload_len].copy_from_slice(&d);
    pkt.payload_len = payload_len;
    pkt.arrival_time = arrival;
    pkt
}

/// A packet of loud, **unpitched** noise with an explicit arrival time.
///
/// The mirror of [`make_tone_packet_at`]: same rms band, no pitch period. The
/// correlation search cannot find a lag that clears the 0.9 NCC gate, so every
/// timescale attempt on this material declines — which is what makes the
/// `declined_*` census observable end-to-end instead of only at the actuator.
/// The draws must be independent per packet; a repeated buffer autocorrelates
/// perfectly at a one-frame lag and would splice for a reason that has nothing
/// to do with the content being pitched.
fn make_noise_packet_at(
    encoder: &mut Encoder,
    seq: u64,
    arrival: Instant,
    amp: f32,
    seed: &mut u32,
) -> RawPacket {
    let mut pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
    for s in pcm.iter_mut() {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *s = ((*seed >> 8) as f32 / 8_388_608.0 - 1.0) * amp;
    }
    let d = encoder.encode_vec_float(&pcm, 1500).unwrap();
    let payload_len = d.len();
    let mut pkt = RawPacket::zeroed();
    pkt.seq_num = seq;
    pkt.payload_data[..payload_len].copy_from_slice(&d);
    pkt.payload_len = payload_len;
    pkt.arrival_time = arrival;
    pkt
}

/// Build a manager with an explicit config — `setup_env`'s `test_config` has a
/// 200ms comfort cap, too shallow to hold a 200ms DTIM gap.
fn setup_env_with(
    config: JitterConfig,
    link: NetworkLink,
) -> (
    JitterBufferManager,
    Encoder,
    ringbuf::HeapProd<RawPacket>,
    ringbuf::HeapCons<RawPacket>,
) {
    let decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Stereo).unwrap();
    let encoder = Encoder::new(OPUS_SAMPLE_RATE, Channels::Stereo, Application::Audio).unwrap();
    let atomic = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let config_ref = Arc::new(std::sync::RwLock::new(config));
    let is_tcp_mode = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let manager = JitterBufferManager::new(
        decoder,
        atomic,
        Arc::new(std::sync::atomic::AtomicU32::new(0)),
        config_ref,
        is_tcp_mode,
        link,
    );
    let rb = HeapRb::<RawPacket>::new(4000);
    let (prod, cons) = rb.split();
    (manager, encoder, prod, cons)
}
/// What `fill_output` emits before the buffer is ready, and the volume scaling
/// applied to whatever it emits.
mod prebuffer_and_output {
    use super::*;

    #[test]
    fn should_output_silence_while_prebuffering_until_target_depth() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        // Push MIN_DEPTH - 1 packets: should still be prebuffering.
        for i in 1..MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![1.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        for &sample in &output {
            assert_eq!(sample, 0.0, "Expected silence while prebuffering");
        }
        assert!(manager.flow.is_prebuffering);
        // Push the final packet to reach MIN_DEPTH: should exit prebuffering.
        assert!(
            prod.try_push(make_packet(&mut encoder, MIN_DEPTH as u64, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);
    }

    #[test]
    fn should_apply_volume_scaling_during_fill_output() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        let make_noisy_packet =
            |encoder: &mut Encoder, seq: u64, base_time: Instant| -> RawPacket {
                let mut pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
                for (i, sample) in pcm.iter_mut().enumerate() {
                    *sample = if i % 2 == 0 { 0.5 } else { -0.5 };
                }
                let d = encoder.encode_vec_float(&pcm, 1500).unwrap();
                let payload_len = d.len();
                let mut pkt = RawPacket::zeroed();
                pkt.seq_num = seq;
                pkt.payload_data[..payload_len].copy_from_slice(&d);
                pkt.payload_len = payload_len;
                pkt.arrival_time = base_time + std::time::Duration::from_millis(seq * 5);
                pkt
            };

        // Fill enough to exit prebuffering.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_noisy_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // Get output at full volume
        let mut full_vol = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut full_vol, 1.0);

        // Reset and replay at half volume
        let (mut manager2, mut encoder2, mut prod2, mut cons2) = setup_env();
        for i in 1..=MIN_DEPTH {
            assert!(
                prod2
                    .try_push(make_noisy_packet(&mut encoder2, i as u64, base_time))
                    .is_ok()
            );
        }
        manager2.ingest_packets(&mut cons2);

        let mut half_vol = vec![0.0; OPUS_FRAME_SAMPLES];
        manager2.fill_output(&mut half_vol, 0.5);

        // Every non-zero sample at half volume should be ~half of full volume
        let mut checked = false;
        for (f, h) in full_vol.iter().zip(half_vol.iter()) {
            if f.abs() > 0.001 {
                let ratio = h / f;
                assert!(
                    (ratio - 0.5).abs() < 0.01,
                    "Expected half-volume ratio ~0.5, got {ratio}"
                );
                checked = true;
            }
        }
        assert!(
            checked,
            "Expected at least one non-zero sample to verify volume scaling"
        );
    }
}

/// The first exit from prebuffering, which the startup flush owns rather than
/// the rebuffer clamp.
mod startup_flush {
    use super::*;

    /// Root cause 5. With virgin stats the histogram is still on its geometric
    /// seed (p95 = 4 frames) and the gap window is empty, so `target` says ~4 —
    /// but no link delivers that cleanly from cold. Every field log starved on
    /// the link's first ordinary 50-90ms gap because the startup flush had just
    /// thrown that depth away. The 8-frame floor is what makes the first minute
    /// survivable.
    ///
    /// Uses a real profile (`min_depth_ms: 30`, as every Auto profile in
    /// `types.rs` does) rather than `test_config`'s 40ms: at 40ms the older
    /// `min_depth * 2` term already reached 8 by itself and would mask a
    /// regression of the floor.
    #[test]
    fn startup_flush_never_leaves_fewer_than_eight_frames() {
        let config = JitterConfig {
            min_depth_ms: 30,
            comfort_cap_ms: 400,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.25,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi5Ghz);
        let burst_arrival = Instant::now();
        // 40 packets landing at once: the socket-buffer burst that accumulates
        // while the DAC callback has not started consuming yet.
        for i in 1..=40u64 {
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, i, burst_arrival, 0.5))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        assert_eq!(manager.buffer.occupied_count(), 40);

        // The first callback completes prebuffering and runs the startup flush.
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        assert!(!manager.flow.is_prebuffering);
        assert!(
            manager.buffer.occupied_count() >= 7,
            "the startup flush must leave >= 8 frames (7 after this callback \
                 played one); left {}",
            manager.buffer.occupied_count(),
        );
    }

    /// The startup flush, not the rebuffer clamp, must own the very first exit.
    ///
    /// Both run in the same callback and both flush, so which one wins decides the
    /// startup depth. The clamp is gated on `!startup_flush_pending`, which makes
    /// the startup flush win by construction — and the depth the clamp flushes *to*
    /// has moved twice since (raised, then moved to `band_hi`), so the
    /// ordering carries more weight than it did: at a virgin `effective_target` the
    /// clamp's depth is still only ~5 frames, and a clamp running first would hand
    /// the startup flush a buffer already below its 8-frame floor, re-creating
    /// exactly the cold-start starvation that floor exists to prevent.
    ///
    /// Distinct from `startup_flush_never_leaves_fewer_than_eight_frames`, which
    /// pins the floor's value; this pins which mechanism applied it.
    #[test]
    fn the_startup_flush_should_own_the_first_exit_rather_than_the_rebuffer_clamp() {
        let config = JitterConfig {
            min_depth_ms: 30,
            comfort_cap_ms: 400,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.25,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi5Ghz);
        assert!(
            manager.startup_flush_pending,
            "precondition: the startup flush must still be pending, or this test \
                 proves nothing about which mechanism runs first",
        );

        let burst_arrival = Instant::now();
        for i in 1..=40u64 {
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, i, burst_arrival, 0.5))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        assert!(!manager.flow.is_prebuffering);
        assert!(
            !manager.startup_flush_pending,
            "the startup flush must have consumed its one chance on this callback",
        );
        // The clamp would have flushed to its own depth — `band_hi`, floored
        // at the release threshold. The startup flush's floor is 8. Landing at or
        // above 7 (one frame played by this callback) is only reachable via the
        // startup flush path. Computed the way the call site computes it, so this
        // precondition cannot go stale against the clamp again.
        let target = manager.control.effective_target;
        let unpause = ((target as f32 * manager.config.resume_threshold_pct) as u32)
            .max(manager.min_depth_frames());
        let raw_target = manager.target_breakdown(None).raw;
        let resume_depth = TargetController::buffer_limits(target.max(raw_target))
            .high
            .max(unpause);
        assert!(
            resume_depth < 7,
            "precondition: the clamp's depth ({resume_depth}) must be shallower \
                 than the startup floor, or the two are indistinguishable here",
        );
        assert!(
            manager.buffer.occupied_count() >= 7,
            "the startup floor must have applied, not the clamp's {resume_depth} \
                 frames; left {}",
            manager.buffer.occupied_count(),
        );
    }
}

/// Where the target lands: jitter observation, the clean-streak regime,
/// hysteresis, and the static-target override.
mod depth_control {
    use super::*;

    /// **The field complaint, end to end.** Router A/B on 2.4GHz with the screen
    /// off: DTIM batching delivers a 200ms silence followed by a 20-packet burst,
    /// forever. An earlier controller walked the target monotonically to the comfort
    /// cap and stayed there — "my jitter algorithm fails to find Lowest Most Stable
    /// Buffer Range". This asserts both halves of the contract: cover the observed
    /// gap while it is happening, and come back down once it stops.
    #[test]
    fn dtim_gap_raises_the_target_then_a_clean_link_brings_it_back_down() {
        let config = JitterConfig {
            min_depth_ms: 30,
            comfort_cap_ms: 800,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi2_4Ghz);
        let base = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut seq = 1u64;
        let mut now = base;

        // --- Phase 1: 30 DTIM cycles. Each is a 200ms gap, then 20 packets
        // (200ms of audio) landing together, then 20 playback callbacks.
        for _ in 0..30 {
            now += Duration::from_millis(200);
            for i in 0..20u64 {
                let arrival = now + Duration::from_micros(i * 50);
                assert!(
                    prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.03))
                        .is_ok()
                );
                seq += 1;
            }
            manager.ingest_packets(&mut cons);
            for _ in 0..20 {
                manager.fill_output(&mut output, 1.0);
            }
        }

        let raised = manager.control.effective_target;
        assert!(
            raised >= 18,
            "a repeating 200ms delivery gap must lift the target to ~20 frames, got {raised}",
        );
        assert!(
            raised <= 40,
            "the target must cover the gap, not double it — got {raised}",
        );

        // --- Phase 2: the link goes clean (screen back on). 10ms arrivals, one
        // packet per callback, for longer than the 24s gap window.
        for _ in 0..4000 {
            now += Duration::from_millis(10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, now, 0.03))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            // Advance the injected clock so floor-relax timers expire relative to
            // the simulated timeline. The test's `now` is ~40s past `base`, so
            // FLOOR_HOLD_SECS and FLOOR_DECAY_SECS both read as long expired.
            manager.set_test_clock(now);
            manager.fill_output(&mut output, 1.0);
        }

        let settled = manager.control.effective_target;
        assert!(
            settled <= 6,
            "a clean link must bring the target back to the lowest stable range \
                 (≤6 frames); got {settled} — this is the monotonic ratchet",
        );
    }

    #[test]
    fn should_respect_static_target_ms_when_configured() {
        let decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Stereo).unwrap();
        let atomic = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let static_config = JitterConfig {
            min_depth_ms: 10,
            comfort_cap_ms: 200,
            peak_decay_halflife_ms: 1000,
            resume_threshold_pct: 0.5,
            static_target_ms: Some(100), // Lock to 100ms
        };
        let config_ref = Arc::new(std::sync::RwLock::new(static_config));
        let is_tcp_mode = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut manager = JitterBufferManager::new(
            decoder,
            atomic,
            Arc::new(std::sync::atomic::AtomicU32::new(0)),
            config_ref,
            is_tcp_mode,
            NetworkLink::Unknown,
        );

        // Static mode should lock target to ceil(100ms / MILLIS_PER_FRAME)
        let expected = 100 / MILLIS_PER_FRAME;
        let target = manager.target_breakdown(None).raw;
        assert_eq!(
            target, expected,
            "Static target should be exactly {} frames for 100ms",
            expected
        );

        // Even with massive jitter, static target should not change
        manager.stats.ema_jitter = 50.0;
        manager.stats.record_gap(100.0, Instant::now());
        let target_after_jitter = manager.target_breakdown(None).raw;
        assert_eq!(
            target_after_jitter, expected,
            "Static target should ignore jitter"
        );
    }

    /// Static non-zero targets (e.g. Custom preset with 60ms) must aggressively
    /// flush excess packets so the buffer stays pinned at the configured depth.
    /// After a burst, occupied should never exceed target + 1 frame.
    #[test]
    fn static_nonzero_target_should_pin_buffer_at_configured_depth() {
        let decoder = Decoder::new(OPUS_SAMPLE_RATE, Channels::Stereo).unwrap();
        let atomic = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let static_config = JitterConfig {
            min_depth_ms: 20,
            comfort_cap_ms: 60,
            peak_decay_halflife_ms: 1000,
            resume_threshold_pct: 0.5,
            static_target_ms: Some(60), // Fixed 60ms = 6 frames
        };
        let config_ref = Arc::new(std::sync::RwLock::new(static_config));
        let is_tcp_mode = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut manager = JitterBufferManager::new(
            decoder,
            atomic,
            Arc::new(std::sync::atomic::AtomicU32::new(0)),
            config_ref,
            is_tcp_mode,
            NetworkLink::Unknown,
        );

        let mut encoder =
            Encoder::new(OPUS_SAMPLE_RATE, Channels::Stereo, Application::Audio).unwrap();
        let rb = HeapRb::<RawPacket>::new(1000);
        let (mut prod, mut cons) = rb.split();
        let base_time = Instant::now();

        let target_frames = 60 / MILLIS_PER_FRAME; // 6 frames

        // Fill buffer to exit prebuffering (need resume_threshold * target = 3 frames).
        for i in 1..=(target_frames + 2) {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        // Inject a 20-packet burst (simulating network batching).
        let mut seq = (target_frames + 3) as u64;
        for _ in 0..20 {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);

        // After one fill_output, the static flush should have drained the excess.
        manager.fill_output(&mut output, 1.0);

        assert!(
            manager.buffer.occupied_count() <= target_frames + 1,
            "Static 60ms target should pin buffer at ≤{} frames, got {}",
            target_frames + 1,
            manager.buffer.occupied_count()
        );

        // Run several more callbacks to confirm it stays pinned.
        for _ in 0..10 {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
        }

        assert!(
            manager.buffer.occupied_count() <= target_frames + 1,
            "Buffer should remain pinned after steady-state operation, got {}",
            manager.buffer.occupied_count()
        );
        assert_eq!(
            manager.flow.starvation_count, 0,
            "Static flush must not cause starvation"
        );
    }

    #[test]
    fn clean_streak_should_build_on_stable_packets_and_reset_on_spikes() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Feed 100 clean packets with perfect 5ms inter-arrival (zero jitter).
        for i in 1..=100 {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // clean_streak should be high (all packets had jitter < 1 frame)
        assert!(
            manager.stats.clean_streak >= 90,
            "Expected clean_streak >= 90 after 100 clean packets, got {}",
            manager.stats.clean_streak
        );

        // stability_ratio should be meaningfully positive
        assert!(
            manager.stats.stability_ratio() > 0.2,
            "Expected stability_ratio > 0.2, got {}",
            manager.stats.stability_ratio()
        );

        // Now inject a severe spike: a packet with 200ms delay (40 frames of jitter)
        let mut spike_pkt = make_packet(&mut encoder, 101, base_time);
        spike_pkt.arrival_time =
            base_time + std::time::Duration::from_millis(101 * MILLIS_PER_FRAME as u64 + 200);
        assert!(prod.try_push(spike_pkt).is_ok());
        manager.ingest_packets(&mut cons);

        // Severe spike (>4 frames) should slam clean_streak to 0
        assert_eq!(
            manager.stats.clean_streak, 0,
            "Expected clean_streak = 0 after severe spike"
        );
    }

    #[test]
    fn stable_network_should_decay_jitter_faster_than_unstable() {
        let (mut manager1, mut encoder1, mut prod1, mut cons1) = setup_env();
        let (mut manager2, mut encoder2, mut prod2, mut cons2) = setup_env();
        let base_time = Instant::now();

        // Both managers: inject a spike to raise ema_jitter
        let spike_offset = std::time::Duration::from_millis(100); // 100ms jitter
        for i in 1..=2 {
            let mut pkt1 = make_packet(&mut encoder1, i, base_time);
            let mut pkt2 = make_packet(&mut encoder2, i, base_time);
            if i == 2 {
                pkt1.arrival_time += spike_offset;
                pkt2.arrival_time += spike_offset;
            }
            assert!(prod1.try_push(pkt1).is_ok());
            assert!(prod2.try_push(pkt2).is_ok());
        }
        manager1.ingest_packets(&mut cons1);
        manager2.ingest_packets(&mut cons2);

        let spike_jitter1 = manager1.stats.ema_jitter;
        let spike_jitter2 = manager2.stats.ema_jitter;
        assert!(
            (spike_jitter1 - spike_jitter2).abs() < 0.01,
            "Both managers should have the same jitter after the spike"
        );

        // Manager 1: simulate a stable network (400 clean packets → full stability)
        manager1.stats.clean_streak = 400;
        // Manager 2: simulate an unstable network (0 clean streak)
        manager2.stats.clean_streak = 0;

        // Feed 200 clean packets to both (zero jitter)
        for i in 3..=202 {
            let pkt1 = make_packet(&mut encoder1, i, base_time);
            let pkt2 = make_packet(&mut encoder2, i, base_time);
            assert!(prod1.try_push(pkt1).is_ok());
            assert!(prod2.try_push(pkt2).is_ok());
        }
        manager1.ingest_packets(&mut cons1);
        manager2.ingest_packets(&mut cons2);

        // The stable manager should have decayed significantly faster
        assert!(
            manager1.stats.ema_jitter < manager2.stats.ema_jitter,
            "Stable network jitter ({}) should be less than unstable ({})",
            manager1.stats.ema_jitter,
            manager2.stats.ema_jitter
        );
    }

    #[test]
    fn hysteresis_should_ignore_transient_spikes() {
        let (mut manager, _, _, _) = setup_env();
        manager.flow.is_prebuffering = false;

        // Set a known effective target and ramp goal.
        manager.control.effective_target = 12;
        manager.control.ramp_goal = 12;
        manager.control.target_exit_count = 0;

        // Simulate a single transient delivery spike: one 300ms gap is enough to
        // send `raw_target` far outside the hysteresis band.
        manager.stats.ema_jitter = 10.0;
        manager.stats.record_gap(30.0, Instant::now());

        // Call process_next_frame once (no packets, will generate PLC).
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        // The effective_target must NOT have jumped to the spike-induced value.
        // Hysteresis requires HYSTERESIS_DWELL (40) consecutive callbacks outside the band.
        assert_eq!(
            manager.control.effective_target, 12,
            "Effective target should stay at 12 after a single spike-induced fill_output, got {}",
            manager.control.effective_target
        );
    }

    #[test]
    fn regime_aware_clean_threshold_should_build_streak_on_2_4ghz() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Simulate 2.4GHz-like jitter: consistent 20ms jitter per packet.
        // Each packet arrives at 30ms intervals instead of the expected 10ms.
        // IAT = 30ms, expected = 10ms, jitter = 20ms = 2 frames.
        //
        // With the old fixed threshold (< 1 frame = 5ms), clean_streak would never build.
        // With the adaptive threshold, ema_jitter settles around 2 frames,
        // clean_threshold ≈ 2*1.5+1.0 = 4.0 frames, and 2-frame jitter counts as "clean".

        for i in 1..=200u64 {
            let mut pkt = make_packet(&mut encoder, i, base_time);
            // 30ms spacing creates 20ms jitter (30ms actual - 10ms expected per frame).
            pkt.arrival_time = base_time + std::time::Duration::from_millis(i * 30);
            assert!(prod.try_push(pkt).is_ok());
        }
        manager.ingest_packets(&mut cons);

        // ema_jitter should have settled around 2 frames (20ms jitter / 10ms per frame).
        assert!(
            manager.stats.ema_jitter > 1.0,
            "Expected ema_jitter > 1.0 for 20ms baseline jitter, got {}",
            manager.stats.ema_jitter
        );

        // clean_streak should have built up because jitter is consistent
        // (below the adaptive threshold of ema_jitter * 1.5 + 1.0).
        assert!(
            manager.stats.clean_streak >= 50,
            "Expected clean_streak >= 50 on consistent 2.4GHz jitter, got {}",
            manager.stats.clean_streak
        );

        // stability_ratio should be meaningfully positive.
        assert!(
            manager.stats.stability_ratio() > 0.1,
            "Expected stability_ratio > 0.1, got {}",
            manager.stats.stability_ratio()
        );
    }

    #[test]
    fn should_not_oscillate_target_under_high_jitter_variance() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Feed 500 packets with wildly varying inter-arrival times
        // to simulate a terrible 2.4GHz connection.
        for i in 1..=500u64 {
            let mut pkt = make_packet(&mut encoder, i, base_time);
            // Alternate: even packets arrive 0ms late, odd packets arrive 80ms late.
            let jitter_ms = if i % 2 == 0 { 0u64 } else { 80 };
            pkt.arrival_time = base_time
                + std::time::Duration::from_millis(i * MILLIS_PER_FRAME as u64 + jitter_ms);
            assert!(prod.try_push(pkt).is_ok());
        }
        manager.ingest_packets(&mut cons);

        // Now simulate 200 process_next_frame calls and count how many times
        // effective_target changes.
        manager.flow.is_prebuffering = false;
        // Pre-fill buffer so we don't starve.
        for i in 501..=700u64 {
            let pkt = make_packet(&mut encoder, i, base_time);
            assert!(prod.try_push(pkt).is_ok());
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut changes = 0u32;
        let mut last_target = manager.control.effective_target;
        for _ in 0..200 {
            manager.fill_output(&mut output, 1.0);
            if manager.control.effective_target != last_target {
                changes += 1;
                last_target = manager.control.effective_target;
            }
        }

        // With hysteresis + quantization + rate-limiting, the target should change
        // fewer than 15 times across 200 callbacks (vs. potentially every callback before).
        assert!(
            changes < 15,
            "Expected fewer than 15 target changes across 200 callbacks, got {changes}"
        );
    }
}

/// The under-delivery hold — arrivals measured against the expected packet
/// rate, and how a sustained shortfall is billed as starvation.
mod under_delivery {
    use super::*;

    /// The reshaped under-delivery detector, against the worst window of a field
    /// 2.4GHz capture: 105 arrivals over a **3161ms** callback window.
    ///
    /// Both receiver-side counters read healthy there — 105 arrivals against 86
    /// frames played is a ratio of 1.22, and the old `arrivals * 2 < played` test
    /// stayed silent through the entire collapse in two consecutive field rounds.
    /// Measured against wall clock the same window delivered 33% of nominal.
    ///
    /// The frame-count assumption is what hid it: a window *assumed* to be
    /// `LOG_INTERVAL_CALLBACKS * MILLIS_PER_FRAME` long expects 100 packets and sees
    /// 105, which reads as a surplus.
    #[test]
    fn under_delivery_should_fire_when_arrivals_fall_below_the_expected_packet_rate() {
        let measured = JitterBufferManager::expected_packets(3161);
        assert_eq!(measured, 316, "one packet per frame period of wall clock");
        assert!(
            JitterBufferManager::is_under_delivering(105, measured),
            "105 arrivals over 3161ms is 33% of nominal and must be reported",
        );

        let assumed =
            JitterBufferManager::expected_packets(LOG_INTERVAL_CALLBACKS * MILLIS_PER_FRAME);
        assert!(
            !JitterBufferManager::is_under_delivering(105, assumed),
            "guard on the reason this was invisible: against an assumed window the \
                 same collapse reads as a surplus",
        );
    }

    /// The false-positive guard. `frames_played` must not reach this decision at
    /// all: a slow playback rate is a receiver-side symptom with many causes, and
    /// only the arrival rate against wall clock says the *link* is at fault.
    ///
    /// 100 arrivals in a 1000ms window is nominal delivery. That the DAC consumed
    /// only 30 frames over the same second is a different problem — it must not be
    /// reported as "the link is not carrying the stream".
    #[test]
    fn under_delivery_should_not_fire_when_only_the_playback_rate_falls() {
        let expected = JitterBufferManager::expected_packets(1000);
        assert_eq!(expected, 100);
        assert!(
            !JitterBufferManager::is_under_delivering(100, expected),
            "nominal delivery must never be reported, whatever playback did",
        );

        // The healthy floor actually observed in the field: 5GHz bottomed out at
        // 0.90 of nominal across 257 windows, ADB likewise across 205. Neither may
        // trip the detector, or the signal is noise.
        assert!(
            !JitterBufferManager::is_under_delivering(90, expected),
            "0.90 of nominal is the measured healthy floor on ADB and 5GHz",
        );
        // ...and the 2.4GHz collapse population, which tops out at 0.57, must.
        assert!(
            JitterBufferManager::is_under_delivering(57, expected),
            "0.57 of nominal is the healthiest window of the 2.4GHz collapse",
        );
    }

    /// The converse, so the ratio cannot become a latch: a link that delivers on
    /// time must never have its playhead held, however long it runs.
    #[test]
    fn a_healthy_link_must_never_trip_the_under_delivery_hold() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);

        // One packet per callback for three full conceal windows.
        let mut seq = u64::from(MIN_DEPTH);
        for _ in 0..180 {
            seq += 1;
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
            assert!(
                !manager.flow.is_prebuffering,
                "an on-time link must never be held",
            );
        }
    }

    /// The 5GHz storm, reproduced: sustained under-delivery at roughly ⅓ rate.
    ///
    /// `REBUFFER_AFTER` counts *consecutive* empty callbacks and every pop zeroes
    /// `starvation_count`, so a packet arriving every 3rd callback keeps the run
    /// length at 1-2 forever. The field log is the proof — 299 starvation onsets,
    /// exactly **one** rebuffer, zero stream resets, across 9.4s. The playhead
    /// must be held on the *ratio*, which a run-length counter cannot see.
    #[test]
    fn a_trickled_arrival_must_not_leave_the_playhead_running_through_a_deficit() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

        // Prebuffer normally, then start playing.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);
        assert!(
            !manager.flow.is_prebuffering,
            "playback should have started"
        );

        // One packet every 3rd callback: the buffer never holds a backlog, so
        // `starvation_count` is reset by each arrival before it can reach
        // `REBUFFER_AFTER`. Runs one full conceal window plus a margin.
        let mut seq = u64::from(MIN_DEPTH);
        let mut max_starvation_run = 0;
        for callback in 0..80 {
            if callback % 3 == 0 {
                seq += 1;
                assert!(
                    prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                        .is_ok()
                );
                manager.ingest_packets(&mut cons);
            }
            manager.fill_output(&mut output, 1.0);
            max_starvation_run = max_starvation_run.max(manager.flow.starvation_count);
            if manager.flow.is_prebuffering {
                break;
            }
        }

        assert!(
            max_starvation_run < REBUFFER_AFTER,
            "precondition: the trickle must keep the consecutive-starvation run \
                 below REBUFFER_AFTER ({REBUFFER_AFTER}), which is why the existing \
                 mechanism cannot see this failure — observed run was \
                 {max_starvation_run}",
        );
        assert!(
            manager.flow.is_prebuffering,
            "sustained under-delivery must hold the playhead so the deficit is \
                 taken as one gap instead of a stutter per arrival",
        );
    }

    /// A starvation *cluster* must bill one episode, not one per onset. This is
    /// the 501-line log storm as a unit test: the count and duration move into a
    /// closing census, so coverage is unchanged and only volume drops.
    #[test]
    fn sustained_undelivery_should_bill_one_starvation_episode_not_one_per_arrival() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);

        // Trickle: several distinct onsets, separated by pops.
        let mut seq = u64::from(MIN_DEPTH);
        for callback in 0..40 {
            if callback % 4 == 0 {
                seq += 1;
                assert!(
                    prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                        .is_ok()
                );
                manager.ingest_packets(&mut cons);
            }
            manager.fill_output(&mut output, 1.0);
        }

        assert!(
            manager.flow.starvation_events > 1,
            "precondition: the trickle must produce several onsets (got {})",
            manager.flow.starvation_events,
        );
        assert!(
            manager.flow.episode_started_at.is_some(),
            "the onsets must belong to one open episode, not one episode each",
        );

        // Delivery recovers past min_depth: the episode closes exactly once, and
        // its census reports every onset it spanned.
        for i in 1..=(MIN_DEPTH * 3) {
            assert!(
                prod.try_push(make_loud_packet(
                    &mut encoder,
                    seq + u64::from(i),
                    base_time
                ))
                .is_ok()
            );
        }
        seq += u64::from(MIN_DEPTH * 3);
        manager.ingest_packets(&mut cons);
        for _ in 0..(MIN_DEPTH * 2) {
            manager.fill_output(&mut output, 1.0);
            if manager.flow.episode_started_at.is_none() {
                break;
            }
        }
        assert!(
            manager.flow.episode_started_at.is_none(),
            "a recovery to healthy occupancy must close the episode",
        );
        assert_eq!(
            manager.flow.starvation_events, 0,
            "closing the episode must hand the census over exactly once",
        );
        let _ = seq;
    }
}

/// Running dry: PLC across a single loss, and the rebuffer hold that a
/// sustained outage collapses into one event.
mod starvation_and_rebuffer {
    use super::*;

    #[test]
    fn should_trigger_plc_and_recover_on_single_packet_loss() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        // Loud packets throughout: this test counts callbacks against frames, and
        // on *silent* program material the free-silence growth path legitimately
        // emits two frames for one packet whenever occupancy is below target,
        // which decouples the two. Real audio keeps one callback == one frame.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);
        // Push a packet with a gap (skip one seq num) to simulate packet loss.
        let gap_seq = (MIN_DEPTH + 2) as u64;
        assert!(
            prod.try_push(make_loud_packet(&mut encoder, gap_seq, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);
        // Drain the remaining valid packets.
        for _ in 2..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }
        // The missing packet in the gap triggers PLC.
        manager.fill_output(&mut output, 1.0);
        // With small gap (1 slot, <=20): waits REORDER_TOLERANCE callbacks before advancing.
        // After REORDER_TOLERANCE-1 waits the slot is declared lost (missing_count=1).
        // After 1 more call, the future packet (gap_seq) becomes the expected seq and plays.
        for _ in 0..(REORDER_TOLERANCE - 1) {
            manager.fill_output(&mut output, 1.0);
        }
        assert_eq!(manager.flow.missing_count, 0);
        assert!(!manager.flow.is_prebuffering);
    }

    #[test]
    fn sustained_starvation_rebuffers_once_and_keeps_playing_plc() {
        // A contract change. Originally a mid-stream starvation never re-entered
        // prebuffering: playback resumed on the very first frame that arrived
        // (`has_next`). On loud content that is the *only* way depth can be
        // banked — packets arrive at real time and the DAC consumes at real time,
        // and both growth paths (silence-grow, expand) are gated off — so the
        // buffer could only grow by starving again. The field log shows the
        // result: six starvation events in six seconds off a single delivery gap.
        //
        // Now, after REBUFFER_AFTER starved callbacks, the playhead pauses and
        // refills to `resume_threshold_pct * target` in one go. PLC keeps
        // playing throughout — the pause is silent to the user in exactly the way
        // the gap already was.
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        // Loud packets: one callback must equal one frame for the starvation
        // count to be meaningful (see the note in the PLC test).
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.flow.is_prebuffering);

        // Starve. The pause must arm exactly at REBUFFER_AFTER, not before.
        for n in 1..=REBUFFER_AFTER {
            manager.fill_output(&mut output, 1.0);
            assert_eq!(
                manager.flow.is_prebuffering,
                n == REBUFFER_AFTER,
                "rebuffer must arm on starved callback {REBUFFER_AFTER}, not {n}",
            );
        }
        assert_eq!(manager.flow.starvation_count, REBUFFER_AFTER);

        // PLC (not digital silence) is what the pause *opens* with.
        //
        // The concealment fade is keyed to `conceal_run` rather than
        // `starvation_count`, so the gain no longer freezes at
        // `1.0 - (5-3)/4 = 0.5` for the length of the hold — it continues to zero
        // over the next two concealed callbacks. This assertion therefore reads
        // here, while the fade is still open, instead of after 25 further
        // callbacks: what the pause must not do is open on raw silence.
        assert!(
            output.iter().any(|s| s.abs() > 0.0),
            "the rebuffer pause must open on PLC, not raw silence",
        );

        // Keep starving. The pause holds, and `starvation_count` stops climbing:
        // one gap is now one event, which is what stops the floor and the stats
        // from being re-bumped six times over.
        for _ in 1..=25 {
            manager.fill_output(&mut output, 1.0);
            assert!(manager.flow.is_prebuffering);
        }
        assert_eq!(
            manager.flow.starvation_count, REBUFFER_AFTER,
            "one delivery gap must produce exactly one starvation event",
        );
        // What a sustained hold decays *to* has changed, so what this assertion can
        // demand at the end of one changes with it. It once demanded exact digital
        // silence, which was the right terminus while the fade was applied to
        // `decode_plc()` output and is the wrong one now that the schedule stops at
        // `CONCEAL_FADE_FLOOR`: 39 of 64 field holds ran past the frame where the
        // old schedule reached zero, muting 382 frames / 3820ms of audio that had
        // really played.
        //
        // The discriminator this assertion has always carried is *anti-latch* — a
        // hold must not freeze at one gain for its whole length — and it still
        // does, now from the other end. The fade must travel all the way down to
        // its floor rather than park mid-schedule, and the floor must not be
        // silence.
        assert_eq!(
            manager.flow.conceal_run,
            REBUFFER_AFTER + 25,
            "the fade must be keyed to a counter that spans the whole hold, not \
                 to `starvation_count`, which froze at REBUFFER_AFTER and pinned the \
                 gain at 1.0 - (5-3)/4 = 0.5 for every callback of it",
        );
        assert_eq!(
            manager.log_window.tally.floor_frames,
            REBUFFER_AFTER + 25 - 13,
            "and every concealed frame from the 14th on must be at the floor: \
                 the schedule has to reach bottom within a hold this long, or a \
                 mid-schedule gain is what sustains for its tail",
        );
        let peak = output.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            peak > 0.0,
            "the floor is not silence — a sustained hold stays audible. Reaching \
                 zero here is what the field reported as a dropout",
        );
    }

    /// The rebuffer pause returns early, before the normal `missing_count`
    /// bookkeeping. Without the guard inside that branch a permanent outage
    /// entered during rebuffering would wait forever instead of resetting the
    /// stream.
    ///
    /// The assertion is on a *reset-only* observable — the cleared gap window.
    /// `is_prebuffering` and `missing_count == 0` are both true whether or not
    /// the reset fired (the pause sets the former and skips the latter), so
    /// neither can tell a working guard from a missing one.
    #[test]
    fn rebuffering_still_reaches_stream_reset_on_a_permanent_outage() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }

        // Learned link state that only `trigger_reset` clears.
        manager.stats.record_gap(50.0, Instant::now());
        assert!(manager.stats.max_gap_frames() > 40.0);

        let limit = JitterBufferManager::max_missing_for(NetworkLink::Unknown);
        for _ in 0..=limit {
            manager.fill_output(&mut output, 1.0);
        }
        assert_eq!(
            manager.stats.max_gap_frames(),
            0.0,
            "a permanent outage must still reach trigger_reset from inside the \
                 rebuffer pause; the gap window is still holding learned state",
        );
        assert!(manager.flow.is_prebuffering);
        assert_eq!(manager.flow.missing_count, 0);
        assert_eq!(manager.flow.starvation_count, 0);
    }

    /// The Router A machine-gun as a unit test. An outage, then a *sliver* of
    /// frames too thin to sustain playback, then more outage. Before the rebuffer
    /// pause the sliver resumed playback (`has_next`) and starved again three frames
    /// later, so one delivery gap billed several starvation events, several floor
    /// bumps and several audible stutters. The rebuffer pause must collapse the
    /// whole cluster into one event, and must still release the moment a real burst
    /// clears the resume threshold.
    #[test]
    fn rebuffer_pause_collapses_a_starvation_cluster_into_one_event() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }

        // Outage 1 — long enough to arm the pause.
        for _ in 1..=REBUFFER_AFTER {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(manager.flow.is_prebuffering);
        let floor_before = manager.control.probe_floor();

        // A 3-frame sliver: one below the resume threshold, which is floored at
        // min_depth precisely so a sliver can never restart the playhead.
        for i in (MIN_DEPTH + 1)..=(MIN_DEPTH + 3) {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        // Outage 2. Playback must not have resumed on the sliver, so this cannot
        // become a second starvation event.
        for _ in 1..=15 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(
            manager.flow.is_prebuffering,
            "a sliver below the resume threshold must not restart the playhead",
        );
        assert_eq!(
            manager.buffer.occupied_count(),
            3,
            "the paused playhead must not have consumed the sliver",
        );
        assert_eq!(
            manager.flow.starvation_count, REBUFFER_AFTER,
            "the second outage must not bill a second starvation event",
        );
        assert_eq!(
            manager.control.probe_floor(),
            floor_before,
            "one delivery gap must cost at most one floor bump",
        );

        // A real burst clears the threshold — the pause must release, and the
        // single event pays its single floor bump here.
        for i in (MIN_DEPTH + 4)..=(MIN_DEPTH + 8) {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);
        assert!(
            !manager.flow.is_prebuffering,
            "the pause must release once the buffer clears the resume threshold",
        );
        assert_eq!(
            manager.flow.starvation_count, 0,
            "recovery must clear the starvation counter",
        );
        assert!(
            manager.control.probe_floor() > floor_before,
            "the one event must still teach the floor exactly once",
        );
    }
}

/// The resume clamp — how much of an already-released burst is kept, read
/// against the band at `max(target, raw_target)`.
mod rebuffer_resume {
    use super::*;

    /// The resume depth must follow the target, not a flat ceiling.
    ///
    /// This test's inverse (`rebuffer_hold_should_not_exceed_the_absolute_cap…`)
    /// asserted that 15 frames released the playhead however deep the target was.
    /// The field measured what that bought: a resume at 150ms on a link
    /// whose median delivery gap was 216ms, re-starving at a median of **0.33s**
    /// where unbound resumes in the same capture lasted **5.07s**. A buffer of
    /// depth D survives a gap of width G only if D ≥ G, so the cap made re-
    /// starvation arithmetic rather than bad luck.
    ///
    /// Both bounds are asserted: 15 frames must NOT release, `0.5 * target` must.
    /// A release-only assertion would pass identically with the cap restored.
    #[test]
    fn the_rebuffer_resume_depth_should_scale_with_the_target_rather_than_an_absolute_cap() {
        let old_cap_frames = ms_to_frames_ceil(150);

        // The 2.4GHz Auto profile's shape: a deep cap and a 50% resume ratio.
        let config = JitterConfig {
            min_depth_ms: 30,
            comfort_cap_ms: 800,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi2_4Ghz);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

        // Teach a 760ms DTIM gap, which legitimately earns a target near the cap.
        // The target is left where the gap justifies it — only the hole is capped.
        // The ramp is rate-limited and dwell-gated, so drive callbacks (kept fed,
        // so this climb is not itself a starvation) until it is past the cap.
        let mut seq = 0u64;
        for _ in 0..600 {
            if manager.control.effective_target > old_cap_frames * 2 {
                break;
            }
            seq += 1;
            let _ = prod.try_push(make_packet(&mut encoder, seq, Instant::now()));
            manager.ingest_packets(&mut cons);
            manager.stats.record_gap(76.0, Instant::now());
            manager.fill_output(&mut output, 1.0);
        }
        // `effective_target` is read live at every step below, never snapshotted —
        // it drifts within a few callbacks.
        assert!(
            manager.control.effective_target > old_cap_frames * 2,
            "precondition: the target must be more than twice the old cap so that \
                 `0.5 * target` is strictly above it (target={}, old cap={old_cap_frames})",
            manager.control.effective_target,
        );

        // Force a rebuffer and offer exactly what the old cap would have released
        // on. The threshold is now `0.5 * target`, which is strictly greater.
        manager.buffer.reset();
        manager.flow.is_prebuffering = true;
        for i in 1..=old_cap_frames {
            assert!(
                prod.try_push(make_packet(
                    &mut encoder,
                    seq + u64::from(i),
                    Instant::now()
                ))
                .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);

        assert!(
            manager.flow.is_prebuffering,
            "the old 150ms cap ({old_cap_frames} frames) must no longer release the \
                 playhead: the threshold is 0.5 * target = {} (target={})",
            (manager.control.effective_target as f32 * 0.5) as u32,
            manager.control.effective_target,
        );

        // Keep feeding one frame per callback until it does release, and record the
        // depth it released at. The threshold is not read up-front on purpose:
        // `effective_target` is recomputed inside `fill_output`, so any
        // pre-computed threshold is stale by the time it is compared.
        let mut released_at = None;
        for i in (old_cap_frames + 1)..=(old_cap_frames * 4) {
            assert!(
                prod.try_push(make_packet(
                    &mut encoder,
                    seq + u64::from(i),
                    Instant::now()
                ))
                .is_ok()
            );
            manager.ingest_packets(&mut cons);
            let occupied = manager.buffer.occupied_count();
            manager.fill_output(&mut output, 1.0);
            if !manager.flow.is_prebuffering {
                released_at = Some(occupied);
                break;
            }
        }

        let released_at =
            released_at.expect("the hold must release once the buffer is deep enough");
        assert!(
            released_at > old_cap_frames,
            "the resume must require more than the removed 150ms cap: released at \
                 {released_at} frames, old cap {old_cap_frames}",
        );
    }

    /// `min_depth` is the outer floor: a link whose configured floor exceeds the
    /// target-scaled threshold resumes at the floor rather than into an immediate
    /// re-starve. Outlived the removal of the flat ceiling — the `.max(min_depth)`
    /// outlived the `.min()`.
    #[test]
    fn a_rebuffer_resume_should_not_drop_below_the_configured_minimum_depth() {
        let deep_floor_ms = 300;
        let config = JitterConfig {
            min_depth_ms: deep_floor_ms,
            comfort_cap_ms: 800,
            peak_decay_halflife_ms: 1000,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Unknown);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let min_depth = ms_to_frames_ceil(deep_floor_ms);

        // Precondition: the floor must be the binding term, or the test is
        // measuring `0.5 * target` and would pass with the floor deleted.
        assert!(
            ((manager.control.effective_target as f32 * 0.5) as u32) < min_depth,
            "precondition: 0.5 * target ({}) must be below min_depth ({min_depth})",
            (manager.control.effective_target as f32 * 0.5) as u32,
        );

        // One frame short of the floor: the hold must NOT release.
        for i in 1..min_depth {
            assert!(
                prod.try_push(make_packet(&mut encoder, u64::from(i), Instant::now()))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);
        assert!(
            manager.flow.is_prebuffering,
            "occupancy of {} frames is below min_depth ({min_depth}), so the \
                 playhead must stay held",
            min_depth - 1,
        );

        // Reaching the floor releases it.
        assert!(
            prod.try_push(make_packet(
                &mut encoder,
                u64::from(min_depth),
                Instant::now()
            ))
            .is_ok()
        );
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);
        assert!(
            !manager.flow.is_prebuffering,
            "min_depth must still release the playhead",
        );
    }

    /// The removal is a no-op on every low-latency link, and that is provable
    /// from the profiles rather than something to be taken on trust.
    ///
    /// The resume depth is `max(target * pct, min_depth)` and `target` can never
    /// exceed `comfort_cap_ms`, so the deepest resume a profile can ever reach is
    /// `comfort_cap_ms * resume_threshold_pct`. Where that product is at or below
    /// the removed 150ms ceiling, the ceiling was unreachable and the arithmetic is
    /// bit-identical before and after. Measured on the profiles: ADB/USB
    /// 100 × 0.2 = 20ms, Ethernet 200 × 0.25 = 50ms, 5GHz 400 × 0.25 = 100ms.
    ///
    /// Same shape as the `MIN_BAND` finding — a constant that only ever bound
    /// on links it was not aimed at. Only 2.4GHz (800 × 0.5 = 400ms) and Unknown
    /// (1000 × 0.25 = 250ms) could reach it, and 2.4GHz is the link it cost.
    #[test]
    fn a_low_latency_link_profile_should_resume_at_the_same_depth_as_before() {
        const REMOVED_CAP_MS: u32 = 150;

        for link in [
            NetworkLink::Adb,
            NetworkLink::UsbTether,
            NetworkLink::Ethernet,
            NetworkLink::Wifi5Ghz,
        ] {
            let config = JitterConfig::for_link_pair(LinkPair {
                phone: link,
                pc: link,
            });
            let deepest_resume_ms =
                (config.comfort_cap_ms as f32 * config.resume_threshold_pct) as u32;
            assert!(
                deepest_resume_ms <= REMOVED_CAP_MS,
                "{link:?}: comfort_cap {}ms * pct {} = {deepest_resume_ms}ms, which \
                     exceeds the removed {REMOVED_CAP_MS}ms cap — the removal is NOT a \
                     no-op on this link and its field behaviour changes",
                config.comfort_cap_ms,
                config.resume_threshold_pct,
            );
        }

        // The converse, so the assertion above cannot pass by the profiles all
        // having drifted shallow: 2.4GHz must still be able to exceed the old cap,
        // or the removal changed nothing anywhere.
        let noisy = JitterConfig::for_link_pair(LinkPair {
            phone: NetworkLink::Wifi2_4Ghz,
            pc: NetworkLink::Wifi2_4Ghz,
        });
        assert!(
            (noisy.comfort_cap_ms as f32 * noisy.resume_threshold_pct) as u32 > REMOVED_CAP_MS,
            "2.4GHz must be able to resume deeper than the removed cap, or the \
                 removal has no effect on the link it was measured on",
        );
    }

    /// **The arithmetic that made the old clamp wrong, stated as config algebra.**
    ///
    /// The rebuffer exit once flushed down to `unpause_threshold`, while
    /// the preemptive-expand trigger is `filtered < low_limit` with
    /// `low_limit = 0.75 * target`. Every Auto profile sets
    /// `resume_threshold_pct <= 0.5`, so the flush target is below the growth
    /// trigger *for every reachable target* — the resume was arming the actuator
    /// it had just made necessary. Field captures measured the consequence
    /// directly: **45 of 49 resumes landed below `low_limit`**, and 55.6% / 53.2%
    /// of all below-`low_limit` windows on the two 2.4GHz captures fall within 3s
    /// of one.
    ///
    /// Hardware-free and config-only, so it cannot rot with the actuators. The
    /// sweep runs the whole reachable target range per profile rather than one
    /// point, because `min_depth` dominates `unpause_threshold` at shallow targets
    /// and that is exactly where a single-point assertion would pass for the wrong
    /// reason.
    #[test]
    fn the_old_resume_depth_was_below_the_growth_trigger_on_every_link_profile() {
        for link in [
            NetworkLink::Adb,
            NetworkLink::UsbTether,
            NetworkLink::Ethernet,
            NetworkLink::Wifi5Ghz,
            NetworkLink::Wifi2_4Ghz,
            NetworkLink::WifiUnknown,
        ] {
            let config = JitterConfig::for_link_pair(LinkPair {
                phone: link,
                pc: link,
            });
            let min_depth = ms_to_frames_ceil(config.min_depth_ms);
            let cap = ms_to_frames_ceil(config.comfort_cap_ms);
            let mut below = 0;
            for target in min_depth..=cap {
                let unpause = ((target as f32 * config.resume_threshold_pct) as u32).max(min_depth);
                let Band { low, high } = TargetController::buffer_limits(target);
                // The new depth can never be below the band by construction.
                assert!(
                    high >= low,
                    "{link:?} target={target}: buffer_limits is inverted",
                );
                // ...nor below the release threshold, which is what the `.max` at
                // the call site states rather than assumes.
                assert!(
                    high >= unpause,
                    "{link:?} target={target}: band_hi {high} < release threshold \
                         {unpause}; the resume clamp would discard below the depth the \
                         release test just demanded",
                );
                if unpause < low {
                    below += 1;
                }
            }
            // The old depth, by contrast, is below the growth trigger across most
            // of the range — and always at the deep end, where the discard is
            // largest and the link is worst.
            let deepest_unpause =
                ((cap as f32 * config.resume_threshold_pct) as u32).max(min_depth);
            assert!(
                deepest_unpause < TargetController::buffer_limits(cap).low,
                "{link:?}: at the comfort cap the old resume depth {deepest_unpause} \
                     must be below low_limit {} — if it is not, this profile never had \
                     the defect and the sweep above proves nothing for it",
                TargetController::buffer_limits(cap).low,
            );
            assert!(
                below > 0,
                "{link:?}: no reachable target put the old resume depth below \
                     low_limit",
            );
        }
    }

    /// The rebuffer exit must land the buffer inside the
    /// controller's own operating band — the one depth where neither the drain nor
    /// the preemptive expand is armed — rather than at the release threshold.
    ///
    /// The field measured 6380ms of audio spliced away across 49 resumes on four
    /// captures, 45 of them landing below `low_limit`. Clamping to `high_limit`
    /// recomputes to 2240ms and 11/49 on the same events.
    #[test]
    fn the_rebuffer_exit_should_leave_the_buffer_inside_its_own_operating_band() {
        const TARGET: u32 = 40;
        let (mut manager, mut encoder, mut prod, mut cons, burst_at, burst_seq) =
            rebuffering_at_target(TARGET);
        let Band {
            low: low_limit,
            high: high_limit,
        } = TargetController::buffer_limits(TARGET);
        let unpause = ((TARGET as f32 * manager.config.resume_threshold_pct) as u32)
            .max(manager.min_depth_frames());
        assert!(
            unpause < low_limit,
            "precondition: the old resume depth ({unpause}) must be below \
                 low_limit ({low_limit}) at this target, or the two behaviours are \
                 indistinguishable here",
        );

        // A DTIM catch-up burst: far more than the band can hold, all at once.
        for i in 0..70u64 {
            assert!(
                prod.try_push(make_tone_packet_at(
                    &mut encoder,
                    burst_seq + i,
                    burst_at,
                    0.5
                ))
                .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let occupied_at_release = manager.buffer.occupied_count();
        assert!(
            occupied_at_release > high_limit,
            "precondition: the burst ({occupied_at_release}) must overshoot \
                 band_hi ({high_limit}), or nothing is clamped",
        );

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.control.effective_target = TARGET;
        manager.control.ramp_goal = TARGET;
        manager.fill_output(&mut output, 1.0);

        assert!(
            !manager.flow.is_prebuffering,
            "the playhead must be released"
        );
        assert_eq!(
            manager.log_window.tally.flush_discards,
            occupied_at_release - high_limit,
            "the clamp must discard exactly the overshoot above band_hi",
        );
        // One frame is played by this same callback.
        assert_eq!(
            manager.buffer.occupied_count(),
            high_limit - 1,
            "the resume must land at band_hi, not at the release threshold \
                 ({unpause})",
        );
        assert!(
            manager.buffer.occupied_count() >= low_limit,
            "the resume must not arm the growth actuator it just made necessary: \
                 left {} against low_limit {low_limit}",
            manager.buffer.occupied_count(),
        );
    }

    /// The regression side of the band clamp: an overshoot that is still inside
    /// the band is not an overshoot — nothing may be discarded.
    ///
    /// This is the case the old clamp got wrong on every single 2.4GHz resume:
    /// occupancy between `unpause_threshold` and `high_limit` was flushed down to
    /// the former, which is below `low_limit` by construction. Fails against the
    /// threshold clamp.
    #[test]
    fn the_rebuffer_exit_should_not_discard_audio_that_sits_below_the_high_limit() {
        const TARGET: u32 = 40;
        const BURST: u64 = 35;
        let (mut manager, mut encoder, mut prod, mut cons, burst_at, burst_seq) =
            rebuffering_at_target(TARGET);
        let Band {
            low: low_limit,
            high: high_limit,
        } = TargetController::buffer_limits(TARGET);
        let unpause = ((TARGET as f32 * manager.config.resume_threshold_pct) as u32)
            .max(manager.min_depth_frames());
        assert!(
            unpause < BURST as u32 && (BURST as u32) <= high_limit,
            "precondition: the burst ({BURST}) must sit strictly between the old \
                 resume depth ({unpause}) and band_hi ({high_limit}) — that is the \
                 band the threshold clamp threw away",
        );
        // Chosen above `low_limit` too, so the closing assertion measures the
        // clamp rather than the size of the burst: at exactly `low_limit` the one
        // frame this callback plays would put it under the band on its own.
        assert!(
            BURST as u32 > low_limit,
            "precondition: the burst ({BURST}) must clear low_limit ({low_limit}) \
                 by at least the frame this callback will play",
        );

        for i in 0..BURST {
            assert!(
                prod.try_push(make_tone_packet_at(
                    &mut encoder,
                    burst_seq + i,
                    burst_at,
                    0.5
                ))
                .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        assert_eq!(manager.buffer.occupied_count(), BURST as u32);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.control.effective_target = TARGET;
        manager.control.ramp_goal = TARGET;
        manager.fill_output(&mut output, 1.0);

        assert!(
            !manager.flow.is_prebuffering,
            "the playhead must be released"
        );
        assert_eq!(
            manager.log_window.tally.flush_discards,
            0,
            "nothing above band_hi arrived, so nothing may be discarded; the \
                 threshold clamp discarded {} frames here",
            BURST as u32 - unpause,
        );
        assert_eq!(
            manager.buffer.occupied_count(),
            BURST as u32 - 1,
            "only the frame this callback played may leave the buffer",
        );
        assert!(
            manager.buffer.occupied_count() >= low_limit,
            "the resume must land at or above low_limit ({low_limit}), got {}",
            manager.buffer.occupied_count(),
        );
    }

    /// The resume clamp must read the band at `raw_target`, not
    /// at the ramped `target`, so it never cuts the burst below the depth the
    /// stats say the link is currently demanding.
    ///
    /// `target` is rate-limited by `advance`, so on the callback that ends a
    /// rebuffer it still carries the pre-outage depth while `raw_target` has
    /// already absorbed the gap that caused the outage. Clamping to the stale
    /// number, the field measured the consequence directly: the landing
    /// depth was below the live `max_gap` on **93.9% (31/33)** of uncompressed
    /// resumes and **100% (8/8)** on 128kbps, short by 10.17 and 11.75 frames.
    /// Recomputed on those same 41 events, this change takes 128kbps to 4/8 at a
    /// 1.74-frame shortfall and cuts the discard from 660/850ms to 110/20ms. The
    /// margin is causal — `(landed - max_gap)` against lines-to-next-starvation is
    /// Spearman r=+0.570, p=0.0005, n=33.
    ///
    /// The target here is pinned *below* what the outage justifies, which is the
    /// whole point: at `TARGET` = 12 the ramped band tops out at 12, while the
    /// 30-frame gap the burst reports puts `gap_floor` — and so `raw_target` — at 31
    /// or above. Fails against the ramped clamp, which lands this buffer at 12.
    #[test]
    fn a_rebuffer_resume_should_not_clamp_below_the_measured_gap() {
        const TARGET: u32 = 12;
        /// The last setup arrival is at `base + 200ms`, so this is a 300ms outage.
        const OUTAGE: Duration = Duration::from_millis(500);
        const GAP_FRAMES: u32 = 30;
        let (mut manager, mut encoder, mut prod, mut cons, burst_at, burst_seq) =
            rebuffering_at_target_after(TARGET, OUTAGE);

        // A catch-up burst larger than either band, so both clamps fire and the
        // only thing under test is the depth they clamp *to*. Pushed before the
        // bands are read because this burst is also what *records* the outage:
        // `record_gap` is driven by the packet that ends a gap, so `max_gap` and
        // `raw_target` are only readable once it has been ingested — which is
        // exactly the order `fill_output` sees them in.
        for i in 0..70u64 {
            assert!(
                prod.try_push(make_tone_packet_at(
                    &mut encoder,
                    burst_seq + i,
                    burst_at,
                    0.5
                ))
                .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let occupied_at_release = manager.buffer.occupied_count();
        let raw_target = manager.target_breakdown(None).raw;
        let stale_high = TargetController::buffer_limits(TARGET).high;
        let Band {
            low: live_low,
            high: live_high,
        } = TargetController::buffer_limits(TARGET.max(raw_target));
        assert_eq!(
            manager.stats.max_gap_frames() as u32,
            GAP_FRAMES,
            "precondition: the outage must register as the gap it is, not the \
                 clamped or saturated value",
        );
        assert!(
            stale_high < GAP_FRAMES,
            "precondition: the ramped band ({stale_high}) must sit below the live gap \
                 ({GAP_FRAMES}), or this test cannot tell the two clamps apart",
        );
        assert!(
            live_high > stale_high,
            "precondition: the raw band ({live_high}) must exceed the ramped one \
                 ({stale_high})",
        );
        assert!(
            occupied_at_release > live_high,
            "precondition: the burst ({occupied_at_release}) must overshoot even \
                 the raw band ({live_high}), or nothing is clamped",
        );

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.control.effective_target = TARGET;
        manager.control.ramp_goal = TARGET;
        manager.fill_output(&mut output, 1.0);

        assert!(
            !manager.flow.is_prebuffering,
            "the playhead must be released"
        );
        // One frame is played by this same callback.
        assert_eq!(
            manager.buffer.occupied_count(),
            live_high - 1,
            "the resume must land at the band the *measurement* justifies \
                 ({live_high}), not at the ramped one ({stale_high})",
        );
        assert!(
            manager.buffer.occupied_count() + 1 >= manager.stats.max_gap_frames() as u32,
            "the landing depth ({}) must cover the gap the link is currently \
                 showing ({:.1}) — that shortfall is what fed the post-resume \
                 re-starvation",
            manager.buffer.occupied_count() + 1,
            manager.stats.max_gap_frames(),
        );
        assert!(
            manager.buffer.occupied_count() >= live_low,
            "the resume must not arm the growth actuator it just made necessary: \
                 left {} against low_limit {live_low}",
            manager.buffer.occupied_count(),
        );
    }

    /// The regression side: the clamp may only ever retain *more* than the ramped
    /// band did, never less.
    ///
    /// `raw_target` falls *below* the ramped target whenever `advance` is walking
    /// down behind a gap that has already aged out — 11 of 33 uncompressed
    /// resumes in a field capture. `buffer_limits(raw_target)` alone would clamp
    /// lower there, turning a fix for one half of the sample into a
    /// regression on the other. `buffer_limits(t).high` is monotone non-decreasing in
    /// `t`, so `max(target, raw_target)` makes the change provably one-directional.
    /// This pins the descent case that the `.max` exists for.
    #[test]
    fn a_rebuffer_resume_should_never_clamp_lower_than_the_ramped_band() {
        const TARGET: u32 = 40;
        /// A 100ms outage against the 200ms last setup arrival — an order of
        /// magnitude smaller than the depth the ramped target is still carrying.
        const OUTAGE: Duration = Duration::from_millis(300);
        let (mut manager, mut encoder, mut prod, mut cons, burst_at, burst_seq) =
            rebuffering_at_target_after(TARGET, OUTAGE);
        let high_limit = TargetController::buffer_limits(TARGET).high;

        for i in 0..70u64 {
            assert!(
                prod.try_push(make_tone_packet_at(
                    &mut encoder,
                    burst_seq + i,
                    burst_at,
                    0.5
                ))
                .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let occupied_at_release = manager.buffer.occupied_count();
        // The short outage leaves `raw_target` far below the ramped target — the
        // descent case. Read after ingest, since the burst is what records the gap.
        let raw_target = manager.target_breakdown(None).raw;
        assert!(
            raw_target < TARGET,
            "precondition: raw_target ({raw_target}) must sit below the ramped \
                 target ({TARGET}), or this is not the descent case",
        );
        assert!(
            occupied_at_release > high_limit,
            "precondition: the burst ({occupied_at_release}) must overshoot the \
                 band ({high_limit}), or nothing is clamped",
        );

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.control.effective_target = TARGET;
        manager.control.ramp_goal = TARGET;
        manager.fill_output(&mut output, 1.0);

        // What clamping on `raw_target` alone would have done here, spelled out so
        // the failure names the regression rather than just the mismatch. The release
        // threshold catches part of the cut, which is why the counterfactual landing
        // is not simply `buffer_limits(raw_target).high - 1`.
        let unpause = ((TARGET as f32 * manager.config.resume_threshold_pct) as u32)
            .max(manager.min_depth_frames());
        let raw_only_band = TargetController::buffer_limits(raw_target).high;
        assert_eq!(
            manager.buffer.occupied_count(),
            high_limit - 1,
            "with raw_target below target the clamp must still land at the ramped \
                 band ({high_limit}); clamping to buffer_limits(raw_target) = \
                 {raw_only_band} alone lands this buffer at {} instead",
            raw_only_band.max(unpause) - 1,
        );
    }

    /// **The resume threshold is a pure function of the target.** A
    /// `max_gap`-derived floor was once added here and the field test failed on both
    /// control links (ADB 62 → 110-189ms, 5GHz 51-65 → 200-244ms). Two arithmetic
    /// errors made that unavoidable, and this test encodes both:
    ///
    /// 1. The floor was read at *prebuffer exit*, not at starvation onset. The gap
    ///    window is arrival-driven — "the gap is recorded by the packet that ends
    ///    it" (`stats::record_gap`) — so by the time this code runs, the gap that
    ///    caused the rebuffer is already in the window. The floor was therefore
    ///    structurally ≥ that gap: 5GHz read 21 frames where 4 was predicted.
    /// 2. The clamp at the emergency-drain threshold bounded nothing, because
    ///    `target` is itself driven by the same `max_gap`. Router A ran away
    ///    4 → 18 → 72 frames (720ms) in ten seconds.
    ///
    /// A primed gap window is the *normal* state at this decision point, so both
    /// halves below prime one. Configs come from [`JitterConfig::for_link_pair`] so
    /// this tracks the shipped Auto profiles rather than a hand-copied snapshot.
    ///
    /// **This test is the boundary marker it was already implicitly acting as.**
    /// The *resume clamp* now reads `max(target, raw_target)`, which is a
    /// measurement-derived depth — the very shape the failed floor had. The
    /// distinction that makes the clamp legal and the floor illegal is exactly what
    /// this test pins: `unpause_threshold` decides *when* the playhead is released
    /// and must stay `max((target * pct) as u32, min_depth)`; the clamp decides only
    /// *how much of an already-released burst is kept*. A primed gap window below
    /// must therefore continue to leave the threshold untouched. If a future change
    /// wires a measurement into the release side, this test fails first.
    #[test]
    fn should_resume_prebuffer_at_the_target_fraction_regardless_of_the_measured_gap() {
        use crate::domain::types::LinkPair;

        // (link, pinned target, resume threshold = max((target * pct) as u32, min_depth))
        let cases = [
            (NetworkLink::Adb, 2u32, 3u32),
            (NetworkLink::Wifi5Ghz, 3, 3),
            (NetworkLink::Wifi2_4Ghz, 20, 10),
            (NetworkLink::WifiUnknown, 4, 3),
        ];

        // A virgin window (stream start) and a window primed with the 70.2-frame
        // gap Router A actually recorded must produce the same threshold.
        for primed_gap in [None, Some(70.2f32)] {
            for (link, target, expected) in cases {
                let config = JitterConfig::for_link_pair(LinkPair {
                    phone: link,
                    pc: link,
                });
                let (mut manager, mut encoder, mut prod, mut cons) = setup_env_with(config, link);
                let base_time = Instant::now();
                let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
                if let Some(gap) = primed_gap {
                    manager.stats.record_gap(gap, base_time);
                }

                // One frame short of the threshold: must still hold.
                for i in 1..expected as u64 {
                    assert!(
                        prod.try_push(make_loud_packet(&mut encoder, i, base_time))
                            .is_ok()
                    );
                }
                manager.ingest_packets(&mut cons);
                // Pinning `ramp_goal` alongside `effective_target` keeps `advance`
                // in its dead zone, so the callback sees the pinned target rather
                // than a mid-ramp value.
                manager.control.effective_target = target;
                manager.control.ramp_goal = target;
                manager.fill_output(&mut output, 1.0);
                assert!(
                    manager.flow.is_prebuffering,
                    "{link:?} (gap={primed_gap:?}): {} frames must not release a \
                         {expected}-frame threshold",
                    expected - 1,
                );

                // The threshold itself: must release, whatever the window holds.
                assert!(
                    prod.try_push(make_loud_packet(&mut encoder, expected as u64, base_time))
                        .is_ok()
                );
                manager.ingest_packets(&mut cons);
                manager.control.effective_target = target;
                manager.control.ramp_goal = target;
                manager.fill_output(&mut output, 1.0);
                assert!(
                    !manager.flow.is_prebuffering,
                    "{link:?} (gap={primed_gap:?}): the resume threshold must depend \
                         only on target, resume_threshold_pct and min_depth",
                );
            }
        }
    }
}

/// Crossing a hole: fast-forward, re-anchoring after a sender restart, the
/// reorder tolerance, and the fade-in on the far side.
mod gap_recovery {
    use super::*;

    #[test]
    fn should_fast_forward_past_large_udp_sequence_gap() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        // Loud packets: one callback must equal one frame here (see the note in
        // the PLC test) or the reorder-tolerance countdown can't be counted.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }
        // Simulate a massive 10 packet UDP loss! We inject sequence 15 into the buffer,
        // while the playhead is currently looking for sequence (MIN_DEPTH + 1).
        let future_seq = MIN_DEPTH as u64 + 10;
        assert!(
            prod.try_push(make_loud_packet(&mut encoder, future_seq, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);
        // 1st missing frame: we wait (gap_hold_count increments, PLC output)
        // After REORDER_TOLERANCE waits, advance_one() fires and playhead advances past the gap.
        // The gap is 10 slots wide (beyond distance>20 threshold for large-gap fast-forward)
        // so fast_forward fires after advance_one resolves missing count > threshold.
        for _ in 0..REORDER_TOLERANCE {
            manager.fill_output(&mut output, 1.0);
        }
        // After REORDER_TOLERANCE calls, advance_one was called and missing_count incremented.
        assert_eq!(manager.flow.missing_count, 0);
        assert!(!manager.flow.is_prebuffering);
    }

    #[test]
    fn should_fast_forward_without_decoder_reset_on_small_gaps() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        // Loud packets: one callback must equal one frame here (see the note in
        // the PLC test) or the reorder-tolerance countdown can't be counted.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }
        // Simulate a small 2 packet UDP loss.
        // We inject sequence (MIN_DEPTH + 3) into the buffer,
        // playhead expects (MIN_DEPTH + 1).
        let future_seq = MIN_DEPTH as u64 + 3;
        assert!(
            prod.try_push(make_loud_packet(&mut encoder, future_seq, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);

        // Wait for REORDER_TOLERANCE calls so gap_hold_count trips
        for _ in 0..REORDER_TOLERANCE {
            manager.fill_output(&mut output, 1.0);
        }

        // Now it should have fast-forwarded AND played the packet, so next_play_seq is future_seq + 1.
        assert_eq!(manager.buffer.next_play_seq(), future_seq + 1);
        // And the decoder state MUST be preserved (opus_next_expected_seq should not be None).
        // Since we waited REORDER_TOLERANCE frames, opus_next_expected_seq advanced via PLC!
        assert!(manager.decoder.opus_next_expected_seq.is_some());
    }

    #[test]
    fn should_recover_from_three_second_network_drop() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        // 1. Initial network fill
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }
        // 2. Network drop exceeding the link-aware timeout.
        // setup_env() uses NetworkLink::Unknown → max_missing = 2000ms / 10 = 200 frames.
        // We simulate 250 frames (2.5s) to comfortably exceed the threshold.
        for _ in 1..=250 {
            manager.fill_output(&mut output, 1.0);
        }
        // The manager must be heavily in prebuffering mode, waiting out the extreme lag
        assert!(manager.flow.is_prebuffering);
        // 3. Fresh batch arrives. ingest_packets directly inserts them (no flush).
        //    The jitter buffer is empty (starvation drained it), so it re-anchors at batch_start.
        let batch_start = MIN_DEPTH as u64 + 100;
        let batch_end = MIN_DEPTH as u64 + 250;
        for seq in batch_start..=batch_end {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        // 4. fill_output: exits prebuffering (151 packets >= 100 limit), then sees a large gap
        //    (batch_start - old_next_play ≫ 20 frames) → large-gap fast_forward fires immediately.
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);
        assert!(manager.buffer.next_play_seq() >= batch_start);
    }

    #[test]
    fn should_reanchor_playhead_on_sender_crash_restart() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        // 1. Initial network fill (e.g. sequence 1000..1005)
        let early_seq = 1000;
        for i in 0..MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, early_seq + i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 0..MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }
        // Assert we are playing around the 1000 mark!
        assert!(manager.buffer.next_play_seq() > 999);
        // 2. Android App force-crash and instantly restarts!
        // It starts sending sequence 0, 1, 2 again!
        for i in 0..MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        // The buffer physically detects that sequence 0 is > 128 packets BEHIND sequence 202.
        // It violently flushes its own timeline and re-anchors to 0!
        manager.fill_output(&mut output, 1.0);
        // The playhead must instantly snap back to 1!
        assert_eq!(manager.buffer.next_play_seq(), 1);
    }

    /// reorder tolerance is link-aware. Congested 2.4 GHz waits one extra
    /// ~30ms window for a straggler (fewer hole-skips → fewer fade-in splices);
    /// clean links keep the tight default; no-buffer mode never waits regardless.
    #[test]
    fn reorder_tolerance_should_widen_only_on_2_4ghz() {
        // 2.4 GHz gets the widened window.
        assert_eq!(
            reorder_tolerance_for(NetworkLink::Wifi2_4Ghz, false),
            REORDER_TOLERANCE_2_4GHZ
        );
        const { assert!(REORDER_TOLERANCE_2_4GHZ > REORDER_TOLERANCE) };

        // Clean / cable / unknown links keep the tight default.
        for link in [
            NetworkLink::Wifi5Ghz,
            NetworkLink::Ethernet,
            NetworkLink::Adb,
            NetworkLink::UsbTether,
            NetworkLink::WifiUnknown,
            NetworkLink::Unknown,
        ] {
            assert_eq!(
                reorder_tolerance_for(link, false),
                REORDER_TOLERANCE,
                "{link:?} should use the default reorder tolerance"
            );
        }

        // No-buffer mode never waits, even on 2.4 GHz.
        assert_eq!(reorder_tolerance_for(NetworkLink::Wifi2_4Ghz, true), 0);
        assert_eq!(reorder_tolerance_for(NetworkLink::Wifi5Ghz, true), 0);
    }

    /// When the playhead skips a hole (a missing slot with no reordered packet
    /// behind it), the next real frame is non-adjacent to what was just played.
    /// That raw splice clicks; marks the frame so it gets a 2ms linear
    /// fade-in. This asserts the flag is raised on an `advance_one` hole-skip and
    /// consumed (fade applied) on the next real frame.
    #[test]
    fn gap_skip_should_fade_in_next_frame() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Fill to MIN_DEPTH to exit prebuffering, then drain to steady state.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        // Drain everything currently buffered so the playhead sits on an empty
        // slot with a future packet available behind a hole.
        for _ in 0..MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }

        // Push a packet several sequence numbers ahead of the playhead, leaving a
        // hole at next_play_seq with no packet occupying the missing slot. This is
        // the `lowest_available_seq` fast-forward / advance_one hole-skip path.
        let hole_base = manager.buffer.next_play_seq();
        let future_seq = hole_base + 5;
        // A distinctly loud frame so the fade-in is measurable at sample 0.
        let loud = vec![0.5f32; OPUS_FRAME_SAMPLES];
        let d = encoder.encode_vec_float(&loud, 1500).unwrap();
        let mut pkt = RawPacket::zeroed();
        pkt.seq_num = future_seq;
        pkt.payload_data[..d.len()].copy_from_slice(&d);
        pkt.payload_len = d.len();
        pkt.arrival_time =
            base_time + std::time::Duration::from_millis(future_seq * MILLIS_PER_FRAME as u64);
        assert!(prod.try_push(pkt).is_ok());
        manager.ingest_packets(&mut cons);

        // Advance callbacks until the hole is skipped and the future frame plays.
        // The fade-in flag must be set the moment the hole is skipped, and cleared
        // once the next real frame is emitted with the fade applied.
        let mut saw_fadein_flag = false;
        for _ in 0..(REORDER_TOLERANCE + 4) {
            manager.fill_output(&mut output, 1.0);
            if manager.pending_gap_fadein {
                saw_fadein_flag = true;
            }
        }

        assert!(
            saw_fadein_flag || !manager.pending_gap_fadein,
            "gap-skip should have raised the fade-in flag at some point"
        );
        // After the future frame has played, the flag is consumed.
        assert!(
            !manager.pending_gap_fadein,
            "fade-in flag should be cleared once the next real frame is emitted"
        );
    }
}

/// The drain path — `accelerate`, its NCC and masking gates, and the emergency
/// tier above the high limit.
mod drain {
    use super::*;

    #[test]
    fn should_aggressively_flush_in_no_buffer_mode_without_starvation() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();

        // Enable No Buffer mode
        let mut no_buffer_cfg = test_config();
        no_buffer_cfg.static_target_ms = Some(0);
        no_buffer_cfg.min_depth_ms = 0; // The UI enforces this for No Buffer
        {
            let mut w = manager.config_ref.write().unwrap();
            *w = no_buffer_cfg;
        }
        // Force the config update tick
        manager.config_check_countdown = 100;

        let base_time = Instant::now();

        // Inject a 10 packet burst!
        for i in 1..=10 {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        // In No Buffer mode, target is 0. The config update path transiently
        // sets effective_target = max(min_depth_frames, 2) = 2, causing an
        // initial flush to 3 packets. After the static target override kicks in
        // (target = 0), silence fast-forward drains most remaining packets.
        // At most 1 packet may remain after a single fill_output call — it will
        // be consumed on the next call. The key invariant: no starvation.
        assert!(
            manager.buffer.occupied_count() <= 1,
            "Expected at most 1 packet remaining after aggressive flush, got {}",
            manager.buffer.occupied_count()
        );

        // It should not have starved, because it played packets.
        assert_eq!(manager.flow.starvation_count, 0);
    }

    #[test]
    fn flush_with_crossfade_should_produce_output_without_decoder_reset() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Fill buffer way beyond target.
        for i in 1..=100u64 {
            assert!(
                prod.try_push(make_packet(&mut encoder, i, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.flow.is_prebuffering = false;
        // Disable the startup flush so it doesn't drain the buffer before the
        // manual flush_with_crossfade call this test is exercising.
        manager.startup_flush_pending = false;

        // Decode one packet to populate decode_buf.
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);

        // Record opus_next_expected_seq before flush.
        let seq_before = manager.decoder.opus_next_expected_seq;

        // Manually flush down to 10 frames using the crossfade path.
        manager.flush_with_crossfade(10);

        // The decoder state should NOT have been hard-reset.
        // opus_next_expected_seq should still be Some (not None, which reset_state sets).
        assert!(
            manager.decoder.opus_next_expected_seq.is_some(),
            "opus_next_expected_seq should be Some after crossfade flush"
        );

        // The sequence should have advanced (decoder was fed through skipped frames).
        assert!(
            manager.decoder.opus_next_expected_seq > seq_before,
            "opus_next_expected_seq should have advanced past flushed frames"
        );

        // Buffer should be at or below the flush target.
        assert!(
            manager.buffer.occupied_count() <= 10,
            "Buffer should be <= 10 after flush, got {}",
            manager.buffer.occupied_count()
        );
    }

    /// the fast (emergency-drain) acceleration tier keys on the IIR-filtered
    /// buffer level, not instantaneous occupancy. A single transient burst — the
    /// characteristic delivery pattern of TCP/ADB — must NOT push the filtered level
    /// past the fast threshold, so the low-quality (NCC 0.5) crossfade does not fire
    /// on every burst (the cause of the constant electric/buzzy tone on ADB).
    #[test]
    fn transient_burst_should_not_engage_fast_drain_tier() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Reach steady state at MIN_DEPTH so a target is established.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        // Dump a large burst at once (TCP/ADB batching): instantaneous occupancy
        // jumps far past the fast threshold, but the IIR filter (α≈0.98-0.99) only
        // nudges the filtered level a little per callback.
        for seq in (MIN_DEPTH as u64 + 1..).take(40) {
            assert!(
                prod.try_push(make_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);

        let occupied_after_burst = manager.buffer.occupied_count();
        manager.fill_output(&mut output, 1.0);

        let target = manager
            .control
            .target_breakdown(&manager.config, &manager.stats, None)
            .raw;
        // The emergency (fast, no-cooldown, NCC-0.5) tier triggers at
        // `emergency_threshold(high_limit)` of the NetEQ decision band.
        let high_limit = TargetController::buffer_limits(target).high;
        let fast_threshold = emergency_threshold(high_limit);

        // The instantaneous burst is well past the fast threshold...
        assert!(
            occupied_after_burst > fast_threshold,
            "test precondition: burst ({occupied_after_burst}) should exceed fast threshold ({fast_threshold})"
        );
        // ...but the smoothed level the fast tier actually reads is still far below it,
        // so the emergency tier stays disengaged on a transient burst.
        assert!(
            manager.flow.filtered_buffer_level <= fast_threshold as f32,
            "filtered level ({:.1}) should stay <= fast threshold ({fast_threshold}) after a single burst",
            manager.flow.filtered_buffer_level,
        );
    }

    /// A *severe* overrun must drain even on LOUD audio: the emergency (fast) tier
    /// bypasses the cooldown and drops the correlation threshold 0.9 → 0.5, because
    /// at that point latency dominates a brief audible edit. This is the
    /// anti-plateau guard.
    ///
    /// The threshold it must cross is `emergency_threshold(high_limit)`, not the
    /// `4 × high` this test used to assert. Moderate overrun between `high` and
    /// that threshold is a *different* contract — see
    /// `moderate_loud_overrun_should_be_drained_not_tolerated`, which now
    /// requires the normal tier to drain it rather than tolerate it.
    #[test]
    fn loud_severe_overrun_should_emergency_drain() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        // Establish steady state at MIN_DEPTH with loud audio.
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        // Prime a large standing overrun with loud packets.
        let mut seq = MIN_DEPTH as u64 + 1;
        for _ in 0..60 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);

        // Run many callbacks while feeding one fresh loud packet per callback so
        // the stream never runs dry (rate-matched input). With a 60-packet standing
        // overrun the filtered level is driven past `emergency_threshold`, so the
        // emergency tier fires and drains it even though the audio is loud.
        // Track the peak filtered level so we can prove it did NOT balloon.
        let mut peak_filtered = manager.flow.filtered_buffer_level;
        for _ in 0..400 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
            peak_filtered = peak_filtered.max(manager.flow.filtered_buffer_level);
        }

        let target = manager
            .control
            .target_breakdown(&manager.config, &manager.stats, None)
            .raw;
        let high_limit = TargetController::buffer_limits(target).high;
        let fast_threshold = emergency_threshold(high_limit);

        // The emergency tier must have pulled a severe overrun back within the
        // fast-drain band.
        assert!(
            manager.flow.filtered_buffer_level <= fast_threshold as f32,
            "severe loud overrun should emergency-drain to within the fast \
                 threshold (<= {fast_threshold}), got {:.1}",
            manager.flow.filtered_buffer_level,
        );
        // And it must have drained well below the primed overrun peak — proving
        // the emergency drain actually fired on loud audio rather than holding flat.
        assert!(
            manager.flow.filtered_buffer_level < peak_filtered,
            "filtered level should settle below the overrun peak ({:.1}), got {:.1}",
            peak_filtered,
            manager.flow.filtered_buffer_level,
        );
        assert_eq!(
            manager.flow.starvation_count, 0,
            "must not starve while draining"
        );
    }

    /// The inverse of the guard this test used to be, and the reason it turned
    /// over.
    ///
    /// It formerly asserted that a MODERATE overrun (filtered between `high` and
    /// `4×high`) of LOUD audio must **not** time-stretch — loud overrun was
    /// tolerated and deferred to the next quiet moment. Three field rounds priced
    /// that contract: `ARTIFACT_MASK_RMS` is -22dBFS, program material sits above
    /// it essentially always, and the field census measured the consequence —
    /// `declined_rms_mask` on 93-96% of 36 455 drain attempts across three links,
    /// 243 splices in 881 seconds. "Deferred until a quiet moment" was in practice
    /// "never", so the buffer parked at 99ms against a 76ms target on ADB and
    /// starvation went 0 → 14 episodes on a cable.
    ///
    /// The quiet moment never arrives on music. So the drain must fire here, and
    /// the correlation gate inside `accelerate` — not the content's loudness — is
    /// what decides whether any individual splice happens.
    #[test]
    fn moderate_loud_overrun_should_be_drained_not_tolerated() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        let target = manager
            .control
            .target_breakdown(&manager.config, &manager.stats, None)
            .raw;
        let high_limit = TargetController::buffer_limits(target).high;

        // Prime a MODERATE overrun: above `high` so the drain band is entered, but
        // short of the emergency margin so this exercises the *normal* tier — the
        // one the masking gate used to close.
        let mut seq = MIN_DEPTH as u64 + 1;
        for _ in 0..high_limit + 4 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);
        let primed_occupancy = manager.buffer.occupied_count();

        // `filtered` is an α≈0.99 IIR, so it reads near zero until callbacks have
        // driven it toward the standing occupancy: band entry has to be observed
        // across the run, not from a reading taken before any callback.
        //
        // It also cannot be observed as a *level* at all. The drain debits the
        // filtered level from inside `fill_output` (NetEQ's BufferLevelFilter
        // time-stretch compensation), so the instant `filtered` crosses
        // `high_limit` a frame is removed and the level is knocked back under it.
        // Every value visible from out here is the end state of the previous
        // `fill_output`, which is post-drain by construction — measured peak 2.90
        // against a live limit of 3, never once at or above it. `peak >= high` is
        // unfalsifiable in the direction that matters: it fails precisely when the
        // drain is working.
        //
        // What IS observable is the branch's own effect. `tally.accelerated` is
        // incremented on exactly one path, and that path is reachable only through
        // `stretch_allowed && over_high` — so counting it counts band entries. It
        // is a 1Hz-windowed counter, so accumulate per-callback deltas rather than
        // reading it at the end; a delta that coincides with a window reset is
        // skipped, which can only under-count.
        //
        // Note also that the snapshot `high_limit` above is the *virgin-stats*
        // limit and drifts from the live one within a few callbacks (5 vs 3 here),
        // so the emergency bound is recomputed per callback from the manager's own
        // `effective_target` rather than from the snapshot.
        let stretches_before = manager.timescale.op_count();
        let mut peak_overrun: f32 = 0.0;
        let mut accelerations = 0u32;
        for _ in 0..200 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);

            let filtered = manager.flow.filtered_buffer_level;
            let live_high = TargetController::buffer_limits(manager.control.effective_target).high;
            peak_overrun = peak_overrun.max(filtered);
            // Hold the overrun inside the NORMAL tier. The emergency tier was
            // always exempt from the masking gate, so a drain up there would prove
            // nothing about the gate that was removed.
            assert!(
                filtered < emergency_threshold(live_high) as f32,
                "test precondition: the overrun must stay in the NORMAL tier \
                     (filtered={filtered:.1}, emergency at {})",
                emergency_threshold(live_high),
            );

            let accel_before = manager.log_window.tally.accelerated;
            manager.fill_output(&mut output, 1.0);
            if manager.log_window.tally.accelerated > accel_before {
                accelerations += manager.log_window.tally.accelerated - accel_before;
            }
        }

        assert!(
            accelerations > 0,
            "loud audio held above the high limit must be drained — a silent \
                 actuator here is the defect, not the guard (peak \
                 filtered={peak_overrun:.1})",
        );
        assert!(
            manager.timescale.op_count() > stretches_before,
            "an accelerate that never reaches the splice cannot drain",
        );
        assert_eq!(
            manager.log_window.tally.declined_rms_mask, 0,
            "the masking gate must no longer be able to decline a drain; a \
                 non-zero reading means it came back by some path",
        );
        assert!(
            manager.flow.filtered_buffer_level < peak_overrun,
            "draining must actually lower the buffer, not merely fire: peak was \
                 {:.1}, now {:.1}",
            peak_overrun,
            manager.flow.filtered_buffer_level,
        );
        // The IIR reading above is bounded by the drain that produced it, so it
        // moves very little. Occupancy is the legible proof: the primed overrun
        // has to be gone, not merely dented, under a rate-matched loud stream.
        assert!(
            manager.buffer.occupied_count() < primed_occupancy,
            "the standing overrun must be worked off, not held: primed {} frames, \
                 still holding {}",
            primed_occupancy,
            manager.buffer.occupied_count(),
        );
    }

    /// Three early field captures across ADB, 2.4GHz and 5GHz contain zero
    /// accelerate/expand/drain lines, because the timescale layer logs at
    /// `trace!` and the mobile crate installs `LevelFilter::Info`. The buffer
    /// was measurably parked above its high limit on ADB and the log could not
    /// say whether the drain never armed, was rate-limited, or was refused by
    /// the masking gate — three different defects with three different fixes.
    ///
    /// The census answered it (the mask, on 93-96% of attempts), which is why the
    /// mask is gone. What the census must still do is name the *remaining* reason
    /// a drain does not happen, and the rate limiter is now the only one: an
    /// undifferentiated "drain declined" counter would be no more use than the
    /// silence it replaced. Here the cooldown is engaged by a real splice and the
    /// immediately following callback must attribute its decline to that, with
    /// the retired mask reading zero.
    #[test]
    fn a_declined_drain_should_report_why_it_was_declined() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        let target = manager
            .control
            .target_breakdown(&manager.config, &manager.stats, None)
            .raw;
        let high_limit = TargetController::buffer_limits(target).high;

        // Moderate overrun: above `high` so the drain band is entered, below the
        // emergency margin so the masking gate still governs.
        let mut seq = MIN_DEPTH as u64 + 1;
        for _ in 0..high_limit + 2 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);

        // Fewer callbacks than LOG_INTERVAL_CALLBACKS, so the window this asserts on
        // is the one still open — the log line takes the tally when it fires.
        for _ in 0..50 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
        }

        assert!(
            manager.log_window.tally.declined_cooldown > 0,
            "a drain blocked by the rate limiter must be counted as such — with \
                 the masking gate retired, this is the only remaining decline reason \
                 and an unattributed one would restart the guessing",
        );
        assert!(
            manager.log_window.tally.accelerated > 0,
            "precondition: the cooldown can only be engaged by a splice that fired",
        );
        assert_eq!(
            manager.log_window.tally.declined_rms_mask, 0,
            "the masking gate is retired; a non-zero reading means it came back by \
                 some path and the drain is closed on music again",
        );
    }

    /// White noise has no pitch period, so the correlation search cannot find one
    /// and the splice must be refused — on loud content exactly as on quiet.
    ///
    /// This is what carries the quality guarantee now that the masking gate is not
    /// a precondition. The concern the gate was defending against (an audible edit
    /// on loud material) is real; the claim under test is that the correlation
    /// threshold, not the loudness, is what separates a transparent splice from an
    /// artifact. If loud unpitched content could splice freely, the demotion really
    /// would be the wholesale removal of psychoacoustic masking.
    ///
    /// Every declined attempt must still emit its staged window verbatim. A window
    /// staged and then dropped is a *deletion* — an audible gap, and worse than the
    /// splice it declined to make.
    #[test]
    fn a_loud_splice_that_fails_the_ncc_gate_must_still_emit_its_window_verbatim() {
        let mut ts = TimeScaler::new();

        // Loud, unpitched: a zero-mean LCG at 0.6 amplitude — an rms far above
        // ARTIFACT_MASK_RMS, so the old gate would have refused it outright. The
        // zero-mean part is load-bearing: a DC-offset noise source correlates
        // strongly with itself at every lag and would clear the gate for reasons
        // that have nothing to do with pitch.
        let mut seed = 0x1234_5678u32;
        let mut noise_frame = |amp: f32| {
            let mut pcm = vec![0.0f32; OPUS_FRAME_SAMPLES];
            for s in pcm.iter_mut() {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *s = ((seed >> 8) as f32 / 8_388_608.0 - 1.0) * amp;
            }
            pcm
        };
        // The two halves must be *independent* draws. Staging the same buffer twice
        // would correlate perfectly with itself at a one-frame lag and clear the
        // gate for a reason that has nothing to do with the content being splicable.
        let first = noise_frame(0.6);
        let second = noise_frame(0.6);
        let rms = JitterBufferManager::get_rms(&first);
        assert!(
            rms > ARTIFACT_MASK_RMS,
            "test precondition: the sample must be loud enough that the retired \
                 gate would have refused it, got rms={rms:.4}",
        );

        let mut window = ts.window_begin(&first);
        assert!(window.extend(&second));
        let mut playback = VecDeque::new();
        let spliced = window.accelerate(false, rms, &mut playback);
        assert!(
            spliced.is_none(),
            "loud white noise has no pitch period — the correlation gate must \
                 refuse it, or the quality guarantee has moved to nothing",
        );

        let staged = window.staged().len();
        window.emit(&mut playback);
        assert_eq!(
            playback.len(),
            staged,
            "a declined splice must emit its staged window verbatim — dropping it \
                 is a deletion, which is worse than the splice it refused",
        );
    }

    /// The masking gate must not be reachable as a veto from any path on the drain
    /// branch — including the one that used to be its sole exemption.
    ///
    /// `moderate_loud_overrun_should_be_drained_not_tolerated` proves the normal
    /// tier drains loud content. This proves the *reason*: that the loudness test
    /// no longer participates in the decision at all. The distinction matters
    /// because the emergency tier could mask a still-closed normal tier — the
    /// buffer would drain in the field and the census would look healthy while the
    /// defect (only draining at 227ms+) survived untouched. So the overrun here is
    /// held deliberately inside the normal tier.
    #[test]
    fn the_masking_gate_must_not_be_the_sole_veto_on_the_drain_path() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        let target = manager
            .control
            .target_breakdown(&manager.config, &manager.stats, None)
            .raw;
        let high_limit = TargetController::buffer_limits(target).high;

        let mut seq = MIN_DEPTH as u64 + 1;
        for _ in 0..high_limit + 2 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);

        for _ in 0..120 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
            assert!(
                manager.flow.filtered_buffer_level < emergency_threshold(high_limit) as f32,
                "test precondition: the overrun must stay in the NORMAL tier — the \
                     emergency tier was always exempt from the mask and would prove \
                     nothing here (filtered={:.1}, emergency at {})",
                manager.flow.filtered_buffer_level,
                emergency_threshold(high_limit),
            );
        }

        assert!(
            manager.log_window.tally.accelerated > 0,
            "the normal tier must drain loud content on its own, without the \
                 emergency tier's exemption",
        );
        assert!(
            manager.log_window.tally.loud_splices > 0,
            "those splices landed on content the retired gate would have refused, \
                 and the census must say so — this is the population any field-reported \
                 warble has to be weighed against",
        );
        assert_eq!(
            manager.log_window.tally.declined_rms_mask, 0,
            "the loudness test must not participate in the drain decision at all",
        );
    }

    /// The emergency tier must be a *fraction* of the drain limit, not a constant
    /// added to it.
    ///
    /// Both prior forms failed the same way. `4 * high_limit` was ~2 seconds of
    /// audio and was removed for never firing; `high_limit + 15` replaced it and
    /// the field census says it did not fire either — 0.0% / 0.2% / 0.0% of windows
    /// on ADB / 5GHz / 2.4GHz, against thresholds of 22.7 / 24.3 / 49.8 frames and
    /// filtered levels parked at 9.12 / 9.86 / 18.79.
    ///
    /// The direction is what this test pins: a flat margin is a *smaller* fraction
    /// of a large target than of a small one, so the tier gets relatively harder to
    /// reach exactly as the target grows. The threshold must therefore rise with
    /// `high_limit` — and, above the floor, rise strictly *proportionally*, so the
    /// ratio does not drift across the 5-to-50-frame range these links span.
    #[test]
    fn the_emergency_drain_threshold_should_scale_with_the_high_limit() {
        // Above the floor the ratio is exactly 1.5× on every limit.
        for high in [10u32, 14, 20, 32, 52] {
            assert_eq!(
                emergency_threshold(high),
                high + high / 2,
                "above the floor the threshold must be 1.5×high, not a constant offset",
            );
        }

        // Strictly monotonic: a larger drain limit can never be easier to escape.
        let mut prev = 0;
        for high in 1u32..=64 {
            let t = emergency_threshold(high);
            assert!(
                t > prev || high == 1,
                "threshold must not fall as high_limit rises ({high}: {t} after {prev})",
            );
            assert!(
                t > high,
                "the emergency tier must sit strictly above the normal one \
                     ({high}: {t})",
            );
            prev = t;
        }
    }

    /// ADB's measured target is ~5 frames, so `high_limit` is ~5-7 and the old flat
    /// 150ms margin put the emergency tier at 20-22 frames — 200-220ms of latency
    /// on a *cable*, which is the "buffer jumps to 250ms++" report. The tier has to
    /// be reachable before a quarter second on the links that have no excuse for
    /// one.
    #[test]
    fn a_small_target_should_reach_the_emergency_tier_before_a_quarter_second_of_latency() {
        for target in 3u32..=8 {
            let high = TargetController::buffer_limits(target).high;
            let threshold_ms = emergency_threshold(high) * MILLIS_PER_FRAME;
            assert!(
                threshold_ms < 250,
                "target {target} (high={high}) puts the emergency tier at \
                     {threshold_ms}ms — past the latency the field reported as a defect",
            );
        }
    }

    /// The floor is load-bearing in the other direction. `high_limit / 2` alone
    /// collapses into the normal tier's own dead-band at small limits — at
    /// `high = 3` it is 1 frame, so ordinary oscillation would promote to the
    /// riskier 0.5-correlation splice instead of letting the 0.9 tier try first.
    #[test]
    fn a_large_target_should_not_enter_the_emergency_tier_on_ordinary_overshoot() {
        // The normal tier's dead-band is `max(target/4, MIN_BAND)`; the emergency
        // tier must sit clear of it at every target, so a single band's worth of
        // overshoot is handled by the 0.9-correlation path.
        for target in 2u32..=64 {
            let Band { low, high } = TargetController::buffer_limits(target);
            let band = high - low;
            assert!(
                emergency_threshold(high) > high + band,
                "target {target} (low={low}, high={high}, band={band}): one \
                     dead-band of overshoot must not reach the emergency tier",
            );
        }

        // And at the small end the floor, not the ratio, is what provides that
        // clearance — assert the floor is actually the binding term there.
        let small_high = TargetController::buffer_limits(4).high;
        assert!(
            small_high / 2 < EMERGENCY_MIN_MARGIN,
            "test premise: at a small target the proportional term should be \
                 below the floor (high={small_high})",
        );
        assert_eq!(
            emergency_threshold(small_high),
            small_high + EMERGENCY_MIN_MARGIN,
        );
    }
}

/// The growth path — `expand` below the low limit, its rate limit, and the
/// history it splices from.
mod preemptive_growth {
    use super::*;

    /// **Click-train regression, re-pointed at the rate limit.** The field report
    /// was "on every buffer increase I heard fast-clicking noise artifacts".
    /// Cause: any callback with the filtered level below `low_limit` went straight
    /// to WSOLA `expand`, which inserts exactly one pitch period per call. At the
    /// shared 60ms cooldown a raised target produced a ~17Hz train of OLA splices
    /// for seconds on end.
    ///
    /// That was first answered by making expand an imminent-underrun defence only,
    /// so this test asserted the op count stayed *exactly flat* below target.
    /// Upstream's growth trigger (`filtered < low_limit`,
    /// `decision_logic.cc:294-295`) was then ported precisely because that was too
    /// strict — the field measured 2.4GHz sitting 15.7 frames below target in 85% of
    /// screen-off windows with arrivals matching playback, so the buffer had no
    /// way to reach its own target except by starving first (16 → 57 episodes).
    ///
    /// So the zero-splice assertion is gone, but the contract it protected is not:
    /// what makes a splice train audible is **density**, not existence. This now
    /// asserts the band, from both sides — growth happens (the mechanism the trigger
    /// adds) and it stays under `MIN_EXPAND_INTERVAL` (the click train it must not
    /// become). 300 callbacks at one splice per 20 gives a ceiling of 15.
    ///
    /// A ceiling alone would pass vacuously if expand never fired at all, hence the
    /// lower bound.
    #[test]
    fn below_target_growth_should_stay_rate_limited_on_a_healthy_buffer() {
        // A 150ms floor puts `low_limit` at 11 frames — far above the occupancy we
        // hold — without needing to fake any controller state.
        let config = JitterConfig {
            min_depth_ms: 150,
            comfort_cap_ms: 400,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi2_4Ghz);
        let base = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut seq = 1u64;

        // Prebuffer, then drain down to a modest occupancy. `filtered` starts at
        // zero and climbs slowly, so it stays well under `low_limit` throughout.
        for _ in 0..16 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.03))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..10 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.flow.is_prebuffering);

        // Steady state: one packet in, one frame out — permanently below target,
        // never near empty.
        let stretches_before = manager.timescale.op_count();
        let mut saw_below_target = false;
        for _ in 0..300 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.03))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
            let low = TargetController::buffer_limits(manager.control.effective_target).low;
            if manager.flow.filtered_buffer_level < low as f32
                && manager.buffer.occupied_count() > 1
            {
                saw_below_target = true;
            }
        }
        assert!(
            saw_below_target,
            "test never reached the below-target-but-healthy state it exists to cover",
        );
        let splices = manager.timescale.op_count() - stretches_before;
        assert!(
            splices > 0,
            "preemptive growth never fired across 300 below-target callbacks — the \
                 buffer has no way to reach its target except by starving, which is the \
                 measured defect (2.4GHz: 15.7 frames below target, 85% of windows)",
        );
        assert!(
            splices <= 300 / MIN_EXPAND_INTERVAL as usize,
            "{splices} splices in 300 callbacks exceeds one per {MIN_EXPAND_INTERVAL} — \
                 this is the ~17Hz click train returning by the growth path",
        );
    }

    /// The loud-content half of the same contract, and the one that can regress by
    /// a route the quiet test cannot see. `expand` has a VAD escape past
    /// its NCC gate on near-silent windows; loud material stays fully gated, so
    /// this pins the density bound on the population where the *correlation* check
    /// is the only quality control standing.
    ///
    /// Same rewrite as its sibling — it asserted zero splices and now asserts the
    /// rate limit — for the reasons documented on
    /// [`below_target_growth_should_stay_rate_limited_on_a_healthy_buffer`].
    #[test]
    fn below_target_growth_should_stay_rate_limited_on_loud_content() {
        // A 150ms floor puts `low_limit` at 11 frames, far above the occupancy held
        // below, so every callback is a below-target one.
        let config = JitterConfig {
            min_depth_ms: 150,
            comfort_cap_ms: 400,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi2_4Ghz);
        let base = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut seq = 1u64;

        for _ in 0..16 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.5))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..10 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.flow.is_prebuffering);

        let stretches_before = manager.timescale.op_count();
        let mut saw_below_target = false;
        for _ in 0..300 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.5))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
            let low = TargetController::buffer_limits(manager.control.effective_target).low;
            if manager.flow.filtered_buffer_level < low as f32
                && manager.buffer.occupied_count() > 1
            {
                saw_below_target = true;
            }
        }

        assert!(
            saw_below_target,
            "test never reached the below-target-but-healthy state it exists to cover",
        );
        let splices = manager.timescale.op_count() - stretches_before;
        assert!(
            splices > 0,
            "preemptive growth never fired on loud content — the NCC gate is \
                 refusing every splice, so the growth path is inert on exactly the \
                 material it will meet in the field",
        );
        assert!(
            splices <= 300 / MIN_EXPAND_INTERVAL as usize,
            "{splices} splices in 300 callbacks exceeds one per {MIN_EXPAND_INTERVAL} — \
                 the click train returning by a new route",
        );
    }

    /// **The convergence claim, and the reason the growth geometry changed.**
    ///
    /// The field measured target step-ups that *never* closed: 2.4GHz sat
    /// 8.13-11.84 frames below target in 70-83% of windows with arrivals matching
    /// playback, because the growth actuator moved 0.059-0.434 fr/s against the
    /// 2.6-5.6 fr/s the climbs needed. The buffer had no route to its own target
    /// except starving first.
    ///
    /// Arrivals are rate-matched to playback here — one packet in, one frame out —
    /// so the *only* thing that can raise the buffer is the actuator. Under the old
    /// geometry the level stays flat and this fails.
    ///
    /// Asserted on `filtered_buffer_level` rather than on `occupied_count`,
    /// because `expand` inserts duration into `playback_buf`, which delays the
    /// next pop rather than adding a packet. `adjust_filtered_level` is where that
    /// insertion is accounted, and it is the reading the depth controller acts on.
    #[test]
    fn a_target_step_up_should_converge_when_arrivals_match_playback() {
        let config = JitterConfig {
            min_depth_ms: 150,
            comfort_cap_ms: 400,
            peak_decay_halflife_ms: 1000,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi2_4Ghz);
        let base = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut seq = 1u64;

        for _ in 0..40 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.03))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..8 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.flow.is_prebuffering);

        // Force the config poll so `min_depth_ms` reaches the controller — it is
        // re-read only every 100 callbacks, and without it the band is degenerate.
        // Rate-match during convergence so the buffer never drains here: a single
        // starvation would arm the recovery window, and with wall-clock frozen in
        // tests it never expires — disarming both actuators for the rest of the
        // run. Never drain past `occ=1` in a test.
        manager.config_check_countdown = 100;
        for _ in 0..12 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.03))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
        }

        // Draw the buffer down to a healthy below-band level — several frames, so
        // it is well under `low_limit` but never starves. This is the step-up
        // precondition: occupancy sits below the band the raised target opened,
        // and only the actuator can lift it back.
        let mut guard = 0;
        while manager.buffer.occupied_count() > 3 {
            manager.fill_output(&mut output, 1.0);
            guard += 1;
            assert!(guard < 64, "drain did not converge");
        }
        assert!(
            manager.buffer.occupied_count() > 1,
            "the drain must leave a non-starving buffer, or the recovery window \
                 arms and freezes the actuator for the rest of the run",
        );
        assert_eq!(
            manager.flow.starvation_count, 0,
            "a starvation here would arm the recovery window and disarm both \
                 actuators for the rest of the run",
        );

        let low_limit = TargetController::buffer_limits(manager.control.effective_target).low;
        assert!(
            low_limit > 1,
            "a degenerate band ({low_limit}) leaves no step-up to converge",
        );
        let start_level = manager.flow.filtered_buffer_level;
        assert!(
            start_level < low_limit as f32,
            "the step-up precondition must stand: filtered {start_level:.2} against \
                 low_limit {low_limit}",
        );
        manager.timescale_cooldown = 0;

        // Rate-matched: exactly one packet in per callback, so arrivals can never
        // fill the deficit. Only the actuator can.
        let mut inserted = 0.0f32;
        for _ in 0..200 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.03))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            let before = manager.log_window.tally.inserted_frames;
            manager.fill_output(&mut output, 1.0);
            // `TimescaleTally` is 1Hz-windowed and resets mid-run, so a
            // plain delta silently discards every insertion across a reset
            // boundary. A negative delta means the window rolled: the current
            // value is itself the post-reset accumulation.
            let delta = manager.log_window.tally.inserted_frames - before;
            inserted += if delta >= 0.0 {
                delta
            } else {
                manager.log_window.tally.inserted_frames
            };
        }

        assert!(
            inserted > 0.0,
            "the actuator never inserted anything across 200 rate-matched \
                 below-band callbacks — this is the inert-actuator defect verbatim",
        );
        assert!(
            manager.flow.filtered_buffer_level > start_level,
            "filtered level did not rise ({start_level:.2} -> {:.2}) despite \
                 {inserted:.2} frames inserted — growth that does not reach the depth \
                 controller cannot close a step-up",
            manager.flow.filtered_buffer_level,
        );
        assert_eq!(
            manager.flow.starvation_count, 0,
            "convergence must not be bought by starving first — that was the old \
                 geometry's only route to target",
        );
    }

    /// **The emission contract that keeps `process_next_frame` reachable.**
    ///
    /// `fill_output` only calls `process_next_frame` when `playback_buf` is empty,
    /// and `process_next_frame` is where ingest, depth control, the drain and the
    /// static flush all live. So any splice that parks a whole frame of surplus
    /// suspends the entire control layer for the following callback — which is
    /// exactly what the rejected future-staging draft did (it emitted
    /// `2 * frame + period` and measured a static 60ms target parking at 25
    /// frames, because the static flush never ran).
    ///
    /// History staging emits `frame + period` instead, so each splice adds **one
    /// pitch period** of surplus rather than a frame plus a period. Surplus does
    /// accumulate — inserting duration is the whole point of the actuator — but it
    /// sheds a frame each time it crosses one, so the control layer is deferred
    /// once per `frame / period` splices instead of once per splice.
    ///
    /// Both halves are asserted: the per-splice increment (the mechanism) and the
    /// resulting skip rate (the consequence). A ceiling on standing surplus alone
    /// would be the wrong assertion — it would fail precisely when the actuator
    /// works.
    ///
    /// Measured per callback rather than at the end, because `playback_buf` is
    /// drained by the next `fill_output`. `tally.expanded` is 1Hz-windowed, so the
    /// trigger precondition is read as a delta.
    #[test]
    fn preemptive_expand_should_leave_at_most_one_pitch_period_of_surplus() {
        let config = JitterConfig {
            min_depth_ms: 150,
            comfort_cap_ms: 400,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi2_4Ghz);
        let base = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut seq = 1u64;

        for _ in 0..16 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.03))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..10 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.flow.is_prebuffering);

        let mut splices = 0u32;
        let mut max_increment = 0usize;
        let mut skipped_callbacks = 0u32;
        for _ in 0..300 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.03))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            let surplus_before = manager.playback_buf.len();
            if surplus_before >= OPUS_FRAME_SAMPLES {
                skipped_callbacks += 1;
            }
            let before = manager.log_window.tally.expanded;
            manager.fill_output(&mut output, 1.0);
            if manager.log_window.tally.expanded > before {
                splices += 1;
                // Exactly one `process_next_frame` ran (a splice emits more than one
                // frame, so the fill loop cannot re-enter), hence
                // `emitted = surplus_after + OPUS_FRAME_SAMPLES - surplus_before`
                // and the inserted audio is the surplus delta.
                max_increment = max_increment.max(manager.playback_buf.len() - surplus_before);
            }
        }

        assert!(
            splices > 0,
            "no splice fired across 300 below-target callbacks — an emission ceiling \
                 passes vacuously when the mechanism never runs",
        );
        // `expand` correlates over `[history | frame]` = 960 sample-frames and puts
        // the reference at `n - OLA_LEN`, so the candidate start index runs
        // `0..min(SEARCH_RANGE, anchor - OLA_LEN)` and the period `anchor - d`
        // reaches at most the anchor itself.
        let anchor_frames = 2 * OPUS_FRAME_SAMPLES / OPUS_CHANNELS as usize - OLA_LEN;
        let max_period_samples = anchor_frames * OPUS_CHANNELS as usize;
        assert!(
            max_increment <= max_period_samples,
            "a splice added {max_increment} samples of surplus, beyond the largest \
                 reachable pitch period ({max_period_samples}) — an increment of a whole \
                 frame plus a period is the future-staging signature, and it suspends \
                 `process_next_frame` on every splice instead of on every few",
        );
        // Surplus sheds a frame whenever it crosses one, so the control layer is
        // deferred at most once per `frame / period` splices rather than once per
        // splice. Bounding the skip rate is what makes the increment bound matter.
        assert!(
            skipped_callbacks < 300 / 4,
            "{skipped_callbacks} of 300 callbacks served entirely from surplus and \
                 skipped `process_next_frame`, where ingest, depth control, the drain \
                 and the static flush live",
        );
    }

    /// **The imminent-underrun tier must survive an empty buffer.**
    ///
    /// This is the tier future-staging would have designed out of existence: with
    /// `occupied <= 1` there is no next packet to stage, so a `has_next()`-guarded
    /// expand degrades to a single frame precisely when growth matters most. The
    /// field captures put 95-98% of 2.4GHz's growth deficit in this tier.
    ///
    /// History is always available, so the widened geometry applies here too.
    ///
    /// Note the tier is *nested inside* `filtered < low_limit`, not an independent
    /// trigger — `imminent_underrun` only escalates urgency within the below-band
    /// branch. So the setup must put the buffer below the band **and** at one
    /// frame, and both preconditions are asserted rather than assumed.
    ///
    /// `peak_decay_halflife_ms` is non-zero on purpose: zero with no static target
    /// is the "Auto" sentinel, and `for_link_pair` would then replace the config
    /// wholesale with the 2.4GHz profile, collapsing `min_depth_ms` and leaving a
    /// degenerate band with `low_limit = 1` that the buffer never falls below.
    #[test]
    fn preemptive_expand_should_still_fire_when_the_buffer_has_no_next_packet() {
        let config = JitterConfig {
            min_depth_ms: 150,
            comfort_cap_ms: 400,
            peak_decay_halflife_ms: 1000,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi2_4Ghz);
        let base = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut seq = 1u64;

        for _ in 0..40 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.03))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..8 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.flow.is_prebuffering);

        // The config is re-read only every 100 callbacks, so `min_depth_ms` would
        // otherwise never reach the controller inside this test and the band would
        // stay degenerate. Force the poll, then let the target converge while
        // arrivals keep pace so the buffer does not drain during convergence.
        manager.config_check_countdown = 100;
        for _ in 0..12 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.03))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
        }

        // Drain to exactly one frame without ever starving.
        let mut guard = 0;
        while manager.buffer.occupied_count() > 1 {
            manager.fill_output(&mut output, 1.0);
            guard += 1;
            assert!(guard < 64, "drain did not converge");
        }
        assert_eq!(
            manager.buffer.occupied_count(),
            1,
            "setup must leave exactly one frame so `imminent_underrun` stands",
        );
        assert_eq!(
            manager.flow.starvation_count, 0,
            "a starvation here would arm the recovery window and disarm both \
                 actuators for the rest of the run",
        );
        let low_limit = TargetController::buffer_limits(manager.control.effective_target).low;
        assert!(
            low_limit > 1,
            "a degenerate band ({low_limit}) makes the below-band branch unreachable, \
                 and the tier under test lives inside it",
        );
        assert!(
            manager.flow.filtered_buffer_level < low_limit as f32,
            "the below-band precondition must stand: filtered {:.2} against low_limit \
                 {low_limit}",
            manager.flow.filtered_buffer_level,
        );

        // The last frame is served across however many callbacks the parked
        // surplus spans. `expand` fires from inside `process_next_frame`, so the
        // delta is accumulated rather than read after a single callback — and
        // `imminent_underrun` is confirmed to have actually stood, so the
        // assertion cannot pass on a preemptive splice instead.
        manager.timescale_cooldown = 0;
        let before = manager.log_window.tally.expanded;
        let mut saw_imminent = false;
        for _ in 0..3 {
            if manager.buffer.occupied_count() <= 1 {
                saw_imminent = true;
            }
            manager.fill_output(&mut output, 1.0);
        }
        assert!(saw_imminent, "the underrun precondition never stood");
        assert!(
            manager.log_window.tally.expanded > before,
            "expand refused at `occupied <= 1` — this is the tier that prevents \
                 starvation, and 95-98% of the measured growth deficit sits in it",
        );
    }

    /// **A discontinuity must drop the splice history.**
    ///
    /// `expand` correlates the current frame against the previously *emitted* one.
    /// After a starvation fade-in, a hole skip, a silence fast-forward shed, or a
    /// reset, the two are no longer adjacent in the source, so a splice would
    /// replay audio that never played there. `forget()` covers those points.
    ///
    /// Asserted through the manager rather than on the private field so the test
    /// pins the wiring, not the implementation: a reset must leave the history
    /// empty, and the geometry must recover on the next decoded frame.
    #[test]
    fn expand_history_should_be_dropped_across_a_discontinuity() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

        for seq in 1..=12u64 {
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, base, 0.03))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..6 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(
            manager.timescale.has_history(),
            "the decode path must remember every frame, not only the ones that \
                 splice — the 20-callback rate limit would otherwise leave history \
                 200ms stale and destroy the correlation",
        );

        manager.trigger_reset();
        assert!(
            !manager.timescale.has_history(),
            "a reset discards the emitted stream, so the remembered frame is no \
                 longer adjacent to whatever plays next",
        );

        // One decoded frame restores it, so the narrowed geometry lasts exactly
        // one callback.
        for seq in 20..=32u64 {
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, base, 0.03))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..8 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(
            manager.timescale.has_history(),
            "history must come back on the first decoded frame after a reset",
        );
    }

    /// The inversion of expand's energy gate, and the reason it turned over.
    ///
    /// Expand used to require `(SILENCE_RMS..ARTIFACT_MASK_RMS).contains(&rms)` on
    /// top of `occupied <= 1`. The field census measured the cost: **7 expands in
    /// 881 seconds** across all three links, because program material does not sit
    /// in that band. The underrun defence was therefore unavailable at exactly the
    /// moments it exists for, and the buffer starved instead — ADB 0 → 14 episodes
    /// on a cable.
    ///
    /// Starvation runs `generate_plc`, which does not fade until
    /// `conceal_run > 3`, so the first three frames of every episode are raw
    /// extrapolation; 50% of ADB's episodes were ≤30ms, entirely inside that band.
    /// The trade is not "a splice vs. nothing" — it is one NCC-0.9-gated pitch
    /// period against a whole frame of unfaded PLC.
    #[test]
    fn expand_should_defend_an_imminent_underrun_on_loud_content() {
        // A 150ms floor keeps `low_limit` well above the occupancy held below, the
        // same way the click-train test does it. `setup_env`'s 40ms config lets the
        // target converge to 2 frames, where `low_limit` is 1 and the band is too
        // degenerate to say anything about the trigger.
        let config = JitterConfig {
            min_depth_ms: 150,
            comfort_cap_ms: 400,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi2_4Ghz);
        let base = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut seq = 1u64;

        // Amplitude 0.5 gives rms ≈ 0.35, far above the retired `ARTIFACT_MASK_RMS`
        // gate — this is the content the old clause refused.
        for _ in 0..16 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.5))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..10 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.flow.is_prebuffering);

        // Drain down to the trigger, then hold it there with an exactly
        // rate-matched loud trickle: one packet in, one frame out, so every callback
        // ends one frame from empty without ever starving.
        //
        // Draining *past* one frame would starve, and a starvation arms the 500ms
        // recovery window that holds `stretch_allowed` off. Wall-clock does not
        // advance meaningfully across a test's callbacks, so that window would
        // never expire and the defence could not fire for the rest of the run.
        while manager.buffer.occupied_count() > 1 {
            manager.fill_output(&mut output, 1.0);
        }
        assert_eq!(
            manager.flow.starvation_count, 0,
            "test precondition: the drain-down must not starve",
        );

        // `tally.expanded` is a 1Hz-windowed counter and this loop spans two
        // windows, so accumulate per-callback deltas rather than reading it at the
        // end. A delta that coincides with a window reset is skipped, which can
        // only under-count.
        //
        // The trigger is sampled *after* `fill_output`, not after `ingest_packets`.
        // The gate reads `occupied_count()` from inside the callback, after this
        // callback's own frame has already been popped — so from out here the state
        // it saw is the post-fill one. Sampling post-ingest reads one frame higher
        // and never sees the trigger at all.
        let mut expands = 0u32;
        let mut saw_trigger = false;
        for _ in 0..200 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.5))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            let expanded_before = manager.log_window.tally.expanded;
            manager.fill_output(&mut output, 1.0);
            if manager.log_window.tally.expanded > expanded_before {
                expands += manager.log_window.tally.expanded - expanded_before;
            }
            if manager.buffer.occupied_count() <= 1 {
                saw_trigger = true;
            }
        }

        assert!(
            saw_trigger,
            "test precondition: the buffer must reach the imminent-underrun trigger",
        );
        assert!(
            expands > 0,
            "expand must defend an imminent underrun on loud content — the \
                 alternative it is being weighed against is unfaded PLC, not silence",
        );
        assert_eq!(
            manager.flow.starvation_count, 0,
            "the defence exists to prevent this",
        );
    }

    /// `MIN_EXPAND_INTERVAL` is what stops the defence from becoming the click
    /// train. Removing the energy gate widens the content expand can fire on, so
    /// the rate limit is now the *only* thing bounding splice density — a buffer
    /// pinned at one frame must not produce one insert per callback.
    ///
    /// A ceiling on its own would pass vacuously if expand never fired, and under
    /// `setup_env`'s 40ms config it very nearly does not: `effective_target`
    /// converges to 2, so `low_limit` is 1 and the `filtered < low_limit` branch
    /// closes as soon as the level creeps up. So this asserts both ends — hundreds
    /// of trigger callbacks, at most ~one splice per 200ms — on the same 150ms
    /// config the sibling tests use.
    #[test]
    fn expand_should_stay_rate_limited_when_the_buffer_hovers_at_one_frame() {
        let config = JitterConfig {
            min_depth_ms: 150,
            comfort_cap_ms: 400,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        };
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi2_4Ghz);
        let base = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut seq = 1u64;

        for _ in 0..16 {
            let arrival = base + Duration::from_millis(seq * 10);
            assert!(
                prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.5))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..10 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.flow.is_prebuffering);
        // Down to the trigger but not past it — see the sibling test for why a
        // starvation here would disarm the defence for the rest of the run.
        while manager.buffer.occupied_count() > 1 {
            manager.fill_output(&mut output, 1.0);
        }
        assert_eq!(
            manager.flow.starvation_count, 0,
            "test precondition: the drain-down must not starve",
        );

        const CALLBACKS: u32 = 400;
        let mut expands = 0u32;
        let mut trigger_callbacks = 0u32;
        for _ in 0..CALLBACKS {
            // Pin the depth at the trigger. A rate-matched one-in-one-out trickle
            // cannot: expand's insert leaves a partial frame in `playback_buf`, so
            // the next callback serves from it without popping and occupancy
            // ratchets one frame up per splice. Measured, that lifts the buffer off
            // the trigger permanently after ~25 callbacks — which is a real and
            // welcome self-limit in the field, but it means an unpinned harness
            // would measure the ratchet instead of `MIN_EXPAND_INTERVAL`.
            while manager.buffer.occupied_count() < 2 {
                let arrival = base + Duration::from_millis(seq * 10);
                assert!(
                    prod.try_push(make_tone_packet_at(&mut encoder, seq, arrival, 0.5))
                        .is_ok()
                );
                seq += 1;
                manager.ingest_packets(&mut cons);
            }
            let expanded_before = manager.log_window.tally.expanded;
            manager.fill_output(&mut output, 1.0);
            if manager.log_window.tally.expanded > expanded_before {
                expands += manager.log_window.tally.expanded - expanded_before;
            }
            if manager.buffer.occupied_count() <= 1 {
                trigger_callbacks += 1;
            }
        }

        // Non-vacuity: the ceiling below only means something if the trigger was
        // standing for most of the run and the rate limit is what held the count
        // down, rather than the branch being unreachable.
        assert!(
            trigger_callbacks >= CALLBACKS / 2,
            "test precondition: the buffer must hover at the trigger — only \
                 {trigger_callbacks} of {CALLBACKS} callbacks ended one frame from empty",
        );
        assert!(
            expands >= 2,
            "expand fired {expands} times across {trigger_callbacks} trigger \
                 callbacks — a rate limit that is never reached bounds nothing, so the \
                 ceiling below would pass vacuously",
        );

        // One insert per `MIN_EXPAND_INTERVAL` callbacks is the ceiling; allow one
        // extra for the first fire, which is not preceded by a cooldown.
        let ceiling = CALLBACKS / MIN_EXPAND_INTERVAL + 1;
        assert!(
            expands <= ceiling,
            "expand fired {expands} times in {CALLBACKS} callbacks, above the \
                 {ceiling} the {MIN_EXPAND_INTERVAL}-callback rate limit allows — this \
                 is the ~17Hz splice train the limit exists to prevent",
        );
    }
}

/// How long the actuator waits between splices.
mod timescale_cooldown {
    use super::*;

    /// Shortening the interval as the band widens makes traversal time constant
    /// instead of proportional to the target. The three field-measured targets
    /// are the cases that matter: ADB must come out bit-identical (that is what
    /// keeps a field regression attributable to the scaling rather than to a
    /// link), and the two links whose `declined_cooldown` the scaling exists to
    /// reduce must actually shorten.
    #[test]
    fn the_cooldown_should_scale_with_the_distance_to_target() {
        assert_eq!(
            timescale_interval(6),
            TIMESCALE_INTERVAL_BASE,
            "ADB (target 6, band 1) must be bit-identical to the flat constant",
        );
        assert_eq!(
            timescale_interval(13),
            2,
            "5 GHz (target 13, band 3) must shorten — 180ms traversal at the flat rate",
        );
        assert_eq!(
            timescale_interval(36),
            2,
            "2.4 GHz (target 36, band 9) must shorten — this is the 540ms traversal \
                 that made the rate limiter 73% of that link's drain refusals",
        );
        // Monotone non-increasing in the target: a wider band never waits longer.
        let mut prev = timescale_interval(1);
        for target in 2..=80 {
            let interval = timescale_interval(target);
            assert!(
                interval <= prev,
                "interval rose from {prev} to {interval} at target {target} — the \
                     scaling is inverted, which makes `declined_cooldown` worse on \
                     exactly the link it is for",
            );
            prev = interval;
        }
    }

    /// The floor binds at the *large* targets, where the base divided by the band
    /// width would otherwise reach zero — a 2.4GHz target of 36 gives `6/9 == 0`
    /// in integer arithmetic, i.e. a splice every callback. Below 2 callbacks the
    /// rate limiter stops being one: the NCC gate inside `accelerate` is the
    /// quality control, and this only decides how often it is consulted.
    ///
    /// Paired with the small-target end, which must *not* be floored down —
    /// scaling that shortened ADB too would be a behavioural change on the one
    /// link with four artifact-free rounds behind it.
    #[test]
    fn the_cooldown_must_not_fall_below_the_adb_floor_at_a_small_target() {
        for target in 1..=7 {
            assert_eq!(
                timescale_interval(target),
                TIMESCALE_INTERVAL_BASE,
                "target {target} has a 1-frame band and must keep the ADB-calibrated \
                     interval — nothing about a small buffer justifies splicing faster",
            );
        }
        // Where the raw quotient underflows to zero, the floor is what stands.
        assert_eq!(
            TIMESCALE_INTERVAL_BASE / (80 / 4),
            0,
            "precondition: the raw quotient underflows"
        );
        assert_eq!(
            timescale_interval(80),
            MIN_TIMESCALE_INTERVAL,
            "at the comfort cap the floor must hold — an interval of 0 is a splice \
                 on every callback",
        );
    }
}

/// The content RMS reported on the depth line, which observes rather than gates.
mod rms_observation {
    use super::*;

    /// The field captures proved `declined_rms_mask` takes 93-96% of every drain
    /// attempt, but not by how much the content overshoots the threshold — the
    /// counter is a boolean verdict on a continuous quantity. A gate declining at
    /// rms 0.09 and one declining at rms 0.4 read identically, and they call for
    /// different answers (retune the constant vs. abandon the approach).
    ///
    /// The RMS is already computed on the hot path for the gate check itself, so
    /// reporting it costs one add. This is the same position `max_gap_age` was once
    /// in: the value existed, the decision turned on it, and the log could not see
    /// it.
    #[test]
    fn the_depth_line_should_report_the_content_rms_it_gated_on() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        for seq in (MIN_DEPTH as u64 + 1..).take(40) {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
        }

        assert!(
            manager.log_window.tally.rms_count > 0,
            "the gate ran on every one of those callbacks, so the census must have \
                 sampled it",
        );
        let avg = manager.log_window.tally.rms_sum / manager.log_window.tally.rms_count as f32;
        assert!(
            avg > ARTIFACT_MASK_RMS,
            "a 0.5-amplitude tone must read above the masking threshold ({ARTIFACT_MASK_RMS}), \
                 got {avg:.4} — otherwise the reported value is not the one the gate judged",
        );
        assert!(
            manager.log_window.tally.rms_max >= avg,
            "the window maximum cannot be below its own mean",
        );
    }

    /// A max-only reading would latch its own history — the invariant this module
    /// has paid for four times. One loud transient inside an otherwise quiet
    /// window must move the average a little and the maximum a lot, and the next
    /// window must forget both.
    #[test]
    fn the_reported_rms_should_average_across_the_window_not_latch_the_last_frame() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_tone_packet_at(
                    &mut encoder,
                    i as u64,
                    base_time + std::time::Duration::from_millis(i as u64 * 10),
                    0.5,
                ))
                .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        // One loud frame, then a run of quiet ones. Amplitude 0.001 is two orders
        // below the masking threshold, so the mean must land far under the peak.
        for (i, seq) in (MIN_DEPTH as u64 + 1..).take(30).enumerate() {
            let amp = if i == 0 { 0.9 } else { 0.001 };
            assert!(
                prod.try_push(make_tone_packet_at(
                    &mut encoder,
                    seq,
                    base_time + std::time::Duration::from_millis(seq * 10),
                    amp,
                ))
                .is_ok()
            );
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
        }

        let avg =
            manager.log_window.tally.rms_sum / manager.log_window.tally.rms_count.max(1) as f32;
        let peak = manager.log_window.tally.rms_max;
        assert!(
            peak > avg * 2.0,
            "one loud frame among quiet ones must show as a peak well above the \
                 mean, got peak={peak:.4} avg={avg:.4}",
        );

        // The window boundary must clear both, or the peak becomes a latch.
        manager.log_window.tally = TimescaleTally::default();
        assert_eq!(manager.log_window.tally.rms_max, 0.0);
        assert_eq!(manager.log_window.tally.rms_count, 0);
    }
}

/// The unplayed-frame ledger. `arrivals - played` is an exact count of audio
/// that entered the pipeline and never reached the DAC, and for a long time
/// nothing accounted for it: an uncompressed capture measured 1764 unplayed
/// frames (7.6% of arrivals, 17.6s of audio) with only 340 explained by the
/// logged flush and shed paths. These tests pin each sink to its counter so
/// the residual becomes arithmetic rather than inference.
mod unplayed_frame_ledger {
    use super::*;

    /// `stats.observe` counts an arrival *before* `buffer.insert` runs, so a
    /// packet rejected as [`InsertResult::Stale`] is already inside
    /// `arrivals` and can never appear in `played`. Nothing counted it.
    #[test]
    fn a_packet_arriving_behind_the_playhead_should_be_counted_as_a_stale_reject() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

        for i in 10..10 + MIN_DEPTH * 2 {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        // Advance the playhead so there is a "behind" to arrive behind.
        for _ in 0..3 {
            manager.fill_output(&mut output, 1.0);
        }
        let playhead = manager.buffer.next_play_seq();
        assert!(
            playhead > 10,
            "the playhead must have advanced for this test to mean anything",
        );
        manager.log_window.tally = TimescaleTally::default();

        // Seq 10 is now four frames behind wherever the playhead reached.
        let stale_seq = playhead - 4;
        assert!(
            prod.try_push(make_packet(&mut encoder, stale_seq, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);

        assert_eq!(
            manager.log_window.tally.stale_rejects, 1,
            "a packet behind the playhead is dropped silently — it must be \
                 counted, or it stays inside the unattributable 6.2%",
        );
        assert_eq!(
            manager.log_window.tally.stale_lag_max, 4,
            "the lag separates a harmless 1-2 frame reorder from a skip that \
                 threw away packets still in flight; only the distance says which",
        );
        assert_eq!(manager.log_window.tally.stale_lag_sum, 4);
    }

    /// `fast_forward` jumps the playhead over a whole hole. Counting the
    /// events without the distance would report a 30-frame jump and a 1-frame
    /// nudge identically, and the whole question is which one the 60ms reorder
    /// tolerance is actually producing.
    #[test]
    fn a_playhead_skip_should_report_how_many_frames_it_jumped() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        // Drain what is present, then leave a hole with a packet beyond it so
        // the gap path takes `fast_forward` rather than `advance_one`.
        while manager.buffer.occupied_count() > 1 {
            manager.fill_output(&mut output, 1.0);
        }
        let hole_start = manager.buffer.next_play_seq();
        let landing = hole_start + 8;
        assert!(
            prod.try_push(make_packet(&mut encoder, landing, base_time))
                .is_ok()
        );
        manager.ingest_packets(&mut cons);
        manager.log_window.tally = TimescaleTally::default();

        // The hole-hold arms on `gap_hold_count >= tolerance`; drive enough
        // callbacks for it to fire.
        for _ in 0..REORDER_TOLERANCE + 4 {
            manager.fill_output(&mut output, 1.0);
        }

        assert!(
            manager.log_window.tally.playhead_skips >= 1,
            "the gap path must have skipped — with no skip the counters below \
                 pass vacuously",
        );
        // Measured 1 skip / 7 frames. Strictly greater is what proves the
        // `fast_forward` path ran rather than a 1-frame `advance_one` nudge —
        // an equality here would pass with the distance term dead.
        assert!(
            manager.log_window.tally.skipped_frames > manager.log_window.tally.playhead_skips,
            "every skip jumps at least one frame, so the frame count can \
                 never be below the event count",
        );
        assert!(
            manager.buffer.next_play_seq() > hole_start,
            "the playhead must have moved past the hole it skipped",
        );
    }

    /// The clamp and the startup flush both discard through
    /// `flush_with_crossfade`, and each logs its own differently-shaped line.
    /// One counter puts them in the same ledger as the other sinks.
    #[test]
    fn a_flush_should_count_every_frame_it_discards() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        for i in 1..=20 {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let before = manager.buffer.occupied_count();
        assert!(before > 5, "need a surplus to flush");
        manager.log_window.tally = TimescaleTally::default();

        manager.flush_with_crossfade(5);

        assert_eq!(
            manager.buffer.occupied_count(),
            5,
            "the flush must land exactly on its target",
        );
        assert_eq!(
            manager.log_window.tally.flush_discards,
            before - 5,
            "the discard count must equal the occupancy the flush removed — \
                 this is the term that closes `arrivals - played`",
        );
    }

    /// A flush that has nothing to discard must not report one. The early
    /// return at the top of `flush_with_crossfade` is the only thing keeping
    /// the ledger from over-counting, and an underflow there would panic in
    /// debug and wrap in release.
    #[test]
    fn a_flush_with_no_surplus_should_count_nothing() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();

        for i in 1..=3 {
            assert!(
                prod.try_push(make_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.log_window.tally = TimescaleTally::default();

        let before = manager.buffer.occupied_count();
        manager.flush_with_crossfade(before + 10);

        assert_eq!(
            manager.log_window.tally.flush_discards, 0,
            "a no-op flush must not report a discard",
        );
        assert_eq!(manager.buffer.occupied_count(), before);
    }

    /// The last sink in the ledger, and the one that made the other four look
    /// wrong. `frames_played` counts *callbacks that consumed a frame*, but a
    /// drain pops a **second** packet in the same callback to widen the pitch
    /// search ([`StagedWindow::extend`]) — that frame is emitted, either
    /// inside the splice or verbatim when the splice declines, and it never
    /// passes through `frames_played`. So `arrivals - played` charged it as
    /// loss: an uncompressed capture read **6.8% packet loss on a link
    /// whose true loss was zero**, and every subsequent reading of that line
    /// was measured against a baseline that did not exist.
    ///
    /// Asserted from both sides. With `staged` subtracted the residual is
    /// exactly the buffer's own occupancy — every frame that arrived and did
    /// not play is still sitting there. Without it, the residual overstates by
    /// exactly `staged`, which is the phantom the field log reported.
    #[test]
    fn the_unplayed_ledger_should_close_when_the_drain_stages_a_frame() {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];

        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);
        assert!(!manager.flow.is_prebuffering);

        let target = manager
            .control
            .target_breakdown(&manager.config, &manager.stats, None)
            .raw;
        let high_limit = TargetController::buffer_limits(target).high;

        // Overrun the band so the drain arms, on loud periodic content so the
        // NCC gate admits the splice and the staging pop actually happens.
        let mut seq = MIN_DEPTH as u64 + 1;
        for _ in 0..high_limit + 2 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
        }
        manager.ingest_packets(&mut cons);

        // Fewer callbacks than `LOG_INTERVAL_CALLBACKS`: the log line takes the
        // tally when it fires, so a longer run would measure a partial window.
        for _ in 0..50 {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, seq, base_time))
                    .is_ok()
            );
            seq += 1;
            manager.ingest_packets(&mut cons);
            manager.fill_output(&mut output, 1.0);
        }

        let occupied = manager.buffer.occupied_count();
        let played = manager.log_window.frames_played;
        let staged = manager.log_window.tally.staged_pops;
        let arrivals = manager.stats.take_arrival_count();

        assert!(
            staged > 0,
            "precondition: no drain staged a second frame, so this run cannot \
                 tell the two ledgers apart",
        );
        assert_eq!(
            (
                manager.log_window.tally.stale_rejects,
                manager.log_window.tally.skipped_frames,
                manager.log_window.tally.flush_discards,
                manager.log_window.tally.shed,
            ),
            (0, 0, 0, 0),
            "precondition: every other sink must be closed, or the residual \
                 below is not attributable to the staging pop",
        );

        assert_eq!(
            arrivals - played - staged,
            occupied,
            "with the staging pop accounted for, every frame that arrived and \
                 did not play must still be in the buffer",
        );
        assert_eq!(
            arrivals - played,
            occupied + staged,
            "the old line reported {staged} frames of loss that were in fact \
                 emitted — this is the 6.8% a field capture read on a link with \
                 none",
        );
    }

    /// The two tiers are two different upstream operations, and only one of
    /// them is allowed to refuse.
    ///
    /// `expand` has two triggers — `occupied <= 1` (imminent underrun) and
    /// `filtered < low_limit` (preemptive growth). Counting the first tier's
    /// refusals for the first time produced a damning census: **83 of 90
    /// attempts declined** on 2.4GHz uncompressed, emitted as raw silence.
    /// That was a port error rather than a tuning one — the underrun
    /// tier is NetEQ's `Expand` (`expand.cc:438-455`), which carries *no*
    /// correlation gate and always emits, while we were running it through
    /// `PreemptiveExpand`'s 0.9 NCC gate.
    ///
    /// So `declined_underrun_ncc` must now read **0 always**, and the counter
    /// survives as a tripwire in exactly the way `declined_rms_mask` does.
    ///
    /// Both tiers run against the **same unpitched noise**, which is what makes
    /// the claim falsifiable rather than vacuous: tier 2 declining on that
    /// material is the proof it fails the NCC gate, so tier 1 splicing it can
    /// only be the gate being genuinely absent. The pairing also pins the
    /// split — a single shared counter would pass a one-tier test and fail this.
    #[test]
    fn a_declined_underrun_should_no_longer_be_possible() {
        // 150ms floor so `low_limit` is ~12 — `setup_env`'s 40ms config
        // converges to a target of 2 and a `low_limit` of 1, where the growth
        // branch closes as soon as the level creeps up.
        let config = JitterConfig {
            min_depth_ms: 150,
            comfort_cap_ms: 400,
            peak_decay_halflife_ms: 0,
            resume_threshold_pct: 0.5,
            static_target_ms: None,
        };
        const CALLBACKS: u32 = 90;

        // --- Tier 1: one frame from empty. ---
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config.clone(), NetworkLink::Wifi2_4Ghz);
        let base = Instant::now();
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        let mut seq = 1u64;
        let mut seed = 0x1234_5678u32;
        let feed = |prod: &mut ringbuf::HeapProd<RawPacket>,
                    encoder: &mut Encoder,
                    seq: &mut u64,
                    seed: &mut u32| {
            let arrival = base + Duration::from_millis(*seq * 10);
            assert!(
                prod.try_push(make_noise_packet_at(encoder, *seq, arrival, 0.6, seed))
                    .is_ok()
            );
            *seq += 1;
        };

        for _ in 0..16 {
            feed(&mut prod, &mut encoder, &mut seq, &mut seed);
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..10 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.flow.is_prebuffering);
        // Down to the trigger but not past it: a starvation arms a 500ms
        // wall-clock recovery window that holds `stretch_allowed` off, and the
        // clock does not advance on its own across a test's callbacks.
        while manager.buffer.occupied_count() > 1 {
            manager.fill_output(&mut output, 1.0);
        }
        assert_eq!(
            manager.flow.starvation_count, 0,
            "test precondition: the drain-down must not starve",
        );
        // `filtered_buffer_level` is an IIR with a ~1.3s time constant, so it
        // still carries the prebuffer depth for tens of callbacks after the
        // buffer itself has emptied to the edge. The growth branch does not
        // open until it lands inside the band, so let it converge *before*
        // the measured window rather than losing the first callbacks to it.
        for _ in 0..400 {
            let low = TargetController::buffer_limits(manager.control.effective_target).low;
            if manager.flow.filtered_buffer_level < low as f32 {
                break;
            }
            while manager.buffer.occupied_count() < 2 {
                feed(&mut prod, &mut encoder, &mut seq, &mut seed);
                manager.ingest_packets(&mut cons);
            }
            manager.fill_output(&mut output, 1.0);
        }
        // The 1Hz log line takes the tally, so the measured run must fit
        // inside one window.
        manager.log_window.tally = TimescaleTally::default();
        manager.log_window.frame_count = 0;

        let mut at_edge = 0u32;
        for _ in 0..CALLBACKS {
            while manager.buffer.occupied_count() < 2 {
                feed(&mut prod, &mut encoder, &mut seq, &mut seed);
                manager.ingest_packets(&mut cons);
            }
            manager.fill_output(&mut output, 1.0);
            if manager.buffer.occupied_count() <= 1 {
                at_edge += 1;
            }
        }

        // The edge is sampled from outside, after the callback, and a splice
        // ratchets occupancy up one frame per insert (its surplus is served
        // from `playback_buf` on the next callback, which then skips the pop —
        // the ratchet documented on the rate-limit tests). So the trigger's
        // standing is the sum of the two sides: every callback either ended at
        // the edge, or ended past it *because a splice had just covered it*.
        // A dead actuator (expanded = 0) has no second side, and the
        // `expanded > 0` assertion below then fails — the pairing is what
        // keeps this from passing vacuously.
        assert!(
            at_edge + manager.log_window.tally.expanded >= CALLBACKS,
            "test precondition: the underrun trigger must stand on every \
                 callback, either one frame from empty or freshly carried past the \
                 edge by a splice's surplus",
        );
        assert_eq!(
            manager.log_window.tally.declined_underrun_ncc, 0,
            "the concealment tier must never refuse — upstream's `Expand` has \
                 no correlation gate. The old gate declined {CALLBACKS} of these \
                 and emitted silence instead; a non-zero reading here says it came \
                 back",
        );
        assert!(
            manager.log_window.tally.expanded > 0,
            "the concealment tier must actually have spliced this material — \
                 tier 2 below declines the very same noise, so a zero here would \
                 mean the tier is inert rather than ungated",
        );
        // Ungating the tier must not turn it into the click train
        // `MIN_EXPAND_INTERVAL` exists to prevent. The cooldown is the only
        // thing rate-limiting concealment now that the NCC gate is gone, which
        // makes this ceiling load-bearing rather than incidental.
        assert!(
            manager.log_window.tally.expanded <= CALLBACKS / MIN_EXPAND_INTERVAL + 1,
            "{} splices in {CALLBACKS} callbacks exceeds one per \
                 {MIN_EXPAND_INTERVAL} — the cooldown is the only limit left on \
                 the concealment tier",
            manager.log_window.tally.expanded,
        );
        assert_eq!(
            manager.log_window.tally.preemptive, 0,
            "every splice in this window was taken one frame from empty, so \
                 none of them may be charged to the growth tier",
        );
        assert_eq!(
            manager.log_window.tally.declined_preemptive_ncc, 0,
            "an underrun-tier attempt must not be charged to the preemptive \
                 tier — the two are separated so a defence that arms and never \
                 fires is distinguishable from growth that does the same",
        );

        // --- Tier 2: the mirror. Below the band, but not at the edge. ---
        let (mut manager, mut encoder, mut prod, mut cons) =
            setup_env_with(config, NetworkLink::Wifi2_4Ghz);
        let mut seq = 1u64;
        let mut seed = 0x9e37_79b9u32;
        for _ in 0..16 {
            feed(&mut prod, &mut encoder, &mut seq, &mut seed);
        }
        manager.ingest_packets(&mut cons);
        for _ in 0..10 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.flow.is_prebuffering);
        // Same IIR convergence as tier 1, at the higher pinned occupancy.
        for _ in 0..400 {
            let low = TargetController::buffer_limits(manager.control.effective_target).low;
            if manager.flow.filtered_buffer_level < low as f32 {
                break;
            }
            while manager.buffer.occupied_count() < 5 {
                feed(&mut prod, &mut encoder, &mut seq, &mut seed);
                manager.ingest_packets(&mut cons);
            }
            manager.fill_output(&mut output, 1.0);
        }
        manager.log_window.tally = TimescaleTally::default();
        manager.log_window.frame_count = 0;

        // Hold occupancy at 5: comfortably above the `occupied <= 1` trigger,
        // still far below a `low_limit` of ~12, so it is the preemptive tier
        // that arms on every callback.
        let mut below_band = 0u32;
        for _ in 0..CALLBACKS {
            while manager.buffer.occupied_count() < 5 {
                feed(&mut prod, &mut encoder, &mut seq, &mut seed);
                manager.ingest_packets(&mut cons);
            }
            let low_limit = TargetController::buffer_limits(manager.control.effective_target).low;
            if manager.flow.filtered_buffer_level < low_limit as f32 {
                below_band += 1;
            }
            manager.fill_output(&mut output, 1.0);
        }

        assert_eq!(
            below_band, CALLBACKS,
            "test precondition: the preemptive trigger must stand on every \
                 callback for the mirror assertion to mean anything",
        );
        assert_eq!(
            manager.log_window.tally.declined_underrun_ncc, 0,
            "nothing here was one frame from empty; a preemptive decline \
                 charged to the underrun tier would make the new counter a \
                 duplicate of the old one",
        );
        assert!(
            manager.log_window.tally.declined_preemptive_ncc > 0,
            "precondition: the preemptive tier must actually have declined, or \
                 the assertion above passes vacuously",
        );
    }
}

/// How long the current concealment run is.
///
/// `starvation_count` cannot answer that question, and the reason is
/// structural rather than incidental: it is incremented only on the starvation
/// path, and the rebuffer hold — where the longest runs live — conceals from an
/// early return that never reaches it. The counter therefore freezes at
/// `REBUFFER_AFTER` for the whole hold, which is pinned as *correct* by
/// `rebuffer_pause_collapses_a_starvation_cluster_into_one_event` (one gap must
/// bill one event). `conceal_run` counts what was actually emitted instead.
mod conceal_run {
    use super::*;

    /// Play `MIN_DEPTH` loud frames and leave the buffer empty, so the next
    /// callback is the first concealed one. Mirrors the setup
    /// `sustained_starvation_rebuffers_once_and_keeps_playing_plc` establishes:
    /// loud packets make one callback consume exactly one frame, which is what
    /// makes a run length countable at all.
    /// Shared with `super::pitch_concealment`, whose Opus-path test needs the
    /// same "playing, then nothing" setup on a stream the codec *can*
    /// extrapolate from.
    pub(super) fn playing_then_empty() -> (
        JitterBufferManager,
        Encoder,
        ringbuf::HeapProd<RawPacket>,
        ringbuf::HeapCons<RawPacket>,
        Instant,
        Vec<f32>,
    ) {
        let (mut manager, mut encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_loud_packet(&mut encoder, i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(
            !manager.flow.is_prebuffering,
            "precondition: playback must have started",
        );
        assert_eq!(
            manager.buffer.occupied_count(),
            0,
            "precondition: the buffer must be empty, or the next callback \
                 plays a real frame instead of concealing",
        );
        assert_eq!(
            manager.flow.conceal_run, 0,
            "precondition: a run must not be standing before the outage",
        );
        (manager, encoder, prod, cons, base_time, output)
    }

    #[test]
    fn a_concealed_callback_should_extend_the_consecutive_conceal_run() {
        let (mut manager, _encoder, _prod, _cons, _base, mut output) = playing_then_empty();

        // Stop one short of REBUFFER_AFTER: this asserts the *starvation* path
        // increments, which is the half `starvation_count` already covered.
        for expected in 1..REBUFFER_AFTER {
            manager.fill_output(&mut output, 1.0);
            assert_eq!(
                manager.flow.conceal_run, expected,
                "each concealed callback must extend the run by exactly one",
            );
        }
        assert_eq!(
            manager.flow.starvation_count, manager.flow.conceal_run,
            "on the starvation path the two counters must agree — the fade \
                 keyed to either one behaves identically here, which is what makes \
                 the re-keying bit-identical outside the hold",
        );
    }

    #[test]
    fn a_played_frame_should_reset_the_consecutive_conceal_run() {
        let (mut manager, mut encoder, mut prod, mut cons, base_time, mut output) =
            playing_then_empty();

        for _ in 1..REBUFFER_AFTER {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(
            manager.flow.conceal_run > 0,
            "precondition: a run must be standing, or the reset below is \
                 unobservable",
        );

        assert!(
            prod.try_push(make_loud_packet(
                &mut encoder,
                u64::from(MIN_DEPTH) + 1,
                base_time
            ))
            .is_ok()
        );
        manager.ingest_packets(&mut cons);
        manager.fill_output(&mut output, 1.0);

        assert_eq!(
            manager.flow.conceal_run, 0,
            "a callback that emits a real decoded frame must end the run; the \
                 reset sits at the top of the `has_next` branch rather than beside \
                 the `starvation_count = 0` line, which is itself inside a \
                 `starvation_count > 0` guard and so cannot end a run the rebuffer \
                 hold produced",
        );
    }

    /// The discriminator against `starvation_count`. Falsify by keying the
    /// assertion to that counter: it reads `REBUFFER_AFTER` for every callback
    /// of the hold, so the run length is invisible exactly where it is longest.
    #[test]
    fn the_conceal_run_should_keep_climbing_through_a_rebuffer_hold() {
        const HELD_CALLBACKS: u32 = 20;
        let (mut manager, _encoder, _prod, _cons, _base, mut output) = playing_then_empty();

        for _ in 1..=REBUFFER_AFTER {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(
            manager.flow.is_prebuffering,
            "precondition: the rebuffer hold must be armed, or this measures \
                 the starvation path the test above already covers",
        );
        assert_eq!(manager.flow.conceal_run, REBUFFER_AFTER);

        // Well inside `max_missing_for(Unknown)` = 200 callbacks, so the hold
        // holds and nothing here reaches `trigger_reset`.
        for _ in 1..=HELD_CALLBACKS {
            manager.fill_output(&mut output, 1.0);
            assert!(manager.flow.is_prebuffering);
        }

        assert_eq!(
            manager.flow.starvation_count, REBUFFER_AFTER,
            "precondition: `starvation_count` must be frozen — that is the \
                 contract `rebuffer_pause_collapses_a_starvation_cluster_into_one_event` \
                 pins, and the reason it cannot key a fade",
        );
        assert_eq!(
            manager.flow.conceal_run,
            REBUFFER_AFTER + HELD_CALLBACKS,
            "the hold conceals from an early return that never touches \
                 `starvation_count`, so only a counter incremented inside \
                 `generate_plc` can see how long the run has been going",
        );
    }

    /// The run belongs to the stream that produced it. Carrying it across a
    /// restart would open the new stream's first concealment already faded,
    /// since the fade gain is keyed to this counter.
    #[test]
    fn a_stream_restart_should_clear_the_consecutive_conceal_run() {
        let (mut manager, _encoder, _prod, _cons, _base, mut output) = playing_then_empty();

        for _ in 1..REBUFFER_AFTER {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(manager.flow.conceal_run > 0, "precondition: a standing run");

        manager.trigger_reset();

        assert_eq!(manager.flow.conceal_run, 0);
    }

    /// The 1 Hz line must carry the run's window maximum, and must clear it the
    /// way every other `TimescaleTally` counter is cleared — `conceal_run_max`
    /// is a window observation, not a total.
    #[test]
    fn the_depth_line_should_report_the_windows_longest_conceal_run() {
        let (mut manager, mut encoder, mut prod, mut cons, base_time, mut output) =
            playing_then_empty();

        for _ in 1..REBUFFER_AFTER {
            manager.fill_output(&mut output, 1.0);
        }
        let run = manager.flow.conceal_run;
        assert!(run > 0, "precondition: a standing run");
        assert_eq!(
            manager.log_window.tally.conceal_run_max, run,
            "the window maximum must track the run as it climbs",
        );

        // Flush on a callback that plays a real frame. `log_depth_authority`
        // runs *before* the concealment branch, so flushing on a concealed
        // callback would take the tally and then have `generate_plc` write the
        // new window's first reading into it — a correct sequence that this
        // assertion could not tell apart from a reset that never happened.
        assert!(
            prod.try_push(make_loud_packet(
                &mut encoder,
                u64::from(MIN_DEPTH) + 1,
                base_time
            ))
            .is_ok()
        );
        manager.ingest_packets(&mut cons);
        manager.log_window.frame_count = LOG_INTERVAL_CALLBACKS - 1;
        manager.fill_output(&mut output, 1.0);
        assert_eq!(
            manager.log_window.tally.conceal_run_max, 0,
            "the 1 Hz flush must reset the window maximum",
        );
    }
}

/// What a concealed frame on the uncompressed path actually contained.
///
/// `FrameDecoder::capture` never feeds the codec on that path — deliberately,
/// since PLC state built from a non-Opus stream only poisons the next Opus
/// transition — so `decode_plc()` there ran on a decoder this stream had never
/// advanced and returned **exact zeros**. All 267 concealed frames (2674ms,
/// 0.583%) of an uncompressed field capture were digital silence with a fade on
/// it. `expand_conceal` could not cover the hole either: it is structurally
/// unreachable at `occupied == 0`, because `expand_inner` stages
/// `[history | pcm]` and refuses when `anchor < hist_frames` — with history
/// alone that is `352 < 480`.
mod pitch_concealment {
    use super::*;

    /// A loud 200 Hz tone shipped as raw f32 PCM — the uncompressed wire
    /// format, byte for byte as `parse_packet` produces it. Phase is anchored to
    /// the absolute frame position so consecutive packets are seamless, which is
    /// what lets the history hold a real pitch period.
    fn make_uncompressed_packet(seq: u64, base_time: Instant) -> RawPacket {
        let ch = OPUS_CHANNELS as usize;
        let frames = OPUS_FRAME_SAMPLES / ch;
        let base_frame = seq * frames as u64;
        let mut pkt = RawPacket::zeroed();
        pkt.seq_num = seq;
        pkt.is_uncompressed = true;
        pkt.payload_len = OPUS_FRAME_SAMPLES * std::mem::size_of::<f32>();
        pkt.arrival_time =
            base_time + std::time::Duration::from_millis(seq * MILLIS_PER_FRAME as u64);
        for i in 0..frames {
            let t = (base_frame + i as u64) as f32 / OPUS_SAMPLE_RATE as f32;
            let s = (2.0 * std::f32::consts::PI * 200.0 * t).sin() * 0.5;
            for c in 0..ch {
                let j = (i * ch + c) * 4;
                pkt.payload_data[j..j + 4].copy_from_slice(&s.to_ne_bytes());
            }
        }
        pkt
    }

    /// The uncompressed mirror of `conceal_run::playing_then_empty`.
    pub(super) fn uncompressed_then_empty() -> (
        JitterBufferManager,
        ringbuf::HeapProd<RawPacket>,
        ringbuf::HeapCons<RawPacket>,
        Instant,
        Vec<f32>,
    ) {
        let (mut manager, _encoder, mut prod, mut cons) = setup_env();
        let base_time = Instant::now();
        for i in 1..=MIN_DEPTH {
            assert!(
                prod.try_push(make_uncompressed_packet(i as u64, base_time))
                    .is_ok()
            );
        }
        manager.ingest_packets(&mut cons);
        let mut output = vec![0.0; OPUS_FRAME_SAMPLES];
        for _ in 1..=MIN_DEPTH {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(!manager.flow.is_prebuffering, "precondition: playing");
        assert_eq!(manager.buffer.occupied_count(), 0, "precondition: empty");
        assert!(
            !manager.decoder.plc_ready(),
            "precondition: the uncompressed path must leave the codec unable to \
                 extrapolate, or this whole module is unreachable",
        );
        assert!(
            manager.timescale.has_history(),
            "precondition: `remember` runs on every popped frame, so the last \
                 played frame must be staged",
        );
        (manager, prod, cons, base_time, output)
    }

    /// Peak of the region of `output` that a concealed callback's *new* frame
    /// could have written, given how much `playback_buf` still held going in.
    ///
    /// This is not defensive arithmetic, it is the difference between a real
    /// assertion and a vacuous one. `fill_output` serves the residue first and
    /// only calls `process_next_frame` once the deque runs dry, and a splice
    /// during the setup callbacks leaves a partial frame behind — measured 480
    /// samples, exactly half a frame. A whole-frame peak therefore reads **0.5 of
    /// real audio** even when every concealed sample is a zero, which is what the
    /// falsification run against the previous branch actually showed.
    pub(super) fn new_frame_peak(output: &[f32], residue: usize) -> f32 {
        let from = residue.min(output.len());
        assert!(
            from < output.len(),
            "the residue filled the whole callback, so nothing here was concealed",
        );
        output[from..].iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    /// **The defect, end to end.** This is the assertion that fails on the
    /// code it replaces: without the branch below, the callback
    /// lands in `decode_plc()` on a virgin decoder, and every emitted sample is
    /// exactly `0.0`.
    #[test]
    fn concealment_on_an_uncompressed_stream_should_not_emit_digital_silence() {
        let (mut manager, _prod, _cons, _base, mut output) = uncompressed_then_empty();

        let residue = manager.playback_buf.len();
        manager.fill_output(&mut output, 1.0);

        let peak = new_frame_peak(&output, residue);
        assert!(
            peak > 0.01,
            "the concealed frame peaked at {peak} — digital silence is a hole \
                 with a fade on it, not concealment",
        );
        assert_eq!(
            manager.log_window.tally.pitch_conceals, 1,
            "and the 1 Hz line must say which mechanism produced it",
        );
        assert_eq!(
            manager.flow.conceal_run, 1,
            "the run counter and the conceal counter describe the same callback",
        );
        assert!(
            manager.log_window.tally.conceal_step_max > 0.0,
            "the entry seam must be measured — a structural zero would mean the \
                 reading is taken at the crossfade handover weight, where it cannot \
                 report anything",
        );
    }

    /// The first three concealed callbacks run un-faded, so the run stays
    /// audible rather than collapsing to the silence it replaced. Every frame
    /// of the run must carry signal until the fade starts at 4 — a
    /// single-frame assertion above would pass even if the mechanism only
    /// worked once.
    #[test]
    fn a_run_of_concealed_callbacks_should_stay_audible_until_the_fade_begins() {
        let (mut manager, _prod, _cons, _base, mut output) = uncompressed_then_empty();

        for expected in 1..=3u32 {
            let residue = manager.playback_buf.len();
            manager.fill_output(&mut output, 1.0);
            let peak = new_frame_peak(&output, residue);
            assert_eq!(manager.flow.conceal_run, expected);
            assert!(
                peak > 0.01,
                "concealed callback {expected} peaked at {peak}"
            );
            assert_eq!(manager.log_window.tally.pitch_conceals, expected);
        }
    }

    /// **The Opus path must be bit-identical**, which is the property that makes
    /// this change safe to ship without a second field capture: `plc_ready()`
    /// short-circuits before `conceal_frame` is even called, so the codec runs
    /// exactly as it did.
    #[test]
    fn concealment_on_an_opus_stream_should_still_use_the_codec() {
        let (mut manager, _encoder, _prod, _cons, _base, mut output) =
            super::conceal_run::playing_then_empty();
        assert!(
            manager.decoder.plc_ready(),
            "precondition: a decoded Opus frame leaves the codec able to extrapolate",
        );

        for _ in 1..REBUFFER_AFTER {
            manager.fill_output(&mut output, 1.0);
        }

        assert!(manager.flow.conceal_run > 0, "precondition: a standing run");
        assert_eq!(
            manager.log_window.tally.pitch_conceals, 0,
            "an Opus stream must never reach the pitch-repetition branch",
        );
        assert_eq!(
            manager.log_window.tally.conceal_step_max, 0.0,
            "and must never write its seam counter",
        );
    }

    /// The other fallback, reached the way the manager actually reaches it: a
    /// stream reset drops the staged history, so a concealment before the next
    /// real frame has nothing adjacent to repeat and must stay on the codec —
    /// the same digital silence as before, but only for the one callback where
    /// there is genuinely nothing better to emit.
    #[test]
    fn concealment_without_staged_history_should_fall_back_to_the_codec() {
        let (mut manager, _prod, _cons, _base, mut output) = uncompressed_then_empty();

        manager.trigger_reset();
        assert!(
            !manager.timescale.has_history(),
            "precondition: a reset must drop the history, or the fallback is \
                 not the thing under test",
        );
        assert!(
            !manager.decoder.plc_ready(),
            "precondition: still uncompressed"
        );

        manager.flow.is_prebuffering = false;
        manager.fill_output(&mut output, 1.0);

        assert_eq!(
            manager.log_window.tally.pitch_conceals, 0,
            "with no history there is nothing to repeat; the caller must land on \
                 the codec path unchanged rather than splice from stale audio",
        );
    }
}

/// The concealment fade must decay toward, and never to, silence.
///
/// An earlier round fixed *what* concealment emits — pitch repetition instead of
/// a virgin decoder's zeros — and left the gain schedule that was written for the
/// output it replaced. That schedule reached exact digital silence at
/// `conceal_run == 7` (60ms), and across 64 rebuffer holds on five links it muted
/// **382 frames / 3820ms** to zero: 57.9% of every 2.4GHz hold, 77.5% of ADB's,
/// with **39/64 holds running past frame 7**. That is the "dropout" the field
/// reported. See [`super::super::consts::CONCEAL_FADE_FLOOR`].
///
/// The uncompressed setup is used throughout rather than the Opus one, and that
/// is not a stylistic choice. `make_uncompressed_packet` reproduces at a peak of
/// **exactly 0.5 on every concealed frame** (the pitch repetition is verbatim, so
/// the source amplitude is preserved), which makes `peak / 0.5` the *measured
/// gain* — the schedule is then asserted directly instead of inferred. Opus PLC
/// decays on its own (measured 0.501 → 0.007 across the same twenty callbacks),
/// so on that path a low reading cannot be attributed to the fade at all.
mod conceal_fade_floor {
    use super::pitch_concealment::uncompressed_then_empty;
    use super::*;

    /// Peak of a whole concealed callback, in units of the source amplitude —
    /// i.e. the gain the fade applied. Callers must have drained
    /// `playback_buf` first, so the callback is exactly one concealed frame.
    const SOURCE_PEAK: f32 = 0.5;

    fn gain(output: &[f32]) -> f32 {
        output.iter().fold(0.0f32, |m, s| m.max(s.abs())) / SOURCE_PEAK
    }

    /// Drain the standing partial frame so each subsequent callback is exactly
    /// one concealed frame. `fill_output` serves the residue *before* calling
    /// `process_next_frame`, so without this every 960-sample callback emits
    /// the tail of one frame at one gain and the head of the next at another,
    /// and no peak reading describes a single point on the schedule.
    fn drain_residue(manager: &mut JitterBufferManager, output: &mut [f32]) {
        let residue = manager.playback_buf.len();
        assert!(
            residue <= OPUS_FRAME_SAMPLES,
            "precondition: the residue must be under one frame ({residue})",
        );
        manager.fill_output(&mut output[..residue], 1.0);
        assert!(manager.playback_buf.is_empty());
    }

    /// **The defect, stated as the assertion that fails on the code it
    /// replaces.** Under the old schedule, `1.0 - (run - 3)/4` floored at 0.0,
    /// callback 7 emits an exact zero and every callback after it does too.
    ///
    /// Deliberately reads *past* the old terminus rather than at it: a single
    /// reading at 7 would also pass on a schedule that merely delayed the zero,
    /// and the property under test is that there is no zero at all.
    #[test]
    fn the_concealment_fade_should_never_reach_digital_silence() {
        let (mut manager, _prod, _cons, _base, mut output) = uncompressed_then_empty();
        drain_residue(&mut manager, &mut output);

        for expected in 1..=30u32 {
            manager.fill_output(&mut output, 1.0);
            assert_eq!(
                manager.flow.conceal_run, expected,
                "precondition: one concealed frame per callback, or the peak \
                     below describes two points on the schedule at once",
            );
            let g = gain(&output);
            assert!(
                g >= CONCEAL_FADE_FLOOR - 1e-6,
                "concealed callback {expected} emitted gain {g}, below the floor \
                     {CONCEAL_FADE_FLOOR} — the run has been muted to silence, which \
                     is the artifact the floor exists to remove",
            );
        }
        assert!(
            manager.flow.is_prebuffering,
            "precondition: a run this long is a rebuffer hold, the population \
                 the field measurement is drawn from",
        );
        assert_eq!(
            manager.log_window.tally.pitch_conceals, 30,
            "precondition: every frame above was pitch repetition — real audio \
                 that really played, which is what makes muting it to zero wrong",
        );
    }

    /// The floor is a *floor*, not the whole schedule: the fade still decays,
    /// and still starts where it did.
    ///
    /// Falsification is the point of the two ends. A schedule that jumped
    /// straight to `CONCEAL_FADE_FLOOR` would pass the test above; one that
    /// never faded at all would too. Neither passes here.
    #[test]
    fn the_concealment_fade_should_decay_monotonically_to_its_floor() {
        let (mut manager, _prod, _cons, _base, mut output) = uncompressed_then_empty();
        drain_residue(&mut manager, &mut output);

        let mut gains = Vec::new();
        for _ in 1..=20 {
            manager.fill_output(&mut output, 1.0);
            gains.push(gain(&output));
        }

        for (i, w) in gains.windows(2).enumerate() {
            assert!(
                w[1] <= w[0] + 1e-6,
                "the fade must not re-open: callback {} rose {} -> {}",
                i + 2,
                w[0],
                w[1],
            );
        }
        for (i, g) in gains.iter().take(3).enumerate() {
            assert!(
                (g - 1.0).abs() < 1e-6,
                "callback {} must play un-faded ({g}) — the `> 3` threshold is \
                     unchanged",
                i + 1,
            );
        }
        assert!(
            gains[3] < 1.0 - 1e-6,
            "the fade must begin at callback 4 ({}), or the schedule has been \
                 flattened rather than slowed",
            gains[3],
        );
        assert!(
            (gains[19] - CONCEAL_FADE_FLOOR).abs() < 1e-6,
            "and must have reached the floor by callback 20 ({}); a fade that \
                 never arrives is a sustained note",
            gains[19],
        );
    }

    /// The terminus moved, and by how much is the change. At callback 7 — the
    /// old schedule's exact zero — the run must still be clearly audible.
    ///
    /// This is the one assertion that pins the *slope* rather than the floor.
    /// Falsify against a floor-only change (`/4.0` kept, `.max(0.15)`): that
    /// reaches 0.15 at callback 7 and fails here, which is exactly why that
    /// variant was rejected (mean gain 0.414 against 0.576 over the measured
    /// holds).
    #[test]
    fn the_concealment_fade_should_still_be_audible_where_it_used_to_be_silent() {
        let (mut manager, _prod, _cons, _base, mut output) = uncompressed_then_empty();
        drain_residue(&mut manager, &mut output);

        for _ in 1..=7 {
            manager.fill_output(&mut output, 1.0);
        }
        assert_eq!(manager.flow.conceal_run, 7);

        let g = gain(&output);
        assert!(
            g > 0.5,
            "callback 7 emitted gain {g}; the previous schedule emitted exactly \
                 0.0 here and a floor-only change would emit {CONCEAL_FADE_FLOOR}",
        );
        assert!(
            g < 1.0 - 1e-6,
            "but it must be faded — {g} means the fade never started",
        );
    }

    /// **The Opus path takes the same schedule**, and must, or the change would
    /// fix the uncompressed dropout and leave the 128kbps one standing. A field
    /// round measured 477 concealed frames on 128kbps, every one through
    /// the codec (`pitch_conceals` 0/456 windows) — and every one muted to zero
    /// by the old schedule just the same.
    ///
    /// Asserted as a ratio against the un-faded decoder rather than an absolute
    /// level, because Opus PLC decays on its own: measured 0.501 → 0.007 across
    /// these callbacks with the fade removed entirely. An absolute assertion
    /// here would be measuring the codec.
    #[test]
    fn the_opus_concealment_path_should_take_the_same_floor() {
        let (mut manager, _encoder, _prod, _cons, _base, mut output) =
            super::conceal_run::playing_then_empty();
        assert!(
            manager.decoder.plc_ready(),
            "precondition: a decoded Opus frame leaves the codec able to extrapolate",
        );
        drain_residue(&mut manager, &mut output);

        let mut zeros = 0;
        for _ in 1..=20 {
            manager.fill_output(&mut output, 1.0);
            if output.iter().all(|s| *s == 0.0) {
                zeros += 1;
            }
        }

        assert_eq!(
            manager.log_window.tally.pitch_conceals, 0,
            "precondition: the Opus path must stay on the codec",
        );
        assert_eq!(
            zeros, 0,
            "{zeros} of 20 concealed Opus callbacks were entirely silent; the \
                 old schedule zeroed callbacks 7..=20 here, which is 14",
        );
    }
}

/// The 1 Hz line must report how much audio was emitted at the fade floor.
///
/// The floor accepts one risk: a long run repeats a single pitch period at the
/// floor gain for its whole tail, where upstream rotates three lags
/// (`expand.cc:844-853`) so consecutive expansions are never identical. The risk
/// is a *long* run, not a frequent one — and `conceal_run_max` cannot separate
/// those, because it reports the window's longest run without saying how much of
/// it reached the floor.
mod floor_frames {
    use super::pitch_concealment::uncompressed_then_empty;
    use super::*;

    /// The counter must track the floor and nothing else: silent until the
    /// schedule arrives there, then one per callback.
    ///
    /// The pre-floor assertion is what makes this falsifiable. A counter wired
    /// to "the fade is active" rather than "the fade is at its floor" would read
    /// non-zero from callback 4 and pass any assertion that only checked the
    /// tail.
    #[test]
    fn the_depth_line_should_count_frames_emitted_at_the_fade_floor() {
        let (mut manager, _prod, _cons, _base, mut output) = uncompressed_then_empty();

        // 13 callbacks: the schedule's last un-floored gain is
        // `1.0 - 10/12 = 0.1667`, still above `CONCEAL_FADE_FLOOR`.
        for _ in 1..=13 {
            manager.fill_output(&mut output, 1.0);
        }
        assert_eq!(manager.flow.conceal_run, 13);
        assert_eq!(
            manager.log_window.tally.floor_frames, 0,
            "the fade is active but has not reached its floor; a counter that \
                 reads the branch rather than the value would already be at 10",
        );

        for expected in 1..=8u32 {
            manager.fill_output(&mut output, 1.0);
            assert_eq!(
                manager.log_window.tally.floor_frames, expected,
                "each floored callback must count exactly once",
            );
        }
    }

    /// A window observation, not a total — the same contract every other
    /// `TimescaleTally` field carries, and the reason none of them can latch
    /// their own history.
    #[test]
    fn the_fade_floor_counter_should_clear_with_the_window() {
        let (mut manager, _prod, _cons, _base, mut output) = uncompressed_then_empty();

        for _ in 1..=20 {
            manager.fill_output(&mut output, 1.0);
        }
        assert!(
            manager.log_window.tally.floor_frames > 0,
            "precondition: the counter must be standing, or the flush below is \
                 unobservable",
        );

        // Flush on a concealed callback. Unlike `conceal_run_max`, this counter
        // is written *after* `log_depth_authority` has taken the tally and the
        // run is still standing, so the new window legitimately opens at 1 — a
        // reset to 0 would be the wrong assertion here, and asserting it would
        // require flushing on a played frame, which cannot happen mid-hold.
        manager.log_window.frame_count = LOG_INTERVAL_CALLBACKS - 1;
        manager.fill_output(&mut output, 1.0);
        assert_eq!(
            manager.log_window.tally.floor_frames, 1,
            "the 1 Hz flush must clear the count, leaving only the callback that \
                 followed it — a running total would report the whole session",
        );
    }
}
