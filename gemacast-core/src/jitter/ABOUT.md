# Gemacast Jitter Buffer — Complete Technical Reference

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Module Structure](#module-structure)
3. [Data Flow Pipeline](#data-flow-pipeline)
4. [Constants Reference](#constants-reference)
5. [`types.rs` — Packet Representation](#typesrs--packet-representation)
6. [`buffer.rs` — Circular Reorder Buffer](#bufferrs--circular-reorder-buffer)
7. [`manager.rs` — The Brain](#managerrs--the-brain)
   - [Struct Fields](#struct-fields)
   - [Constructor & Helpers](#constructor--helpers)
   - [Packet Ingestion (`ingest_packets`)](#packet-ingestion)
   - [Audio Output (`fill_output`)](#audio-output)
   - [Frame Processing (`process_next_frame`)](#frame-processing)
   - [Acceleration (Time-Shrinking)](#acceleration-time-shrinking)
   - [Preemptive Expand (Time-Stretching)](#preemptive-expand-time-stretching)
   - [Flush with Crossfade](#flush-with-crossfade)
   - [Decoder Interface](#decoder-interface)
   - [Packet Loss Concealment](#packet-loss-concealment)
8. [Configuration & Presets](#configuration--presets)
9. [Design Lineage: WebRTC NetEQ](#design-lineage-webrtc-neteq)

---

## Architecture Overview

The jitter buffer sits between the **network thread** (receives UDP/TCP packets) and the **audio callback thread** (feeds the OS audio driver at real-time deadlines). Its job is to:

1. **Reorder** packets that arrive out of sequence (common on UDP)
2. **Buffer** enough frames to absorb network jitter without underruns
3. **Accelerate** playback when the buffer grows too deep (reduces latency)
4. **Expand** playback when the buffer is dangerously shallow (prevents gaps)
5. **Conceal** lost packets using Opus PLC (packet loss concealment)

```
Network Thread                    Audio Callback Thread
┌─────────────┐    SPSC Ring     ┌─────────────────────────────────┐
│ UDP/TCP Recv │───────────────▶ │ ingest_packets()                │
│ parse_packet │  (lock-free)    │   └─▶ JitterBuffer.insert()     │
└─────────────┘                  │                                 │
                                 │ fill_output() ◀── OS callback   │
                                 │   └─▶ process_next_frame()      │
                                 │         ├─ pop_next + decode     │
                                 │         ├─ accelerate / expand   │
                                 │         └─ PLC if missing        │
                                 └─────────────────────────────────┘
```

### Thread Safety Model

- **Zero shared mutable state.** The `JitterBuffer` and `JitterBufferManager` are owned entirely by the audio thread.
- **Lock-free ingestion.** Raw packets cross the thread boundary via a `ringbuf` SPSC (single-producer single-consumer) ring buffer. The network thread pushes; the audio thread pops.
- **Shared atomics** for read-only signals: `latency_metric` (AtomicU32) lets the UI read the current latency; `is_tcp_mode` (AtomicBool) tells the jitter logic whether the transport is TCP/ADB.
- **Config updates** use an `Arc<RwLock<JitterConfig>>`. The audio thread polls it every 100 callbacks (~500ms) via `try_read()` — never blocks.

---

## Module Structure

```
gemacast-core/src/jitter/
├── mod.rs      — Module exports
├── types.rs    — RawPacket struct (inline-array, zero-alloc)
├── buffer.rs   — JitterBuffer: circular reorder buffer (O(1) insert/pop)
└── manager.rs  — JitterBufferManager: the full pipeline (decode, adapt, WSOLA)
```

---

## Data Flow Pipeline

Every ~2.67ms (cpal's 128-sample callback at 48kHz), the OS audio driver calls `fill_output()`. Here's the complete flow:

```
fill_output(output_buf)
│
├── while output not full:
│   ├── if playback_buf has samples → bulk-copy to output (with volume)
│   └── if playback_buf empty → process_next_frame()
│
process_next_frame()
│
├── 1. IIR-filter the buffer level (α=254/256)
├── 2. Bleed starvation_bump (proportional decay)
├── 3. Tick cooldown counters (starvation_bump, timescale, recovery)
├── 4. Hot-reload config (every 100 callbacks)
├── 5. Compute adaptive target depth
│   ├── Static mode: use user-specified ms
│   └── Adaptive mode: min_depth + jitter_margin + starvation_bump
│       ├── jitter_margin = (ema_jitter*2 + ema_peak) * margin_scale
│       └── Clamped to [min_depth, comfort_cap]
├── 6. Hysteresis + quantization + rate-limited ramping
│   ├── Quantize to adaptive quantum grid (1 for low-cap presets, 4 for others)
│   ├── Require HYSTERESIS_DWELL (40) callbacks outside band to commit
│   └── Ramp ±1 frame per RAMP_INTERVAL (5) callbacks
├── 7. Flush ceiling check
│   ├── No-buffer mode: flush to target+1 if > target+3
│   └── Normal mode: gentle flush to 3× target if > 5× target
├── 8. Prebuffering gate
│   └── Wait until buffer ≥ resume_threshold_pct × target
├── 9. Gap handling (reorder tolerance)
│   └── Hold for REORDER_TOLERANCE (6) callbacks before declaring loss
├── 10. Pop next packet + decode
├── 11. Decision: Normal / Accelerate / Expand
│   ├── If buffer > threshold AND not in starvation recovery:
│   │   ├── Silence fast-forward (RMS < 0.005): shed up to 4 extra frames
│   │   ├── Fast acceleration (buffer ≥ 3× target): NCC 0.5, no cooldown
│   │   └── Normal acceleration: NCC 0.9, cooldown = 6 callbacks
│   ├── If buffer < min_depth (low):
│   │   └── Preemptive expand (stretch by one pitch period)
│   └── Otherwise: normal playback
├── 12. Starvation path (no packet available):
│   ├── Increment starvation counter
│   ├── If starvation > adaptive threshold → enter prebuffering
│   └── Generate PLC (Opus concealment)
```

---

## Constants Reference

| Constant | Value | Meaning |
|---|---|---|
| `OLA_LEN` | 128 samples | Hann window length for OLA crossfading (~2.67ms at 48kHz) |
| `SEARCH_RANGE` | 720 samples | Cross-correlation search window (~15ms, covers full human pitch range) |
| `MILLIS_PER_FRAME` | 10ms | Duration of one Opus frame (480 samples / 48000 Hz × 1000) |
| `MAX_MISSING` | 200 frames | 2000ms of silence before full stream reset |
| `REORDER_TOLERANCE` | 3 frames | ~30ms window to wait for reordered packets on WiFi |
| `HYSTERESIS_BAND` | 3 frames (default) | Target must deviate by >N frames to trigger change. **Adaptive**: 1 for low-cap presets (≤80ms) |
| `HYSTERESIS_DWELL` | 15 callbacks | Must stay outside band for 15 callbacks (~150ms) to commit (Adaptive based on preset) |
| `TARGET_QUANTUM` | 4 frames (default) | Snap target to multiples of N. **Adaptive**: 1 for low-cap (≤80ms), 2 for high-cap (≥1000ms) |
| `STARVATION_COOLDOWN` | 200 callbacks | ~2s lockout after starvation bump (prevents ratcheting) |
| `RAMP_INTERVAL` | 5 callbacks | Move ±1 frame every 5 callbacks (~50ms ramp steps) |
| `MIN_TIMESCALE_INTERVAL` | 6 callbacks | Minimum gap between acceleration/expansion ops (~60ms) |
| `PROBE_DOWN_INTERVAL` | 200 callbacks | ~2s between downward probe attempts when network is stable |

---

## `types.rs` — Packet Representation

### `RawPacket`

Represents a single audio packet received from the network, stored **undecoded**.

```rust
pub struct RawPacket {
    pub seq_num: u64,                          // Monotonic sender sequence number
    pub payload_data: [u8; MAX_PACKET_PAYLOAD], // Inline array (no heap alloc)
    pub payload_len: usize,                     // Actual bytes used
    pub arrival_time: Instant,                  // NIC arrival timestamp
    pub is_uncompressed: bool,                  // Raw f32 PCM (bypass Opus)
    pub is_silence: bool,                       // Intentional silence (no payload)
}
```

**Key design decisions:**

- **Inline array vs Vec**: The `payload_data` field uses a fixed-size `[u8; MAX_OPUS_PACKET_SIZE]` instead of `Vec<u8>`. This eliminates ~200 heap allocations/sec on the network thread (one per packet at 5ms intervals). The SPSC ring buffer pre-allocates all slots at startup.
- **Undecoded storage**: Packets are stored as raw Opus bytes. The Opus decoder lives entirely on the audio thread, which is required for PLC — Opus PLC depends on the decoder's internal state machine from the previous successfully decoded frame.

### `RawPacket::zeroed()`

Creates a zero-initialized packet. Used only as a placeholder during buffer/test setup. Real packets are populated by `parse_packet` on the network thread.

---

## `buffer.rs` — Circular Reorder Buffer

### `JitterBuffer`

A fixed-capacity circular buffer that automatically reorders UDP packets by sequence number.

```
Capacity: 512 slots × 5ms/frame = 2.56 seconds of maximum buffering
```

#### Design

Each slot corresponds to `seq_num % 512`. When a packet arrives, it's placed directly at its computed index — out-of-order packets "land" in the correct position automatically. The `next_play_seq` cursor tracks what we expect to play next.

```
Slots:  [_, _, P2, _, P4, P5, _, P7, ...]
         ^                              
    next_play_seq = 2
```

#### Fields

| Field | Type | Purpose |
|---|---|---|
| `slots` | `Vec<Option<RawPacket>>` | 512 pre-allocated packet slots |
| `next_play_seq` | `u64` | The sequence number expected for the next `pop_next()` |
| `initialized` | `bool` | Set to `true` on first packet (anchors `next_play_seq`) |
| `occupied` | `u32` | O(1) count of filled slots (maintained incrementally) |
| `cached_min_seq` | `Option<u64>` | Lazy-cached minimum sequence for gap detection |

#### Functions

##### `insert(packet) → InsertResult`

Places a packet in its sequence-indexed slot.

- **Accepted**: Normal insertion. Increments `occupied`.
- **Stale**: Packet is behind `next_play_seq` (already played). Rejected.
- **StreamRestarted**: Packet is far behind (> capacity). This means the sender restarted. Resets the entire buffer and re-anchors to the new sequence.

If a packet arrives far _ahead_ (>= `next_play_seq + capacity`), the buffer calls `skip_to()` to advance the playhead, clearing stale slots.

##### `pop_next() → Option<RawPacket>`

Consumes the packet at `next_play_seq` and advances the cursor.

- Returns `Some(pkt)` if the slot contains the expected packet.
- Returns `None` if the slot is empty (gap/loss) or contains a future packet. Always advances `next_play_seq` to prevent deadlocks.

##### `has_next() → bool`

Peeks without consuming. Returns `true` if the slot at `next_play_seq` contains a packet with a matching sequence number.

##### `advance_one()`

Skips one slot unconditionally. Used when the gap-hold timer expires and we accept the loss.

##### `occupied_count() → u32`

O(1) count of filled slots. Maintained incrementally on insert/pop.

##### `lowest_available_seq() → Option<u64>`

Finds the minimum sequence number in the buffer. Uses a lazy cache: O(1) on cache hit, O(n) recompute after mutations. Used by the gap-hold logic to decide whether to fast-forward past a gap.

##### `fast_forward(next_seq)`

Advances `next_play_seq` to `next_seq`, clearing all skipped slots. Used when the gap is too large to wait for reordering.

##### `skip_to(new_seq)` (private)

The underlying skip implementation. Clears each skipped slot individually if `skip_distance < capacity`, or does a full bulk clear if larger.

##### `reset()`

Clears all slots and resets to uninitialized state. Called on disconnect/reconnect.

---

## `manager.rs` — The Brain

### Struct Fields

The `JitterBufferManager` struct owns all state for the jitter pipeline:

#### Core Pipeline
| Field | Type | Purpose |
|---|---|---|
| `decoder` | `Decoder` | Opus decoder instance (single-threaded, audio thread only) |
| `buffer` | `JitterBuffer` | The circular reorder buffer |
| `playback_buf` | `VecDeque<f32>` | Accumulator of decoded PCM ready for the OS audio driver |
| `decode_buf` | `Vec<f32>` | Pre-allocated scratch for Opus decode output (960 samples) |
| `decode_len` | `usize` | Valid sample count in `decode_buf` after last decode |
| `wsola_buf` | `Vec<f32>` | Pre-allocated scratch for WSOLA crossfade (holds "old" PCM) |
| `hann_window` | `Vec<f32>` | Pre-computed Hann window coefficients for OLA crossfading |

#### State Machine
| Field | Type | Purpose |
|---|---|---|
| `is_prebuffering` | `bool` | True while accumulating initial buffer depth |
| `missing_count` | `u32` | Consecutive frames with no packet (drives PLC / reset) |
| `starvation_count` | `u32` | Consecutive frames where buffer was completely empty |
| `gap_hold_count` | `u32` | How long we've been waiting for a reordered packet |
| `opus_next_expected_seq` | `Option<u64>` | Tracks decoder's expected sequence for gap PLC |

#### Jitter Statistics
| Field | Type | Purpose |
|---|---|---|
| `ema_jitter` | `f32` | EWMA of inter-arrival jitter in frames (fast attack, slow decay) |
| `ema_peak` | `f32` | Slow-decay peak tracker — only bumped by verified recurring peaks |
| `ema_jitter_var` | `f32` | EWMA of jitter² for variance/CV tracking |
| `ema_peak_decay_alpha` | `f32` | Per-tick decay multiplier for `ema_peak` (from config halflife) |
| `clean_streak` | `u32` | Consecutive packets below adaptive jitter threshold |
| `starvation_bump` | `f32` | Additive target boost after starvation (bleeds proportionally) |
| `last_ingest_seq` | `Option<u64>` | Previous ingested seq for IAT computation |
| `last_network_arrival` | `Option<Instant>` | Previous arrival timestamp for IAT computation |
| `filtered_buffer_level` | `f32` | IIR-filtered buffer occupancy (α=254/256, ignores batching spikes) |

#### Network Regime Detection
| Field | Type | Purpose |
|---|---|---|
| `last_macro_spike` | `Option<Instant>` | When the last >50ms jitter spike occurred |
| `unstable_regime_until` | `Option<Instant>` | If frequent spikes, lock into "unstable" mode for 60s |

#### NetEQ Peak State Machine
| Field | Type | Purpose |
|---|---|---|
| `peak_history` | `VecDeque<(u64, f32)>` | Recent peaks: (period_ms, height_frames), up to 8 |
| `last_peak_time` | `Option<Instant>` | When the last peak was observed |
| `peak_mode_active` | `bool` | True if ≥2 peaks seen within their max period |

#### Hysteresis & Ramping
| Field | Type | Purpose |
|---|---|---|
| `effective_target` | `u32` | The currently locked-in target depth (frames) |
| `target_exit_count` | `u32` | Consecutive callbacks raw target has been outside the band |
| `ramp_goal` | `u32` | The quantized goal that `effective_target` is ramping toward |
| `ramp_countdown` | `u32` | Rate-limiter: one step per `RAMP_INTERVAL` callbacks |

#### Safety Guards
| Field | Type | Purpose |
|---|---|---|
| `starvation_bump_cooldown` | `u32` | Lockout after bump (prevents positive-feedback ratcheting) |
| `starvation_recovery` | `u32` | Post-starvation acceleration suppression (50 callbacks / ~500ms) |
| `timescale_cooldown` | `u32` | Minimum gap between OLA operations (prevents clicking) |
| `probe_down_countdown` | `u32` | Timer for downward target probing when network is stable |
| `probe_floor` | `u32` | Learned floor: lowest target that caused starvation |

#### Configuration
| Field | Type | Purpose |
|---|---|---|
| `config` | `JitterConfig` | Local cached copy of user settings |
| `config_ref` | `Arc<RwLock<JitterConfig>>` | Shared config for hot-reload |
| `config_check_countdown` | `u32` | Only poll config lock every 100 callbacks |
| `is_tcp_mode` | `Arc<AtomicBool>` | True if transport is TCP/ADB (not UDP) |
| `latency_metric` | `Arc<AtomicU32>` | Published latency (ms) for UI display |

---

### Constructor & Helpers

#### `new(decoder, latency_metric, config_ref, is_tcp_mode) → Self`

Initializes all fields, pre-computes the Hann window and peak decay alpha.

#### `ms_to_frames_ceil(ms) → u32`

Converts milliseconds to frames using ceiling division. Prevents truncation to 0 for sub-frame values (e.g. 2ms / 5ms = 1 frame, not 0).

#### `make_hann_window() → Vec<f32>`

Pre-computes 128 Hann window coefficients: `w[i] = 0.5 × (1 - cos(2πi/N))`. Used for all OLA crossfades throughout the system.

#### `min_depth_frames() → u32`

Converts `config.min_depth_ms` to frames. This is the user-specified absolute floor.

#### `comfort_cap_frames() → f32`

Converts `config.comfort_cap_ms` to frames. This is the ceiling cap on the adaptive target.

#### `stability_ratio() → f32`

Returns 0.0 (unstable) to 1.0 (highly stable). Ramps linearly: `clean_streak / 400`. Full stability = 400 consecutive clean packets (~2 seconds).

#### `clean_threshold() → f32`

Adaptive threshold for what counts as "clean" jitter. On 2.4GHz where baseline jitter is ~4-8 frames, the threshold rises to `~7-13 frames`. On 5GHz (~0.5 frames), the threshold is `~1.75 frames`. Formula: `(ema_jitter × 1.5 + 1.0).min(10.0)`.

#### `quantize_target(raw, quantum) → u32`

Snaps a raw target to the nearest multiple of `quantum`. The quantum is adaptive (see below).

#### `adaptive_quantum() → u32`

Returns `1` when `comfort_cap ≤ 8 frames` (80ms), `2` when `comfort_cap >= 100` (1000ms) for finer settling without overshoot, and `4` otherwise. This prevents low-latency presets (Wired, Fast) from having a raw target of 2 snapped up to 4.

#### `adaptive_hysteresis() → u32`

Returns `1` when `comfort_cap ≤ 8 frames`, `3` otherwise. Narrow band for low-cap presets allows the target to distinguish between 1, 2, 3 frames instead of treating them all as within the dead-zone.

#### `compute_target_depth(tcp_cap_override) → u32`

The core adaptive target formula:

```
target = min_depth + jitter_margin + starvation_bump
jitter_margin = (ema_jitter × 2 + ema_peak) × margin_scale
margin_scale = 1 - stability × 0.4    (at full stability: 60% of raw margin)
```

Clamped to `[min_depth, comfort_cap]`. In static mode, returns the fixed user value.

#### `lerp(a, b, t) → f32`

Standard linear interpolation: `a + (b - a) × t`.

#### `get_rms(samples) → f32`

Root mean square energy of a PCM buffer. Used to distinguish silence (< 0.005) from active audio for VAD decisions.

---

### Packet Ingestion

#### `ingest_packets(consumer: &mut HeapCons<RawPacket>)`

Called at the start of each audio callback. Drains all pending packets from the SPSC ring buffer into the `JitterBuffer`. For each packet, it computes jitter statistics:

1. **Inter-Arrival Time (IAT)**: `actual_arrival_gap - expected_gap` for consecutive packets. Only computed for forward sequence progress (ignores reordered packets).

2. **Clean streak tracking**: If jitter < adaptive threshold → increment. Moderate spikes halve the streak; severe spikes (>2× threshold) slam to zero.

3. **Jitter variance**: EWMA of jitter² (`α=0.05`). Combined with `ema_jitter`, yields coefficient of variation for network classification.

4. **Stability-aware jitter EMA**: Fast attack (α=0.15) on spikes. Decay rate scales with stability — stable networks forget jitter quickly (α=0.04), unstable networks retain memory much longer (α=0.005).

5. **Peak decay (stability-aware)**: When `peak_decay_halflife_ms = 0` (Auto mode), the halflife is interpolated from 34.6s (unstable) to 1.5s (stable) based on the stability ratio. Fixed presets use their configured halflife directly.

6. **Macro spike detection**: Jitter spikes >50ms are tracked. If two spikes occur within 10s, the "unstable regime" flag is set for 60 seconds, locking peak decay to the slowest setting.

7. **NetEQ 2-Peak Trigger State Machine**: Peaks exceeding `target + 3.9 frames` or `2× target` are recorded in a history ring (up to 8 entries, 10s window). When ≥2 peaks exist, `peak_mode_active` is set. The `ema_peak` tracker only jumps on verified recurring peaks — single spurious spikes are ignored.

8. **Buffer insertion**: The packet is inserted into the `JitterBuffer`. If the result is `StreamRestarted`, the Opus decoder state is reset.

---

### Audio Output

#### `fill_output(output: &mut [f32], volume: f32)`

Called by the OS audio driver (cpal callback). Fills the output buffer with PCM samples.

- Bulk-copies from `playback_buf` (a `VecDeque`) using its contiguous slices for SIMD-friendly access.
- Applies volume scaling per-sample.
- If `playback_buf` runs dry, calls `process_next_frame()` to decode and process the next Opus frame.
- If still empty after processing (complete buffer starvation), zero-fills the output.

---

### Frame Processing

#### `process_next_frame()`

The central decision function. Called when `playback_buf` is empty and needs refilling. This is the equivalent of NetEQ's `GetAudio()` + `GetDecision()`.

**Phase 1: Housekeeping**

- **IIR buffer filter**: Smooths instantaneous buffer level with α=254/256. This prevents USB batching spikes (10 packets arriving at once) from triggering spurious flushes.
- **Starvation bump bleed**: Proportional decay: `bleed = 0.05 + bump × 0.03`. Bigger bumps recover faster.
- **Cooldown ticks**: Decrements `starvation_bump_cooldown`, `timescale_cooldown`, `starvation_recovery`.
- **Config hot-reload**: Every 100 callbacks, polls `config_ref` via `try_read()`. On change, resets all adaptive state and triggers a one-time flush if the buffer is too deep for the new config.

**Phase 2: Target Computation**

- Computes the raw adaptive target via `compute_target_depth()`.
- TCP/ADB mode caps the target at 12 frames (60ms) unless the user set a static target.
- Applies **hysteresis**: target must deviate by > `HYSTERESIS_BAND` (3 frames) for `HYSTERESIS_DWELL` (40 callbacks / 200ms) before committing.
- Applies **quantization**: snaps to 1, 2, or 4-frame grid.
- Applies **rate-limited ramping**: moves ±1 frame per `RAMP_INTERVAL` (5 callbacks / 50ms), symmetrically in both directions.
- When the network is stable and the target is at its ramp goal, **downward probing** slowly nudges the target lower (one quantum step every ~1 second) to discover the true minimum. The `probe_floor` prevents probing below a level that previously caused starvation.

**Phase 3: Flush Ceiling**

- **No-buffer mode**: Hard flush to `target+1` if buffer exceeds `target+3`.
- **Normal mode**: Gentle flush to `3× target` if buffer exceeds `5× target`. This prevents unbounded buildup while giving acceleration plenty of headroom.

**Phase 4: Prebuffering Gate**

If `is_prebuffering` is true, check if buffer has accumulated enough frames (`resume_threshold_pct × target`). If not, generate PLC and return.

**Phase 5: Gap Handling**

If there are packets in the buffer but `has_next()` is false (the next expected sequence is missing), the system waits for `REORDER_TOLERANCE` (6 callbacks / ~30ms) before either:
- Fast-forwarding to `lowest_available_seq()` (if the gap is small)
- Advancing one slot (accepting the loss)

For large gaps (>20 packets), the Opus decoder state is reset to avoid hallucinated audio.

**Phase 6: Starvation Recovery**

When emerging from starvation (`starvation_count > 0`):
- **Fade-in**: Applies a 2ms linear fade-in on the first real decoded packet to mask the spectral discontinuity from the preceding Opus PLC prediction.
- Sets `starvation_recovery = 50` — suppresses ALL acceleration for 500ms. This matches WebRTC's `prev_mode != kModeExpand` guard.
- Applies a **starvation bump** (temporary additive target boost):
  - Probe failure (stable network, likely too-aggressive probing): mild bump (one quantum step), sets `probe_floor`.
  - Genuine outage: larger bump based on `ema_peak × 1.5 + 2`.
- Bypasses hysteresis dwell for immediate upward ramp.

**Phase 7: Decode & Decision**

Pops the next packet, decodes it via `capture_pcm()`, then makes the time-stretching decision:

1. **Accelerate** (buffer > threshold AND not in starvation recovery):
   - **Silence fast-forward**: If RMS < 0.005 (silent frame), skip up to 4 extra frames directly (no WSOLA needed). Floor check prevents draining below target.
   - **Fast acceleration** (buffer ≥ 3× target): Uses NCC threshold 0.5, no cooldown, removes multiple pitch periods per operation. Always fires regardless of audio content.
   - **Normal acceleration**: Uses NCC threshold 0.9, cooldown of 6 callbacks. **Energy gated**: only fires during quiet passages (`rms < 0.08`) where crossfade artifacts are masked.

2. **Preemptive expand** (buffer < min_depth): Stretches the frame by one pitch period to slow consumption and prevent imminent starvation. **Energy gated**: only fires on quiet audio (`rms < 0.08 && rms > 0.001`) to prevent stretching artifacts on loud music.

3. **Normal**: Just append the decoded PCM to `playback_buf`.

**Phase 8: Starvation Path**

If no packet is available:
- Increment `missing_count` and `starvation_count`.
- If `missing_count > MAX_MISSING` (2 seconds): full reset.
- If `starvation_count > adaptive_threshold`: enter prebuffering. The threshold is higher on jittery networks (base 10 + up to `ema_peak`, max 40).
- Generate PLC audio via `generate_plc()`.

---

### Acceleration (Time-Shrinking)

Three acceleration mechanisms, from most aggressive to gentlest:

#### Silence Fast-Forward

When the decoded frame is silent (RMS < 0.005), skip extra frames directly:

```
excess = occupied - wsola_threshold
shed_count = min(excess/2, 4)
for each shed: if buffer still > threshold, pop and decode (discard)
```

No crossfade needed — silence is silence. Floor check prevents draining below target.

#### `try_accelerate_internal(fast_mode: bool) → bool`

Single-frame **autocorrelation** acceleration. Finds repeating pitch periods within the current `decode_buf` and removes one (or more in fast mode).

**Step 1: Autocorrelation pitch detection**

```
Reference window: last OLA_LEN (128) sample-frames of the frame
Search window:    [0 .. anchor - OLA_LEN]
```

For each candidate offset `d`, computes **normalized cross-correlation (NCC)**:

```
NCC(d) = Σ(ref[i] × cand[d+i]) / sqrt(Σ(ref[i]²) × Σ(cand[d+i]²))
```

Uses **mono downmix** for the correlation (halves FMA count), then applies the crossfade on full stereo.

**Step 2: Threshold check**

- Normal mode: NCC ≥ 0.9 (strong correlation required for transparent quality)
- Fast mode: NCC ≥ 0.5 (trades quality for faster drain when buffer is critically overfull)

**Step 3: Pitch period removal**

The pitch period = `anchor - best_d` (distance between matching section and reference).

- **Normal mode**: removes exactly one pitch period
- **Fast mode**: removes MULTIPLE pitch periods — `floor(half_frame / pitch_period) × pitch_period`. This matches NetEQ `accelerate.cc:62-67`:
  ```cpp
  peak_index = (fs_mult_120 / peak_index) * peak_index;
  ```
  Removing more per-operation means fewer OLA crossfades needed, which eliminates clicking artifacts.

**Step 4: OLA crossfade**

```
Output = [0..best_d] verbatim
       + Hann crossfade([best_d..], [splice_start..])
       + [splice_start + OLA_LEN ..] verbatim

splice_start = best_d + remove_len
```

The Hann window provides a smooth power-complementary blend: `early × (1-w) + late × w`.

#### `try_wsola_overlap_add_internal(pcm1_len, force_crossfade) → bool`

Cross-packet WSOLA splice. Used by `flush_with_crossfade` to splice pre-flush and post-flush audio. Correlates the tail of `wsola_buf` (old audio) against the head of `decode_buf` (new audio) to find the best phase-aligned splice point.

---

### Preemptive Expand (Time-Stretching)

#### `try_wsola_expand_internal() → bool`

When the buffer is dangerously low (< `min_depth`), this **stretches** the current frame by one pitch period to slow consumption.

Uses the same autocorrelation approach as acceleration, but instead of _removing_ a pitch period, it _duplicates_ one:

```
Output = [0..anchor] verbatim
       + Hann crossfade([anchor..], [best_d..])   ← re-plays a pitch period
       + [best_d + OLA_LEN ..] verbatim
```

This effectively adds ~3-15ms of audio without any audible artifact (when NCC > 0.9). The frame plays back slightly slower, giving the buffer more time to accumulate packets.

Only applied to active audio (RMS > 0.005) — stretching silence would amplify background noise.

---

### Flush with Crossfade

#### `flush_with_crossfade(flush_to: u32)`

Rapidly reduces buffer depth to `flush_to` frames while maintaining audio continuity.

1. **Snapshot**: Copy current `decode_buf` → `wsola_buf` (pre-flush audio)
2. **Skip**: Pop and decode every frame until `occupied_count() ≤ flush_to`. Each skipped frame is fed to the Opus decoder to keep its state warm (prevents the hard transient click that `reset_state()` would cause).
3. **Crossfade**: Use `try_wsola_overlap_add_internal` to blend pre-flush audio (in `wsola_buf`) with post-flush audio (in `decode_buf`). If correlation is too weak, falls back to concatenation.

Used in two contexts:
- **No-buffer mode**: Aggressive flush to `target+1` when buffer exceeds `target+3`
- **Normal mode**: Gentle flush to `3× target` when buffer exceeds `5× target`
- **Config change**: Flush to `1.5× new_target` on hot-reload

---

### Decoder Interface

#### `capture_pcm(pkt: &RawPacket)`

Decodes a packet into `decode_buf`. Handles three payload types:

1. **Silence frames** (`is_silence`): Zero-fills `decode_buf` without touching the decoder. Feeding PLC for silence would poison the decoder's spectral state.

2. **Uncompressed PCM** (`is_uncompressed`): Copies raw f32 bytes directly. No decoder interaction.

3. **Opus encoded**: Calls `decode_opus()`. On failure, falls back to PLC via `decode_plc_to_buf()`.

**Gap handling**: If the packet's sequence number doesn't match `opus_next_expected_seq`:
- Gap > 20: Full decoder reset (discontinuity too large for PLC)
- Gap 1-5: Feed PLC frames to warm the decoder (smooth concealment)
- Gap 6-20: Let the decoder continue (PLC quality degrades naturally)

#### `decode_opus(data) → bool`

Calls `decoder.decode_float()`. Updates `decode_len` with the actual output length.

#### `decode_plc_to_buf()`

Calls `decoder.decode_float(&[])` — Opus interprets an empty payload as "generate PLC". On error, zero-fills.

---

### Packet Loss Concealment

#### `generate_plc()`

Called when no packet is available. Generates one frame of PLC audio via `decode_plc_to_buf()` and appends it to `playback_buf`.

Opus PLC works by extrapolating the decoder's internal spectral state from the last successfully decoded frame. Quality degrades over multiple consecutive PLC frames. To prevent robotic artifacts, the generated PLC audio is gradually **faded to silence** over frames 4-7.

---

### Reset

#### `trigger_reset()` (private)

Full state reset. Clears the buffer, resets all adaptive state (jitter EMA, peaks, stability, hysteresis, probe floor), resets the Opus decoder, and enters prebuffering mode. Called after 2 seconds of complete silence (`MAX_MISSING`).

#### `reset()` (public)

Public wrapper for `trigger_reset()`. Called on disconnect.

---

## Configuration & Presets

The `JitterConfig` struct controls the adaptive algorithm's boundaries:

```rust
pub struct JitterConfig {
    pub min_depth_ms: u32,           // Floor for adaptive target
    pub comfort_cap_ms: u32,         // Ceiling for adaptive target
    pub peak_decay_halflife_ms: u32, // How long to remember jitter spikes (0 = Auto)
    pub resume_threshold_pct: f32,   // % of target to refill before resuming
    pub static_target_ms: Option<u32>, // Bypass adaptive logic entirely
}
```

### How presets map to behavior

| Preset | min_depth | comfort_cap | peak_halflife | resume_pct | quantum | hysteresis | Effect |
|---|---|---|---|---|---|---|---|
| **Auto** | 25ms | 1000ms | 0 (auto) | 25% | **2** | 3 | Adapts to any network. Halflife interpolated by stability. |
| **Wired** | 2ms | 20ms | 500ms | 20% | **1** | **1** | Fine-grained. Raw target of 2 stays 2 (not snapped to 4). |
| **Fast** | 5ms | 40ms | 800ms | 25% | **1** | **1** | Fine-grained. Target tracks jitter precisely. |
| **Balanced** | 25ms | 150ms | 3.5s | 40% | 4 | 3 | Coarse grid reduces transition noise. |
| **Stable** | 50ms | 500ms | 34.6s | 50% | 4 | 3 | High floor, very long memory. |
| **Resilient** | 80ms | 1000ms | 34.6s | 70% | **2** | 3 | Worst networks. 80ms floor, 1s cap. |
| **No Buffer** | 0ms | 0ms | 1s | 0% | — | — | `static_target_ms = 0`. Bypasses adaptive logic. |

---

## Design Lineage: WebRTC NetEQ

This jitter buffer is heavily inspired by **WebRTC's NetEQ** (the audio jitter buffer used in Google Meet, Chrome, etc.). Key concepts borrowed:

| NetEQ Concept | Our Implementation | Source File |
|---|---|---|
| `DecisionLogic::GetDecision()` | `process_next_frame()` decision tree | `decision_logic.cc` |
| `BufferLevelFilter` (IIR α=254/256) | `filtered_buffer_level` | `buffer_level_filter.cc` |
| `DelayManager::CalculateTargetLevel()` | `compute_target_depth()` | `delay_manager.cc` |
| `DelayPeakDetector` (2-peak trigger) | `peak_history` + `peak_mode_active` | `delay_peak_detector.cc` |
| `Accelerate::CheckCriteriaAndStretch()` | `try_accelerate_internal()` | `accelerate.cc` |
| `PreemptiveExpand::Process()` | `try_wsola_expand_internal()` | `preemptive_expand.cc` |
| `TimeStretch` autocorrelation | Autocorrelation pitch detection | `time_stretch.cc` |
| `kCorrelationThreshold = 0.9` | `threshold = 0.9` (normal) / `0.5` (fast) | `time_stretch.h:88` |
| `kMinTimescaleInterval = 5` | `MIN_TIMESCALE_INTERVAL = 6` | `decision_logic.h:114` |
| `prev_mode != kModeExpand` guard | `starvation_recovery` counter | `decision_logic.cc:278` |
| Fast-mode multi-period removal | `remove_len = multiples * pitch_period` | `accelerate.cc:62-67` |

### Key Divergences from NetEQ

1. **Frame size**: NetEQ typically uses 10-20ms frames at 8-16kHz. We use 10ms frames at 48kHz stereo (music-optimized).
2. **Autocorrelation vs cross-packet**: NetEQ correlates across a 30ms decoded buffer. We correlate within a single 10ms frame for acceleration, and across two frames for WSOLA splicing.
3. **No IAT histogram**: NetEQ builds a full inter-arrival time probability distribution. We use simpler dual-EMA tracking with a peak state machine, which is more responsive to rapid network changes.
4. **Stability-aware decay**: NetEQ uses fixed histogram forgetting factors. We continuously interpolate the peak decay halflife based on measured network stability.
5. **Probe-down mechanism**: NetEQ relies on the IAT histogram naturally tightening. We actively probe downward when the network is stable, with a learned floor to prevent repeated starvation.
