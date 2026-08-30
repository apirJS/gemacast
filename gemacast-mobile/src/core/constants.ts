/**
 * Port constants — must match gemacast_core::network::Ports.
 * If the Rust values ever change, update these to match.
 */
export const Ports = {
  DISCOVERY: 23555,
  AUDIO_UDP: 23556,
  CONTROL: 23559,
} as const;
