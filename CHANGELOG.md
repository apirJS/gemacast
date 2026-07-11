# Changelog

## [0.3.0-rc.2](https://github.com/apirJS/gemacast/compare/v0.2.1-rc.2...v0.3.0-rc.2) (2026-07-11)


### Features

* app update mechanism for all platforms ([d226b30](https://github.com/apirJS/gemacast/commit/d226b30cc08b15512a488568f99bd7eead8e728c))
* app updater with installers, CI hardening ([250f622](https://github.com/apirJS/gemacast/commit/250f622c73ff519f754cb9e9a8a3abc90552105e))
* **audio:** added PLC ([c10eb24](https://github.com/apirJS/gemacast/commit/c10eb246fe00f9e9d99fd88ff3f21d33fc10033b))
* foreground service, USB tether, static jitter buffer, volume controls ([ea8847b](https://github.com/apirJS/gemacast/commit/ea8847b8b4c8b4aa417cd838219c95062c046756))
* graceful shutdown, connection reliability, tracing ([ae81b0a](https://github.com/apirJS/gemacast/commit/ae81b0a889ace12578d7d77c5afa0fdb27ded18b))
* launch app on startup ([#32](https://github.com/apirJS/gemacast/issues/32)) ([b3ce683](https://github.com/apirJS/gemacast/commit/b3ce683715752dd91d15c5a98ec9841a662f49b1))
* mDNS discovery, gain slider, play/pause stream ([bb7f76e](https://github.com/apirJS/gemacast/commit/bb7f76e41653253b7fa86cd641d8b95ab3158da0))
* Oboe migration, adaptive jitter buffer, bitrate options, preset system ([af14292](https://github.com/apirJS/gemacast/commit/af14292dd73350a1fee12674534d2961bd2c633e))
* PipeWire process-level capture for Linux ([e6460f6](https://github.com/apirJS/gemacast/commit/e6460f6489efbab129d4bca6bd3030f227ab3ed4))
* Windows process-level capture, resampler, manual IP input, toast notifications ([157cd8c](https://github.com/apirJS/gemacast/commit/157cd8c35027bbf188bb7afd7df3cd0a9a81c543))


### Bug Fixes

* ci-cd ([#60](https://github.com/apirJS/gemacast/issues/60)) ([79a7393](https://github.com/apirJS/gemacast/commit/79a73938123621b0f9515bfbceee42fa7c582077))
* harden updater, move APK install to Kotlin, phased builder pattern ([4b4f473](https://github.com/apirJS/gemacast/commit/4b4f473895f83c8c186a397a89178a1da18b3369))
* mobile UI polish, compatibility improvements ([ae6d549](https://github.com/apirJS/gemacast/commit/ae6d5495fe3464b098b892b7e175bf535c9d9905))
* PipeWire proxy teardown and thread safety ([4372898](https://github.com/apirJS/gemacast/commit/4372898f149a780ce7ee4978698d1d61e754c84e))
* **updater:** updater fixes ([#30](https://github.com/apirJS/gemacast/issues/30)) ([709bdad](https://github.com/apirJS/gemacast/commit/709bdad67a0af9c100c1f0fd6f1be4e84debc834))


### Performance

* migrate WASAPI to newer API with cpal fallback ([24a3b4b](https://github.com/apirJS/gemacast/commit/24a3b4b1be453a646a92347fd827c3d938556297))


### Refactoring

* gemacast core with hexagon pattern ([#8](https://github.com/apirJS/gemacast/issues/8)) ([c645ab1](https://github.com/apirJS/gemacast/commit/c645ab15e6d40dd277698b2bb9e8a8ae59bf217b))
* rewrite PC and Mobile with adapter pattern, Mobile migrated to ReactJS ([98e5176](https://github.com/apirJS/gemacast/commit/98e51765528030fba692bdc0155e3eca02a17d76))
* split core into Discovery/Control/Stream modules ([7d5d3cf](https://github.com/apirJS/gemacast/commit/7d5d3cf0b276f9f0c1db4e2e0045616d61e30876))

## [0.2.0](https://github.com/apirJS/gemacast/compare/v0.1.0...v0.2.0) (2026-07-10)


### Features

* app update mechanism for all platforms ([d226b30](https://github.com/apirJS/gemacast/commit/d226b30cc08b15512a488568f99bd7eead8e728c))
* app updater with installers, CI hardening ([250f622](https://github.com/apirJS/gemacast/commit/250f622c73ff519f754cb9e9a8a3abc90552105e))
* **audio:** added PLC ([c10eb24](https://github.com/apirJS/gemacast/commit/c10eb246fe00f9e9d99fd88ff3f21d33fc10033b))
* foreground service, USB tether, static jitter buffer, volume controls ([ea8847b](https://github.com/apirJS/gemacast/commit/ea8847b8b4c8b4aa417cd838219c95062c046756))
* graceful shutdown, connection reliability, tracing ([ae81b0a](https://github.com/apirJS/gemacast/commit/ae81b0a889ace12578d7d77c5afa0fdb27ded18b))
* launch app on startup ([#32](https://github.com/apirJS/gemacast/issues/32)) ([b3ce683](https://github.com/apirJS/gemacast/commit/b3ce683715752dd91d15c5a98ec9841a662f49b1))
* mDNS discovery, gain slider, play/pause stream ([bb7f76e](https://github.com/apirJS/gemacast/commit/bb7f76e41653253b7fa86cd641d8b95ab3158da0))
* Oboe migration, adaptive jitter buffer, bitrate options, preset system ([af14292](https://github.com/apirJS/gemacast/commit/af14292dd73350a1fee12674534d2961bd2c633e))
* PipeWire process-level capture for Linux ([e6460f6](https://github.com/apirJS/gemacast/commit/e6460f6489efbab129d4bca6bd3030f227ab3ed4))
* Windows process-level capture, resampler, manual IP input, toast notifications ([157cd8c](https://github.com/apirJS/gemacast/commit/157cd8c35027bbf188bb7afd7df3cd0a9a81c543))


### Bug Fixes

* harden updater, move APK install to Kotlin, phased builder pattern ([4b4f473](https://github.com/apirJS/gemacast/commit/4b4f473895f83c8c186a397a89178a1da18b3369))
* mobile UI polish, compatibility improvements ([ae6d549](https://github.com/apirJS/gemacast/commit/ae6d5495fe3464b098b892b7e175bf535c9d9905))
* PipeWire proxy teardown and thread safety ([4372898](https://github.com/apirJS/gemacast/commit/4372898f149a780ce7ee4978698d1d61e754c84e))
* **updater:** updater fixes ([#30](https://github.com/apirJS/gemacast/issues/30)) ([709bdad](https://github.com/apirJS/gemacast/commit/709bdad67a0af9c100c1f0fd6f1be4e84debc834))


### Performance

* migrate WASAPI to newer API with cpal fallback ([24a3b4b](https://github.com/apirJS/gemacast/commit/24a3b4b1be453a646a92347fd827c3d938556297))


### Refactoring

* gemacast core with hexagon pattern ([#8](https://github.com/apirJS/gemacast/issues/8)) ([c645ab1](https://github.com/apirJS/gemacast/commit/c645ab15e6d40dd277698b2bb9e8a8ae59bf217b))
* rewrite PC and Mobile with adapter pattern, Mobile migrated to ReactJS ([98e5176](https://github.com/apirJS/gemacast/commit/98e51765528030fba692bdc0155e3eca02a17d76))
* split core into Discovery/Control/Stream modules ([7d5d3cf](https://github.com/apirJS/gemacast/commit/7d5d3cf0b276f9f0c1db4e2e0045616d61e30876))
