# Changelog

## [0.6.0](https://github.com/jeanhadrien/tron-zero-rust/compare/client-v0.5.0...client-v0.6.0) (2026-11-05)


### Miscellaneous Chores

* **client:** Synchronize tron-zero versions

## [0.5.0](https://github.com/jeanhadrien/tron-zero-rust/compare/client-v0.4.0...client-v0.5.0) (2026-11-05)


### Features

* **client:** gizmo particle effects (derez, rezz, grind sparks) ([846f28c](https://github.com/jeanhadrien/tron-zero-rust/commit/846f28c58949a0286a4d991bd3d983cc18436964))
* **client:** ranked join with account, ticket and leaderboard ([14a3458](https://github.com/jeanhadrien/tron-zero-rust/commit/14a3458242f5889473b6a7c3f8c568d050d328c2))
* **client:** render smoothing separate from sim state ([de34ec1](https://github.com/jeanhadrien/tron-zero-rust/commit/de34ec19968d8a33b6e0bca06dfa402844981b1f))
* **client:** show Localhost only when a local server answers a probe ([30a5d41](https://github.com/jeanhadrien/tron-zero-rust/commit/30a5d412ebda7718c0ac2cfce30431b1299cc5f2))
* **client:** Theme design tokens drive egui style and world rendering ([2c5134a](https://github.com/jeanhadrien/tron-zero-rust/commit/2c5134a89e3d39fe21898b8017ebf392bca93d14))
* **manager,client:** placement via /api/play and a Play ranked button ([150bcfb](https://github.com/jeanhadrien/tron-zero-rust/commit/150bcfbd5d0b8550838d0bbbe79dc4885ced3537))


### Bug Fixes

* **client:** draw grid as sprites behind trails; centralise 2D layers ([f3f4089](https://github.com/jeanhadrien/tron-zero-rust/commit/f3f40891a4a6ff158b2124f4103aefa84dbac5a0))
* **client:** review follow-ups for theme and effects ([f2612e1](https://github.com/jeanhadrien/tron-zero-rust/commit/f2612e177e6c69dd107c422bf05e48044e3cde2c))
* **client:** sparks follow the replicated IsSliding grind flag ([dbe106b](https://github.com/jeanhadrien/tron-zero-rust/commit/dbe106ba48bc7824373e86c9f7920594143d3a52))

## [0.4.0](https://github.com/jeanhadrien/tron-zero-rust/compare/client-v0.3.0...client-v0.4.0) (2026-11-02)


### Features

* **bot:** opponent memory, brain updates and BotDebug overlay (WIP) ([70f6f97](https://github.com/jeanhadrien/tron-zero-rust/commit/70f6f973aa2cf277ab645f35b9dcbe9302c4c739))
* **client:** browse and join WebTransport rooms from the browser ([b1cb419](https://github.com/jeanhadrien/tron-zero-rust/commit/b1cb419bc98bb894ab54c9429ce3a956f6385da6))
* **client:** compile and start in the browser (wasm32, WebGL2, WebTransport client) ([de893ad](https://github.com/jeanhadrien/tron-zero-rust/commit/de893ad569c83054044dd05aef2d7bddaccf01b8))
* **labels:** local-only rider name labels via replicated DisplayName ([7380a79](https://github.com/jeanhadrien/tron-zero-rust/commit/7380a794dece5019f0faa893870b903a5bc0ae4f))
* **netcode:** redundant turn delivery, coalesced acks, per-link budgets, no lost presses on predicted death ([aa4d6bf](https://github.com/jeanhadrien/tron-zero-rust/commit/aa4d6bf8aeb710a0fbb7340223b861e350f5f1ef))
* **net:** execute turns at their intended tick, bounded late window ([ffe5792](https://github.com/jeanhadrien/tron-zero-rust/commit/ffe57922aef3cea27d8863d4d601ba03f3b0a480))
* **net:** sequenced one-shot turn commands with server acks ([d428553](https://github.com/jeanhadrien/tron-zero-rust/commit/d428553b829bfea355cd5163d124ac87f611990c))
* **server:** accept WebTransport sessions beside UDP on one server entity ([8f43c04](https://github.com/jeanhadrien/tron-zero-rust/commit/8f43c0465c9e03501b634d6377b8cb823360b6e2))
* **session:** hello handshake with player name, protocol version and capacity ([68fdc14](https://github.com/jeanhadrien/tron-zero-rust/commit/68fdc14f73be59e2a4a7b540c9cc8359dec04806))


### Bug Fixes

* **announce:** token auth, humans-only playerCount, loopback guard, SIGTERM unregister ([9bcef9e](https://github.com/jeanhadrien/tron-zero-rust/commit/9bcef9e8173c6ba58cac8a33f578e338d1b9877f))
* **client,shared:** autoconnect/quit races, name sanitizing, protocol fingerprint, prefs and sim fixes ([9daed42](https://github.com/jeanhadrien/tron-zero-rust/commit/9daed42d918f8168381c7593dec735abab165b66))
* **client:** keyboard test stamps no timeline tick; correct redundancy bandwidth in docs ([5e9f6cd](https://github.com/jeanhadrien/tron-zero-rust/commit/5e9f6cd74061ab1de017b2df2f5797e04f239334))
* **client:** turn outbox follows Lightyear timeline resyncs ([9c1cba2](https://github.com/jeanhadrien/tron-zero-rust/commit/9c1cba2e09793cb05fe8bff3d466fa9c42e0da62))
* **server,client:** advertise web only when open, gate query overrides, doc fixes ([4606842](https://github.com/jeanhadrien/tron-zero-rust/commit/4606842c0af29b95dd1303cf6261aa0bc1e8ec4a))


### Performance Improvements

* **client:** trim bevy and bevy_egui features to 2D rendering ([784da67](https://github.com/jeanhadrien/tron-zero-rust/commit/784da67a3d1353288576719420d007eed2be27a2))

## [0.3.0](https://github.com/jeanhadrien/tron-zero-rust/compare/client-v0.2.0...client-v0.3.0) (2026-10-02)


### Features

* add release-please and deploy workflows ([15d11b9](https://github.com/jeanhadrien/tron-zero-rust/commit/15d11b9284ad0950279779a4afcfa241cb734b1b))
* egui menu, settings and HUD with shared theme ([5dddb10](https://github.com/jeanhadrien/tron-zero-rust/commit/5dddb107a8b4ebfa7066ba0eee44a5ddacdafffa))
* layout-honest key labels, physical matching unchanged ([c28acfd](https://github.com/jeanhadrien/tron-zero-rust/commit/c28acfdb5eb49f873d26ac6166e63da65328bf36))
* menu ([a4ed88c](https://github.com/jeanhadrien/tron-zero-rust/commit/a4ed88cb53beae906b0d717bce0f05811007aa2f))
* release workflow for Windows client + baked-in manager URL ([469615a](https://github.com/jeanhadrien/tron-zero-rust/commit/469615a206281235a49942e7b4cf82b42dfd98a6))
* server-manager integration (announce + browser) with URL/host hardening ([caa6a6b](https://github.com/jeanhadrien/tron-zero-rust/commit/caa6a6b1d1cc04f94ad6822a8932c11bfdcde174))
* switch to manifest config with linked versions ([619e489](https://github.com/jeanhadrien/tron-zero-rust/commit/619e489fbb02610c801f03d2adfe08c8d8f25de5))

## [0.2.0](https://github.com/jeanhadrien/tron-zero-rust/compare/client-v0.1.0...client-v0.2.0) (2026-10-02)


### Features

* add release-please and deploy workflows ([15d11b9](https://github.com/jeanhadrien/tron-zero-rust/commit/15d11b9284ad0950279779a4afcfa241cb734b1b))
* egui menu, settings and HUD with shared theme ([5dddb10](https://github.com/jeanhadrien/tron-zero-rust/commit/5dddb107a8b4ebfa7066ba0eee44a5ddacdafffa))
* layout-honest key labels, physical matching unchanged ([c28acfd](https://github.com/jeanhadrien/tron-zero-rust/commit/c28acfdb5eb49f873d26ac6166e63da65328bf36))
* menu ([a4ed88c](https://github.com/jeanhadrien/tron-zero-rust/commit/a4ed88cb53beae906b0d717bce0f05811007aa2f))
* release workflow for Windows client + baked-in manager URL ([469615a](https://github.com/jeanhadrien/tron-zero-rust/commit/469615a206281235a49942e7b4cf82b42dfd98a6))
* server-manager integration (announce + browser) with URL/host hardening ([caa6a6b](https://github.com/jeanhadrien/tron-zero-rust/commit/caa6a6b1d1cc04f94ad6822a8932c11bfdcde174))
* switch to manifest config with linked versions ([619e489](https://github.com/jeanhadrien/tron-zero-rust/commit/619e489fbb02610c801f03d2adfe08c8d8f25de5))

## Changelog
