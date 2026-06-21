# Changelog

## [0.2.0](https://github.com/apirJS/gemacast/compare/gemacast-v0.1.0...gemacast-v0.2.0) (2026-06-21)


### Features

* Added Jitter Handling ([471fab1](https://github.com/apirJS/gemacast/commit/471fab10a4276766ad23cc4fd75073281a165620))
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
* **lifecycle:** implement graceful shutdown for PC and Mobile Replaces abrupt process terminations with graceful teardown flows across both applications, ensuring audio streams, ADB forwarders, and network sockets are cleanly closed before exiting. ([eb60b8e](https://github.com/apirJS/gemacast/commit/eb60b8e04682081d9f9d39e5f9ec0d5628644237))
* massive refactor ([2392004](https://github.com/apirJS/gemacast/commit/2392004d9b320be6ad51d11eb04f0f05821a2096))
* Migrate from cpal to Oboe (Low latency mode) for Android ([3ad7644](https://github.com/apirJS/gemacast/commit/3ad764493778c0ec11311b9efc5d7e8a24e36819))
* More tolerant Jitter Manager ([8902522](https://github.com/apirJS/gemacast/commit/890252241315c12fc52a19239aa2127ada1ec51e))
* Proces-Level Loopback Capture on Windows ([21b6430](https://github.com/apirJS/gemacast/commit/21b643020b98e89b00caaab7c842dbe3e520f82c))
* Resampler for PC-side capture with rubato ([cdc9e7c](https://github.com/apirJS/gemacast/commit/cdc9e7c85677e06999ac7aebbec9e7e25d301f70))
* shift to static jitter buffer, robust volume controls, and presence updates ([399066d](https://github.com/apirJS/gemacast/commit/399066dc9f21b9152bdf101c0b7907470ecdf8e6))


### Bug Fixes

* **core:** improve connection reliability and thread teardown ([e6c73c4](https://github.com/apirJS/gemacast/commit/e6c73c41ed3882ef667ce7f8c2ec6218d93d256b))
* **network:** Fix USB vs WIFI naming checks. Stopping sending presence on 'Stop Broadcast' ([d3c94f9](https://github.com/apirJS/gemacast/commit/d3c94f9e95ce7bde1c89f2e6543f27283bbf9018))
* **network:** resolve discovery, graceful disconnects, and mobile timeout logic ([70486b3](https://github.com/apirJS/gemacast/commit/70486b344cd74599f01838605be5d8125475ff54))
* prevent multiple PC instances and mobile connection state bugs ([#3](https://github.com/apirJS/gemacast/issues/3)) ([1be594f](https://github.com/apirJS/gemacast/commit/1be594f769725e7a18c28b590550173d84de9ead))
* test release-plz trigger ([094a03e](https://github.com/apirJS/gemacast/commit/094a03e697603725f5b9a7a6882ef2b33b589a6b))


### Performance

* Reducing reallocation on manager.rs ([cdc9e7c](https://github.com/apirJS/gemacast/commit/cdc9e7c85677e06999ac7aebbec9e7e25d301f70))


### Refactoring

* Changing the discovery mechanism from phone-pc to pc-phone ([b119a24](https://github.com/apirJS/gemacast/commit/b119a2471fcd49cff7185056ff70a6e96fe9b4b5))
* **gemacast-core:** Separating concerns of core into Discovery, Control, and Stream ([8b762b3](https://github.com/apirJS/gemacast/commit/8b762b3f112c30be5ef3c14c36f18107bdd54603))
* **gemacast-core:** Split sender.rs and receiver.rs into several files as modules ([6e623d1](https://github.com/apirJS/gemacast/commit/6e623d144d9dbc7ee5609400744e3f76b8efa615))
* **gemacast-mobile:** Rewritten with adapter pattern ([336ff4f](https://github.com/apirJS/gemacast/commit/336ff4f17b7765c806aa84f756a18f3c605cd9b8))
* **gemacast-mobile:** Rewritten with ReactJS ([3304141](https://github.com/apirJS/gemacast/commit/330414139cb12b5a1b5bb4af6401a980302e3406))
* **gemacast-mobile:** Separate commands into several files ([5e61d12](https://github.com/apirJS/gemacast/commit/5e61d123e122c347ad53bb5c4ac644048f436160))
* **gemacast-pc:** Rewritten with adapter pattern ([1f9ce12](https://github.com/apirJS/gemacast/commit/1f9ce125445ea373d609fd2eec3e98ee4ccd419c))
* **mobile:** Separate css to serveral files, making dom handling and state handling more modular ([d6cf381](https://github.com/apirJS/gemacast/commit/d6cf3819fe0fde014b58ae241bb4d604e6b9293f))
