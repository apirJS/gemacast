/**
 * Port constants — must match gemacast_core::network::Ports.
 * If the Rust values ever change, update these to match.
 */
export const Ports = {
  DISCOVERY: 55555,
  AUDIO_UDP: 55556,
  CONTROL: 55559,
} as const;
