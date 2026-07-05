# Changelog

## [0.5.4](https://github.com/apirJS/gemacast/compare/v0.5.3...v0.5.4) (2026-07-05)


### Bug Fixes

* move install_apk_android logic to kotlin instead of JNI ([c9df954](https://github.com/apirJS/gemacast/commit/c9df9542426d6d8b2d32ff7e0e2ff9166bab94e3))

## [0.5.3](https://github.com/apirJS/gemacast/compare/v0.5.2...v0.5.3) (2026-07-05)


### Bug Fixes

* force release 0.5.3 to test updaters ([76be18c](https://github.com/apirJS/gemacast/commit/76be18c7f851aee797beca9ea3a019644903f3d2))

## [0.5.2](https://github.com/apirJS/gemacast/compare/v0.5.1...v0.5.2) (2026-07-05)


### Bug Fixes

* updater issues ([#45](https://github.com/apirJS/gemacast/issues/45)) ([4a8430f](https://github.com/apirJS/gemacast/commit/4a8430fd47334d11388ee470894faa8f0d770a08))

## [0.5.1](https://github.com/apirJS/gemacast/compare/v0.5.0...v0.5.1) (2026-07-04)


### Bug Fixes

* force trigger 0.5.1 to test updaters ([90bf23c](https://github.com/apirJS/gemacast/commit/90bf23cbf84ae2265b42d7b4d0eabaa7fc252748))
* force trigger 0.5.1 to test updaters ([ee29eb8](https://github.com/apirJS/gemacast/commit/ee29eb85da65b85b3cceec62660b5e3a035ba017))

## [0.5.0](https://github.com/apirJS/gemacast/compare/v0.4.2...v0.5.0) (2026-07-04)


### Features

* pipewire process-level audio capture and desktop audio capture support ([8e98444](https://github.com/apirJS/gemacast/commit/8e984446480756712740fa0d71a6d418f9688a38))
* pipewire process-level capture support for Linux ([#38](https://github.com/apirJS/gemacast/issues/38)) ([9cf620b](https://github.com/apirJS/gemacast/commit/9cf620b293415364c5f6cb3f0ae85ac2d7965198))


### Bug Fixes

* missing build deps on CI ([#40](https://github.com/apirJS/gemacast/issues/40)) ([0c4cb64](https://github.com/apirJS/gemacast/commit/0c4cb64c1927628d3545c9ae2c2ecbdee16089c7))

## [0.6.0](https://github.com/apirJS/gemacast/compare/v0.5.0...v0.6.0) (2026-07-04)


### Features

* Added Jitter Handling ([471fab1](https://github.com/apirJS/gemacast/commit/471fab10a4276766ad23cc4fd75073281a165620))
* app update feature ([#15](https://github.com/apirJS/gemacast/issues/15)) ([a178952](https://github.com/apirJS/gemacast/commit/a1789526939dc7ef51ce7e010541132d60824e58))
* app updater, installers test, fixing broken CI files, fixing broken MSI installer ([#23](https://github.com/apirJS/gemacast/issues/23)) ([3ec1011](https://github.com/apirJS/gemacast/commit/3ec101108a78cf5bd91b15a591e993afe6d6452b))
* **audio:** added PLC ([c2c07fe](https://github.com/apirJS/gemacast/commit/c2c07fe782a1cb6b348c83b0475f4810d64848f0))
* Bitrate option for user and Adaptive Jitter Buffer ([b1bd360](https://github.com/apirJS/gemacast/commit/b1bd360371f25b632bb9ee8decee3220cc2a5771))
* Each Receiver can have their own bitrate quality ([60390ac](https://github.com/apirJS/gemacast/commit/60390ac7f884f20efc20f243724c1a814066e3c1))
* Enable users to manually input the PC address ([58e305d](https://github.com/apirJS/gemacast/commit/58e305dcdfe5c7b5a061c3a086e822851e3db8f6))
* foreground service, usb tether support, media session control, dynamic buffer on the sender side ([5e61d12](https://github.com/apirJS/gemacast/commit/5e61d123e122c347ad53bb5c4ac644048f436160))
* **gemacast-core:** mDNS for discovery ([ace4851](https://github.com/apirJS/gemacast/commit/ace4851274a6460ce454a1b754bb12c4a213800e))
* **gemacast-mobile:** Gain Slider and 'No Buffer' buffer preset ([1dcc201](https://github.com/apirJS/gemacast/commit/1dcc2012bde2e8fab18b689fd5f22ce149a09fa9))
* **gemacast-mobile:** Play/Pause stream functionality ([8921c97](https://github.com/apirJS/gemacast/commit/8921c9748919e435df5c5899d45e300f65ba3862))
* **gemacast-mobile:** Toast notification ([a39331f](https://github.com/apirJS/gemacast/commit/a39331f580080c7b92a78aef9550fa5a33cdbdf3))
* **gemacast-pc:** run the mDNS feature ([b6e4767](https://github.com/apirJS/gemacast/commit/b6e47675f65a220fea86e46466f677b22cefaa60))
* Introducing preset options + custom preset for the Jitter Management Config, added settings panel drawer, improved reconnection mechanism, improved Jitter Management algorithm, improved discovery mechanism ([2d5234e](https://github.com/apirJS/gemacast/commit/2d5234e85192dc0e4214c24a122d6c60b3ea2d35))
* launch app on startup ([#32](https://github.com/apirJS/gemacast/issues/32)) ([7286095](https://github.com/apirJS/gemacast/commit/72860955142ee089607489b13e3946fffc7552f2))
* **lifecycle:** implement graceful shutdown for PC and Mobile Replaces abrupt process terminations with graceful teardown flows across both applications, ensuring audio streams, ADB forwarders, and network sockets are cleanly closed before exiting. ([eb60b8e](https://github.com/apirJS/gemacast/commit/eb60b8e04682081d9f9d39e5f9ec0d5628644237))
* massive refactor ([2392004](https://github.com/apirJS/gemacast/commit/2392004d9b320be6ad51d11eb04f0f05821a2096))
* Migrate from cpal to Oboe (Low latency mode) for Android ([3ad7644](https://github.com/apirJS/gemacast/commit/3ad764493778c0ec11311b9efc5d7e8a24e36819))
* More tolerant Jitter Manager ([8902522](https://github.com/apirJS/gemacast/commit/890252241315c12fc52a19239aa2127ada1ec51e))
* pipewire process-level capture support for Linux ([#38](https://github.com/apirJS/gemacast/issues/38)) ([9cf620b](https://github.com/apirJS/gemacast/commit/9cf620b293415364c5f6cb3f0ae85ac2d7965198))
* Proces-Level Loopback Capture on Windows ([21b6430](https://github.com/apirJS/gemacast/commit/21b643020b98e89b00caaab7c842dbe3e520f82c))
* Resampler for PC-side capture with rubato ([cdc9e7c](https://github.com/apirJS/gemacast/commit/cdc9e7c85677e06999ac7aebbec9e7e25d301f70))
* shift to static jitter buffer, robust volume controls, and presence updates ([399066d](https://github.com/apirJS/gemacast/commit/399066dc9f21b9152bdf101c0b7907470ecdf8e6))
* update mechanism ([#14](https://github.com/apirJS/gemacast/issues/14)) ([a82fcdf](https://github.com/apirJS/gemacast/commit/a82fcdf94a7bdec8de59af608e8839fe7598fe11))


### Bug Fixes

* cargo syntax issue ([3877c41](https://github.com/apirJS/gemacast/commit/3877c41bef71ca337de81701c5fccdcad95169de))
* ci build and clippy errors for updater ([89e00c1](https://github.com/apirJS/gemacast/commit/89e00c183774a9f661056f0523748fde94a3bf87))
* **ci:** fix RPM build by locating output in arch subdirectory ([27c9c19](https://github.com/apirJS/gemacast/commit/27c9c198871068a9201598e532f0403f05dafbbf))
* **core:** improve connection reliability and thread teardown ([e6c73c4](https://github.com/apirJS/gemacast/commit/e6c73c41ed3882ef667ce7f8c2ec6218d93d256b))
* deprecated syntax at install.rs, and change on test-intallers.yaml run condition ([2024944](https://github.com/apirJS/gemacast/commit/202494427f98194fc6af6d706ffa8a02e0af389d))
* force trigger 0.1.2 release to test cargo-dist fix ([219f68f](https://github.com/apirJS/gemacast/commit/219f68fe602a0790cb798c48a50b898d65d39c9c))
* **gemacast-core:** missing feature dep for reqwest, and rust formatter issue ([a3f807a](https://github.com/apirJS/gemacast/commit/a3f807a0ce0a40b6c5acdc9719aca2541ad4f89c))
* missing build deps on CI ([#40](https://github.com/apirJS/gemacast/issues/40)) ([0c4cb64](https://github.com/apirJS/gemacast/commit/0c4cb64c1927628d3545c9ae2c2ecbdee16089c7))
* missing trait on install.rs ([67422f4](https://github.com/apirJS/gemacast/commit/67422f4342e9d87bbb7b288bc104127838652c76))
* mixed codes after rebase, and cargo format issue ([a9a2ee0](https://github.com/apirJS/gemacast/commit/a9a2ee012919ff44abe249a51e6699350073bf82))
* **network:** Fix USB vs WIFI naming checks. Stopping sending presence on 'Stop Broadcast' ([d3c94f9](https://github.com/apirJS/gemacast/commit/d3c94f9e95ce7bde1c89f2e6543f27283bbf9018))
* **network:** resolve discovery, graceful disconnects, and mobile timeout logic ([70486b3](https://github.com/apirJS/gemacast/commit/70486b344cd74599f01838605be5d8125475ff54))
* **pc:** make ADB path resolution robust for Linux packages and fix Fedora RPM CI checks [skip ci] ([09e82db](https://github.com/apirJS/gemacast/commit/09e82db05903929e831cfef0037025141fff3aab))
* prevent multiple PC instances and mobile connection state bugs ([#3](https://github.com/apirJS/gemacast/issues/3)) ([1be594f](https://github.com/apirJS/gemacast/commit/1be594f769725e7a18c28b590550173d84de9ead))
* syntax issue at mobile update installer ([393a2ed](https://github.com/apirJS/gemacast/commit/393a2edb7020d9dc6320f8c2ce441628197db686))
* test release-plz trigger ([094a03e](https://github.com/apirJS/gemacast/commit/094a03e697603725f5b9a7a6882ef2b33b589a6b))
* update tauri android jni implementation for v2 ([e281c17](https://github.com/apirJS/gemacast/commit/e281c17c1e8910826c821a54724a0321b926cda2))
* updater issues ([#34](https://github.com/apirJS/gemacast/issues/34)) ([9b37b96](https://github.com/apirJS/gemacast/commit/9b37b960f6db3b6d715ffec2388e929658389cd3))
* updater issues ([#36](https://github.com/apirJS/gemacast/issues/36)) ([f1d8b81](https://github.com/apirJS/gemacast/commit/f1d8b81e8f55602528bead35e932e3b3de82cba6))
* **updater:** updater fixes ([#30](https://github.com/apirJS/gemacast/issues/30)) ([385a6ec](https://github.com/apirJS/gemacast/commit/385a6ec3ffbda1d86c3801e54733f5955c692075))
* wrong type assumption fix ([858c730](https://github.com/apirJS/gemacast/commit/858c73018ae157181686d94e1928b8dab4ff61b7))


### Performance

* migrate wasapi to newer api and fallback to cpal ([b4242a2](https://github.com/apirJS/gemacast/commit/b4242a242d8c953534f8ecccfcef5e02c5ffe15f))
* Reducing reallocation on manager.rs ([cdc9e7c](https://github.com/apirJS/gemacast/commit/cdc9e7c85677e06999ac7aebbec9e7e25d301f70))


### Refactoring

* Changing the discovery mechanism from phone-pc to pc-phone ([b119a24](https://github.com/apirJS/gemacast/commit/b119a2471fcd49cff7185056ff70a6e96fe9b4b5))
* gemacast core with hexagon pattern ([#8](https://github.com/apirJS/gemacast/issues/8)) ([fadfb82](https://github.com/apirJS/gemacast/commit/fadfb82a3f8fc9c50ce530e05fc6bfaf2f5e571e))
* **gemacast-core:** Separating concerns of core into Discovery, Control, and Stream ([8b762b3](https://github.com/apirJS/gemacast/commit/8b762b3f112c30be5ef3c14c36f18107bdd54603))
* **gemacast-core:** Split sender.rs and receiver.rs into several files as modules ([6e623d1](https://github.com/apirJS/gemacast/commit/6e623d144d9dbc7ee5609400744e3f76b8efa615))
* **gemacast-mobile:** Rewritten with adapter pattern ([336ff4f](https://github.com/apirJS/gemacast/commit/336ff4f17b7765c806aa84f756a18f3c605cd9b8))
* **gemacast-mobile:** Rewritten with ReactJS ([3304141](https://github.com/apirJS/gemacast/commit/330414139cb12b5a1b5bb4af6401a980302e3406))
* **gemacast-mobile:** Separate commands into several files ([5e61d12](https://github.com/apirJS/gemacast/commit/5e61d123e122c347ad53bb5c4ac644048f436160))
* **gemacast-pc:** Rewritten with adapter pattern ([1f9ce12](https://github.com/apirJS/gemacast/commit/1f9ce125445ea373d609fd2eec3e98ee4ccd419c))
* **mobile:** Separate css to serveral files, making dom handling and state handling more modular ([d6cf381](https://github.com/apirJS/gemacast/commit/d6cf3819fe0fde014b58ae241bb4d604e6b9293f))

## [0.5.0](https://github.com/apirJS/gemacast/compare/v0.4.2...v0.5.0) (2026-07-04)


### Features

* pipewire process-level capture support for Linux ([#38](https://github.com/apirJS/gemacast/issues/38)) ([9cf620b](https://github.com/apirJS/gemacast/commit/9cf620b293415364c5f6cb3f0ae85ac2d7965198))

## [0.4.2](https://github.com/apirJS/gemacast/compare/v0.4.1...v0.4.2) (2026-07-01)


### Bug Fixes

* updater issues ([#36](https://github.com/apirJS/gemacast/issues/36)) ([f1d8b81](https://github.com/apirJS/gemacast/commit/f1d8b81e8f55602528bead35e932e3b3de82cba6))

## [0.4.1](https://github.com/apirJS/gemacast/compare/v0.4.0...v0.4.1) (2026-06-30)


### Bug Fixes

* updater issues ([#34](https://github.com/apirJS/gemacast/issues/34)) ([9b37b96](https://github.com/apirJS/gemacast/commit/9b37b960f6db3b6d715ffec2388e929658389cd3))

## [0.4.0](https://github.com/apirJS/gemacast/compare/v0.3.4...v0.4.0) (2026-06-30)


### Features

* launch app on startup ([#32](https://github.com/apirJS/gemacast/issues/32)) ([7286095](https://github.com/apirJS/gemacast/commit/72860955142ee089607489b13e3946fffc7552f2))

## [0.3.4](https://github.com/apirJS/gemacast/compare/v0.3.3...v0.3.4) (2026-06-29)


### Bug Fixes

* **updater:** updater fixes ([#30](https://github.com/apirJS/gemacast/issues/30)) ([385a6ec](https://github.com/apirJS/gemacast/commit/385a6ec3ffbda1d86c3801e54733f5955c692075))

## [0.3.3](https://github.com/apirJS/gemacast/compare/v0.3.2...v0.3.3) (2026-06-29)


### Bug Fixes

* **ci:** fix RPM build by locating output in arch subdirectory ([27c9c19](https://github.com/apirJS/gemacast/commit/27c9c198871068a9201598e532f0403f05dafbbf))
* **gemacast-core:** missing feature dep for reqwest, and rust formatter issue ([a3f807a](https://github.com/apirJS/gemacast/commit/a3f807a0ce0a40b6c5acdc9719aca2541ad4f89c))
* **pc:** make ADB path resolution robust for Linux packages and fix Fedora RPM CI checks [skip ci] ([09e82db](https://github.com/apirJS/gemacast/commit/09e82db05903929e831cfef0037025141fff3aab))

## [0.3.2](https://github.com/apirJS/gemacast/compare/v0.3.1...v0.3.2) (2026-06-28)


### Bug Fixes

* cargo syntax issue ([3877c41](https://github.com/apirJS/gemacast/commit/3877c41bef71ca337de81701c5fccdcad95169de))

## [0.3.1](https://github.com/apirJS/gemacast/compare/v0.3.0...v0.3.1) (2026-06-28)


### Bug Fixes

* deprecated syntax at install.rs, and change on test-intallers.yaml run condition ([2024944](https://github.com/apirJS/gemacast/commit/202494427f98194fc6af6d706ffa8a02e0af389d))

## [0.3.0](https://github.com/apirJS/gemacast/compare/v0.2.5...v0.3.0) (2026-06-28)


### Features

* app updater, installers test, fixing broken CI files, fixing broken MSI installer ([#23](https://github.com/apirJS/gemacast/issues/23)) ([3ec1011](https://github.com/apirJS/gemacast/commit/3ec101108a78cf5bd91b15a591e993afe6d6452b))


### Bug Fixes

* mixed codes after rebase, and cargo format issue ([a9a2ee0](https://github.com/apirJS/gemacast/commit/a9a2ee012919ff44abe249a51e6699350073bf82))

## [0.2.5](https://github.com/apirJS/gemacast/compare/v0.2.4...v0.2.5) (2026-06-27)


### Bug Fixes

* wrong type assumption fix ([858c730](https://github.com/apirJS/gemacast/commit/858c73018ae157181686d94e1928b8dab4ff61b7))

## [0.2.4](https://github.com/apirJS/gemacast/compare/v0.2.3...v0.2.4) (2026-06-27)


### Bug Fixes

* missing trait on install.rs ([67422f4](https://github.com/apirJS/gemacast/commit/67422f4342e9d87bbb7b288bc104127838652c76))

## [0.2.3](https://github.com/apirJS/gemacast/compare/v0.2.2...v0.2.3) (2026-06-27)


### Bug Fixes

* syntax issue at mobile update installer ([393a2ed](https://github.com/apirJS/gemacast/commit/393a2edb7020d9dc6320f8c2ce441628197db686))

## [0.2.2](https://github.com/apirJS/gemacast/compare/v0.2.1...v0.2.2) (2026-06-27)


### Bug Fixes

* update tauri android jni implementation for v2 ([e281c17](https://github.com/apirJS/gemacast/commit/e281c17c1e8910826c821a54724a0321b926cda2))

## [0.2.1](https://github.com/apirJS/gemacast/compare/v0.2.0...v0.2.1) (2026-06-27)


### Bug Fixes

* ci build and clippy errors for updater ([89e00c1](https://github.com/apirJS/gemacast/commit/89e00c183774a9f661056f0523748fde94a3bf87))

## [0.2.0](https://github.com/apirJS/gemacast/compare/v0.1.4...v0.2.0) (2026-06-27)


### Features

* app update feature ([#15](https://github.com/apirJS/gemacast/issues/15)) ([a178952](https://github.com/apirJS/gemacast/commit/a1789526939dc7ef51ce7e010541132d60824e58))
* update mechanism ([#14](https://github.com/apirJS/gemacast/issues/14)) ([a82fcdf](https://github.com/apirJS/gemacast/commit/a82fcdf94a7bdec8de59af608e8839fe7598fe11))

## [0.1.4](https://github.com/apirJS/gemacast/compare/v0.1.3...v0.1.4) (2026-06-25)


### Performance

* migrate wasapi to newer api and fallback to cpal ([b4242a2](https://github.com/apirJS/gemacast/commit/b4242a242d8c953534f8ecccfcef5e02c5ffe15f))

## [0.1.3](https://github.com/apirJS/gemacast/compare/v0.1.2...v0.1.3) (2026-06-25)


### Refactoring

* gemacast core with hexagon pattern ([#8](https://github.com/apirJS/gemacast/issues/8)) ([fadfb82](https://github.com/apirJS/gemacast/commit/fadfb82a3f8fc9c50ce530e05fc6bfaf2f5e571e))

## [0.1.2](https://github.com/apirJS/gemacast/compare/v0.1.1...v0.1.2) (2026-06-21)


### Bug Fixes

* force trigger 0.1.2 release to test cargo-dist fix ([219f68f](https://github.com/apirJS/gemacast/commit/219f68fe602a0790cb798c48a50b898d65d39c9c))

## [0.1.1](https://github.com/apirJS/gemacast/compare/v0.1.0...v0.1.1) (2026-06-21)


### Bug Fixes

* test release-plz trigger ([094a03e](https://github.com/apirJS/gemacast/commit/094a03e697603725f5b9a7a6882ef2b33b589a6b))
