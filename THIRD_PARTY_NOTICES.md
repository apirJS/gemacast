# Third-Party Notices

Gemacast uses open-source software. The tables below summarize major direct
dependencies; each dependency's distributed license text remains authoritative.
Lockfiles contain the complete dependency graph and should be used when
generating release compliance reports.

## Rust

| Crate | License | Purpose |
| --- | --- | --- |
| [tokio](https://crates.io/crates/tokio) | MIT | Async runtime |
| [axum](https://crates.io/crates/axum) | MIT | HTTP and WebSocket control server |
| [tao](https://crates.io/crates/tao) | Apache-2.0 / MIT | Desktop event loop |
| [tray-icon](https://crates.io/crates/tray-icon) | Apache-2.0 / MIT | System tray integration |
| [tauri](https://crates.io/crates/tauri) | Apache-2.0 / MIT | Mobile application framework |
| [opus](https://crates.io/crates/opus) and [audiopus_sys](https://crates.io/crates/audiopus_sys) | MIT / BSD-3-Clause / ISC | Opus codec bindings |
| [oboe](https://crates.io/crates/oboe) | Apache-2.0 | Android low-latency audio output |
| [cpal](https://crates.io/crates/cpal) | Apache-2.0 | Cross-platform audio I/O |
| [pipewire](https://crates.io/crates/pipewire) | MIT | Linux audio capture |
| [screencapturekit](https://crates.io/crates/screencapturekit) | Apache-2.0 / MIT | macOS audio capture |
| [rubato](https://crates.io/crates/rubato) | MIT / Apache-2.0 | Sample-rate conversion |
| [ringbuf](https://crates.io/crates/ringbuf) | MIT / Apache-2.0 | Lock-free audio buffers |
| [mdns-sd](https://crates.io/crates/mdns-sd) | Apache-2.0 / MIT | mDNS discovery |
| [reqwest](https://crates.io/crates/reqwest) | MIT / Apache-2.0 | HTTP client |
| [rustls](https://crates.io/crates/rustls) | Apache-2.0 / MIT / ISC | TLS |
| [ring](https://crates.io/crates/ring) | Apache-2.0 / ISC / OpenSSL | Cryptographic primitives |
| [rcgen](https://crates.io/crates/rcgen) | MIT / Apache-2.0 | X.509 certificate generation |
| [serde](https://crates.io/crates/serde) and [serde_json](https://crates.io/crates/serde_json) | MIT / Apache-2.0 | Serialization |
| [rfd](https://crates.io/crates/rfd) | MIT | Native dialogs |
| [image](https://crates.io/crates/image) | MIT / Apache-2.0 | Image processing |
| [semver](https://crates.io/crates/semver) | MIT / Apache-2.0 | Version comparison |

## Mobile frontend

| Package | License | Purpose |
| --- | --- | --- |
| [react](https://www.npmjs.com/package/react) and [react-dom](https://www.npmjs.com/package/react-dom) | MIT | UI framework |
| [zustand](https://www.npmjs.com/package/zustand) | MIT | State management |
| [tailwindcss](https://www.npmjs.com/package/tailwindcss) | MIT | Styling |
| [lucide-react](https://www.npmjs.com/package/lucide-react) | ISC | Icons |
| [vite](https://www.npmjs.com/package/vite) | MIT | Build tooling |
| [@tauri-apps/api](https://www.npmjs.com/package/@tauri-apps/api) | Apache-2.0 / MIT | Tauri frontend API |

## Website

| Package | License | Purpose |
| --- | --- | --- |
| [vue](https://www.npmjs.com/package/vue) | MIT | UI framework |
| [pinia](https://www.npmjs.com/package/pinia) | MIT | State management |
| [tailwindcss](https://www.npmjs.com/package/tailwindcss) | MIT | Styling |
| [@lucide/vue](https://www.npmjs.com/package/@lucide/vue) | ISC | Icons |
| [vite](https://www.npmjs.com/package/vite) | MIT | Build tooling |

Gemacast itself is distributed under the
[GNU General Public License version 3 or later](LICENSE).
