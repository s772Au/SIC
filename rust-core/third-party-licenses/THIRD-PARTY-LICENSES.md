# Third-party licenses

Scope: actual normal/build dependency edges for aarch64-pc-windows-gnullvm from this workspace, resolved with Cargo metadata --locked --offline --filter-platform. Dev-only edges are excluded. SQLx default features are disabled; unused MySQL/PostgreSQL/RSA components are absent. The dependency-only patched SQLx SQLite driver is included explicitly as vendored third-party code.

Cargo license expressions are preserved verbatim, including any nonstandard slash-separated expressions. Cached license texts and previously recorded exact-upstream texts are retained. See [DEPENDENCIES.json](DEPENDENCIES.json) for version/source/license checksums and [the driver patch record](sqlx-sqlite-0.8.6/SIC-PATCH.md) for the security-only SQLite dependency update.

| Package | Cargo license expression | License texts | Status |
|---|---|---|---|
| aho-corasick 1.1.5 | Unlicense OR MIT | [COPYING](aho-corasick-1.1.5/COPYING), [LICENSE-MIT](aho-corasick-1.1.5/LICENSE-MIT), [UNLICENSE](aho-corasick-1.1.5/UNLICENSE) | license text included |
| allocator-api2 0.2.21 | MIT OR Apache-2.0 | [LICENSE-APACHE](allocator-api2-0.2.21/LICENSE-APACHE), [LICENSE-MIT](allocator-api2-0.2.21/LICENSE-MIT) | license text included |
| anyhow 1.0.104 | MIT OR Apache-2.0 | [LICENSE-APACHE](anyhow-1.0.104/LICENSE-APACHE), [LICENSE-MIT](anyhow-1.0.104/LICENSE-MIT) | license text included |
| atoi 2.0.0 | MIT | [LICENSE](atoi-2.0.0/LICENSE) | license text included |
| atomic-waker 1.1.2 | Apache-2.0 OR MIT | [LICENSE-APACHE](atomic-waker-1.1.2/LICENSE-APACHE), [LICENSE-MIT](atomic-waker-1.1.2/LICENSE-MIT), [LICENSE-THIRD-PARTY](atomic-waker-1.1.2/LICENSE-THIRD-PARTY) | license text included |
| autocfg 1.5.1 | Apache-2.0 OR MIT | [LICENSE-APACHE](autocfg-1.5.1/LICENSE-APACHE), [LICENSE-MIT](autocfg-1.5.1/LICENSE-MIT) | license text included |
| axum 0.8.9 | MIT | [LICENSE](axum-0.8.9/LICENSE) | license text included |
| axum-core 0.5.6 | MIT | [LICENSE](axum-core-0.5.6/LICENSE) | license text included |
| base64 0.22.1 | MIT OR Apache-2.0 | [LICENSE-APACHE](base64-0.22.1/LICENSE-APACHE), [LICENSE-MIT](base64-0.22.1/LICENSE-MIT) | license text included |
| base64 0.23.1 | MIT OR Apache-2.0 | [LICENSE-APACHE](base64-0.23.1/LICENSE-APACHE), [LICENSE-MIT](base64-0.23.1/LICENSE-MIT) | license text included |
| bit-set 0.8.0 | Apache-2.0 OR MIT | [LICENSE-APACHE](bit-set-0.8.0/LICENSE-APACHE), [LICENSE-MIT](bit-set-0.8.0/LICENSE-MIT) | license text included |
| bit-vec 0.8.0 | Apache-2.0 OR MIT | [LICENSE-APACHE](bit-vec-0.8.0/LICENSE-APACHE), [LICENSE-MIT](bit-vec-0.8.0/LICENSE-MIT) | license text included |
| bitflags 2.13.2 | MIT OR Apache-2.0 | [LICENSE-APACHE](bitflags-2.13.2/LICENSE-APACHE), [LICENSE-MIT](bitflags-2.13.2/LICENSE-MIT) | license text included |
| block-buffer 0.10.4 | MIT OR Apache-2.0 | [LICENSE-APACHE](block-buffer-0.10.4/LICENSE-APACHE), [LICENSE-MIT](block-buffer-0.10.4/LICENSE-MIT) | license text included |
| bstr 1.13.1 | MIT OR Apache-2.0 | [COPYING](bstr-1.13.1/COPYING), [LICENSE-APACHE](bstr-1.13.1/LICENSE-APACHE), [LICENSE-MIT](bstr-1.13.1/LICENSE-MIT) | license text included |
| bytes 1.12.1 | MIT | [LICENSE](bytes-1.12.1/LICENSE) | license text included |
| cc 1.11.0 | MIT OR Apache-2.0 | [LICENSE-APACHE](cc-1.11.0/LICENSE-APACHE), [LICENSE-MIT](cc-1.11.0/LICENSE-MIT) | license text included |
| cfg-if 1.0.5 | MIT OR Apache-2.0 | [LICENSE-APACHE](cfg-if-1.0.5/LICENSE-APACHE), [LICENSE-MIT](cfg-if-1.0.5/LICENSE-MIT) | license text included |
| cfg_aliases 0.2.2 | MIT | [LICENSE](cfg_aliases-0.2.2/LICENSE), [NOTICES.md](cfg_aliases-0.2.2/NOTICES.md) | license text included |
| chacha20 0.10.2 | MIT OR Apache-2.0 | [LICENSE-APACHE](chacha20-0.10.2/LICENSE-APACHE), [LICENSE-MIT](chacha20-0.10.2/LICENSE-MIT) | license text included |
| cpufeatures 0.2.17 | MIT OR Apache-2.0 | [LICENSE-APACHE](cpufeatures-0.2.17/LICENSE-APACHE), [LICENSE-MIT](cpufeatures-0.2.17/LICENSE-MIT) | license text included |
| crc 3.4.0 | MIT OR Apache-2.0 | [LICENSE-APACHE](crc-3.4.0/LICENSE-APACHE), [LICENSE-MIT](crc-3.4.0/LICENSE-MIT) | license text included |
| crc-catalog 2.5.0 | MIT OR Apache-2.0 | [LICENSE-Apache-2.0.txt](crc-catalog-2.5.0/LICENSE-Apache-2.0.txt), [LICENSE-MIT.txt](crc-catalog-2.5.0/LICENSE-MIT.txt) | license text included |
| crossbeam-queue 0.3.14 | MIT OR Apache-2.0 | [LICENSE-APACHE](crossbeam-queue-0.3.14/LICENSE-APACHE), [LICENSE-MIT](crossbeam-queue-0.3.14/LICENSE-MIT) | license text included |
| crossbeam-utils 0.8.23 | MIT OR Apache-2.0 | [LICENSE-APACHE](crossbeam-utils-0.8.23/LICENSE-APACHE), [LICENSE-MIT](crossbeam-utils-0.8.23/LICENSE-MIT) | license text included |
| crypto-common 0.1.7 | MIT OR Apache-2.0 | [LICENSE-APACHE](crypto-common-0.1.7/LICENSE-APACHE), [LICENSE-MIT](crypto-common-0.1.7/LICENSE-MIT) | license text included |
| digest 0.10.7 | MIT OR Apache-2.0 | [LICENSE-APACHE](digest-0.10.7/LICENSE-APACHE), [LICENSE-MIT](digest-0.10.7/LICENSE-MIT) | license text included |
| displaydoc 0.2.7 | MIT OR Apache-2.0 | [LICENSE-APACHE](displaydoc-0.2.7/LICENSE-APACHE), [LICENSE-MIT](displaydoc-0.2.7/LICENSE-MIT) | license text included |
| dotenvy 0.15.7 | MIT | [LICENSE](dotenvy-0.15.7/LICENSE) | license text included |
| either 1.19.0 | MIT OR Apache-2.0 | [LICENSE-APACHE](either-1.19.0/LICENSE-APACHE), [LICENSE-MIT](either-1.19.0/LICENSE-MIT) | license text included |
| equivalent 1.0.3 | Apache-2.0 OR MIT | [LICENSE-APACHE](equivalent-1.0.3/LICENSE-APACHE), [LICENSE-MIT](equivalent-1.0.3/LICENSE-MIT) | license text included |
| event-listener 5.4.2 | Apache-2.0 OR MIT | [LICENSE-APACHE](event-listener-5.4.2/LICENSE-APACHE), [LICENSE-MIT](event-listener-5.4.2/LICENSE-MIT) | license text included |
| fancy-regex 0.19.2 | MIT | [LICENSE](fancy-regex-0.19.2/LICENSE) | license text included |
| find-msvc-tools 0.1.15 | MIT OR Apache-2.0 | [LICENSE-APACHE](find-msvc-tools-0.1.15/LICENSE-APACHE), [LICENSE-MIT](find-msvc-tools-0.1.15/LICENSE-MIT) | license text included |
| flume 0.11.1 | Apache-2.0/MIT | [LICENSE-APACHE](flume-0.11.1/LICENSE-APACHE), [LICENSE-MIT](flume-0.11.1/LICENSE-MIT) | license text included |
| foldhash 0.1.5 | Zlib | [LICENSE](foldhash-0.1.5/LICENSE) | license text included |
| form_urlencoded 1.2.2 | MIT OR Apache-2.0 | [LICENSE-APACHE](form_urlencoded-1.2.2/LICENSE-APACHE), [LICENSE-MIT](form_urlencoded-1.2.2/LICENSE-MIT) | license text included |
| futures-channel 0.3.34 | MIT OR Apache-2.0 | [LICENSE-APACHE](futures-channel-0.3.34/LICENSE-APACHE), [LICENSE-MIT](futures-channel-0.3.34/LICENSE-MIT) | license text included |
| futures-core 0.3.34 | MIT OR Apache-2.0 | [LICENSE-APACHE](futures-core-0.3.34/LICENSE-APACHE), [LICENSE-MIT](futures-core-0.3.34/LICENSE-MIT) | license text included |
| futures-executor 0.3.34 | MIT OR Apache-2.0 | [LICENSE-APACHE](futures-executor-0.3.34/LICENSE-APACHE), [LICENSE-MIT](futures-executor-0.3.34/LICENSE-MIT) | license text included |
| futures-intrusive 0.5.0 | MIT OR Apache-2.0 | [LICENSE-APACHE](futures-intrusive-0.5.0/LICENSE-APACHE), [LICENSE-MIT](futures-intrusive-0.5.0/LICENSE-MIT) | license text included |
| futures-io 0.3.34 | MIT OR Apache-2.0 | [LICENSE-APACHE](futures-io-0.3.34/LICENSE-APACHE), [LICENSE-MIT](futures-io-0.3.34/LICENSE-MIT) | license text included |
| futures-macro 0.3.34 | MIT OR Apache-2.0 | [LICENSE-APACHE](futures-macro-0.3.34/LICENSE-APACHE), [LICENSE-MIT](futures-macro-0.3.34/LICENSE-MIT) | license text included |
| futures-sink 0.3.34 | MIT OR Apache-2.0 | [LICENSE-APACHE](futures-sink-0.3.34/LICENSE-APACHE), [LICENSE-MIT](futures-sink-0.3.34/LICENSE-MIT) | license text included |
| futures-task 0.3.34 | MIT OR Apache-2.0 | [LICENSE-APACHE](futures-task-0.3.34/LICENSE-APACHE), [LICENSE-MIT](futures-task-0.3.34/LICENSE-MIT) | license text included |
| futures-util 0.3.34 | MIT OR Apache-2.0 | [LICENSE-APACHE](futures-util-0.3.34/LICENSE-APACHE), [LICENSE-MIT](futures-util-0.3.34/LICENSE-MIT) | license text included |
| generic-array 0.14.7 | MIT | [LICENSE](generic-array-0.14.7/LICENSE) | license text included |
| getrandom 0.2.17 | MIT OR Apache-2.0 | [LICENSE-APACHE](getrandom-0.2.17/LICENSE-APACHE), [LICENSE-MIT](getrandom-0.2.17/LICENSE-MIT) | license text included |
| getrandom 0.4.3 | MIT OR Apache-2.0 | [LICENSE-APACHE](getrandom-0.4.3/LICENSE-APACHE), [LICENSE-MIT](getrandom-0.4.3/LICENSE-MIT) | license text included |
| hashbrown 0.15.5 | MIT OR Apache-2.0 | [LICENSE-APACHE](hashbrown-0.15.5/LICENSE-APACHE), [LICENSE-MIT](hashbrown-0.15.5/LICENSE-MIT) | license text included |
| hashbrown 0.17.1 | MIT OR Apache-2.0 | [LICENSE-APACHE](hashbrown-0.17.1/LICENSE-APACHE), [LICENSE-MIT](hashbrown-0.17.1/LICENSE-MIT) | license text included |
| hashlink 0.10.0 | MIT OR Apache-2.0 | [LICENSE-APACHE](hashlink-0.10.0/LICENSE-APACHE), [LICENSE-MIT](hashlink-0.10.0/LICENSE-MIT) | license text included |
| heck 0.5.0 | MIT OR Apache-2.0 | [LICENSE-APACHE](heck-0.5.0/LICENSE-APACHE), [LICENSE-MIT](heck-0.5.0/LICENSE-MIT) | license text included |
| hex 0.4.3 | MIT OR Apache-2.0 | [LICENSE-APACHE](hex-0.4.3/LICENSE-APACHE), [LICENSE-MIT](hex-0.4.3/LICENSE-MIT) | license text included |
| http 1.5.0 | MIT OR Apache-2.0 | [LICENSE-APACHE](http-1.5.0/LICENSE-APACHE), [LICENSE-MIT](http-1.5.0/LICENSE-MIT) | license text included |
| http-body 1.1.0 | MIT | [LICENSE](http-body-1.1.0/LICENSE) | license text included |
| http-body-util 0.1.5 | MIT | [LICENSE](http-body-util-0.1.5/LICENSE) | license text included |
| httparse 1.10.1 | MIT OR Apache-2.0 | [LICENSE-APACHE](httparse-1.10.1/LICENSE-APACHE), [LICENSE-MIT](httparse-1.10.1/LICENSE-MIT) | license text included |
| httpdate 1.0.3 | MIT OR Apache-2.0 | [LICENSE-APACHE](httpdate-1.0.3/LICENSE-APACHE), [LICENSE-MIT](httpdate-1.0.3/LICENSE-MIT) | license text included |
| hyper 1.12.0 | MIT | [LICENSE](hyper-1.12.0/LICENSE) | license text included |
| hyper-rustls 0.27.10 | Apache-2.0 OR ISC OR MIT | [LICENSE-APACHE](hyper-rustls-0.27.10/LICENSE-APACHE), [LICENSE-ISC](hyper-rustls-0.27.10/LICENSE-ISC), [LICENSE-MIT](hyper-rustls-0.27.10/LICENSE-MIT) | license text included |
| hyper-util 0.1.21 | MIT | [LICENSE](hyper-util-0.1.21/LICENSE) | license text included |
| icu_collections 2.4.0 | Unicode-3.0 | [LICENSE](icu_collections-2.4.0/LICENSE) | license text included |
| icu_locale_core 2.4.0 | Unicode-3.0 | [LICENSE](icu_locale_core-2.4.0/LICENSE) | license text included |
| icu_normalizer 2.4.0 | Unicode-3.0 | [LICENSE](icu_normalizer-2.4.0/LICENSE) | license text included |
| icu_normalizer_data 2.4.0 | Unicode-3.0 | [LICENSE](icu_normalizer_data-2.4.0/LICENSE) | license text included |
| icu_properties 2.4.0 | Unicode-3.0 | [LICENSE](icu_properties-2.4.0/LICENSE) | license text included |
| icu_properties_data 2.4.0 | Unicode-3.0 | [LICENSE](icu_properties_data-2.4.0/LICENSE) | license text included |
| icu_provider 2.4.0 | Unicode-3.0 | [LICENSE](icu_provider-2.4.0/LICENSE) | license text included |
| idna 1.1.0 | MIT OR Apache-2.0 | [LICENSE-APACHE](idna-1.1.0/LICENSE-APACHE), [LICENSE-MIT](idna-1.1.0/LICENSE-MIT) | license text included |
| idna_adapter 1.2.2 | Apache-2.0 OR MIT | [LICENSE-APACHE](idna_adapter-1.2.2/LICENSE-APACHE), [LICENSE-MIT](idna_adapter-1.2.2/LICENSE-MIT) | license text included |
| indexmap 2.14.2 | Apache-2.0 OR MIT | [LICENSE-APACHE](indexmap-2.14.2/LICENSE-APACHE), [LICENSE-MIT](indexmap-2.14.2/LICENSE-MIT) | license text included |
| ipnet 2.12.2 | MIT OR Apache-2.0 | [LICENSE-APACHE](ipnet-2.12.2/LICENSE-APACHE), [LICENSE-MIT](ipnet-2.12.2/LICENSE-MIT) | license text included |
| itoa 1.0.18 | MIT OR Apache-2.0 | [LICENSE-APACHE](itoa-1.0.18/LICENSE-APACHE), [LICENSE-MIT](itoa-1.0.18/LICENSE-MIT) | license text included |
| libc 0.2.191 | MIT OR Apache-2.0 | [LICENSE-APACHE](libc-0.2.191/LICENSE-APACHE), [LICENSE-MIT](libc-0.2.191/LICENSE-MIT) | license text included |
| libsqlite3-sys 0.39.1 | MIT | [LICENSE](libsqlite3-sys-0.39.1/LICENSE) | license text included |
| litemap 0.8.3 | Unicode-3.0 | [LICENSE](litemap-0.8.3/LICENSE) | license text included |
| lock_api 0.4.14 | MIT OR Apache-2.0 | [LICENSE-APACHE](lock_api-0.4.14/LICENSE-APACHE), [LICENSE-MIT](lock_api-0.4.14/LICENSE-MIT) | license text included |
| log 0.4.35 | MIT OR Apache-2.0 | [LICENSE-APACHE](log-0.4.35/LICENSE-APACHE), [LICENSE-MIT](log-0.4.35/LICENSE-MIT) | license text included |
| lru-slab 0.1.3 | MIT OR Apache-2.0 OR Zlib | [LICENSE-APACHE](lru-slab-0.1.3/LICENSE-APACHE), [LICENSE-MIT](lru-slab-0.1.3/LICENSE-MIT), [LICENSE-ZLIB](lru-slab-0.1.3/LICENSE-ZLIB) | license text included |
| matchit 0.8.4 | MIT AND BSD-3-Clause | [LICENSE](matchit-0.8.4/LICENSE), [LICENSE.httprouter](matchit-0.8.4/LICENSE.httprouter) | license text included |
| memchr 2.8.3 | Unlicense OR MIT | [COPYING](memchr-2.8.3/COPYING), [LICENSE-MIT](memchr-2.8.3/LICENSE-MIT), [UNLICENSE](memchr-2.8.3/UNLICENSE) | license text included |
| mime 0.3.17 | MIT OR Apache-2.0 | [LICENSE-APACHE](mime-0.3.17/LICENSE-APACHE), [LICENSE-MIT](mime-0.3.17/LICENSE-MIT) | license text included |
| mio 1.2.4 | MIT | [LICENSE](mio-1.2.4/LICENSE) | license text included |
| num-traits 0.2.19 | MIT OR Apache-2.0 | [LICENSE-APACHE](num-traits-0.2.19/LICENSE-APACHE), [LICENSE-MIT](num-traits-0.2.19/LICENSE-MIT) | license text included |
| once_cell 1.21.4 | MIT OR Apache-2.0 | [LICENSE-APACHE](once_cell-1.21.4/LICENSE-APACHE), [LICENSE-MIT](once_cell-1.21.4/LICENSE-MIT) | license text included |
| parking 2.2.1 | Apache-2.0 OR MIT | [LICENSE-APACHE](parking-2.2.1/LICENSE-APACHE), [LICENSE-MIT](parking-2.2.1/LICENSE-MIT), [LICENSE-THIRD-PARTY](parking-2.2.1/LICENSE-THIRD-PARTY) | license text included |
| parking_lot 0.12.5 | MIT OR Apache-2.0 | [LICENSE-APACHE](parking_lot-0.12.5/LICENSE-APACHE), [LICENSE-MIT](parking_lot-0.12.5/LICENSE-MIT) | license text included |
| parking_lot_core 0.9.12 | MIT OR Apache-2.0 | [LICENSE-APACHE](parking_lot_core-0.9.12/LICENSE-APACHE), [LICENSE-MIT](parking_lot_core-0.9.12/LICENSE-MIT) | license text included |
| percent-encoding 2.3.2 | MIT OR Apache-2.0 | [LICENSE-APACHE](percent-encoding-2.3.2/LICENSE-APACHE), [LICENSE-MIT](percent-encoding-2.3.2/LICENSE-MIT) | license text included |
| pin-project-lite 0.2.17 | Apache-2.0 OR MIT | [LICENSE-APACHE](pin-project-lite-0.2.17/LICENSE-APACHE), [LICENSE-MIT](pin-project-lite-0.2.17/LICENSE-MIT) | license text included |
| pkg-config 0.3.34 | MIT OR Apache-2.0 | [LICENSE-APACHE](pkg-config-0.3.34/LICENSE-APACHE), [LICENSE-MIT](pkg-config-0.3.34/LICENSE-MIT) | license text included |
| potential_utf 0.1.6 | Unicode-3.0 | [LICENSE](potential_utf-0.1.6/LICENSE) | license text included |
| proc-macro2 1.0.107 | MIT OR Apache-2.0 | [LICENSE-APACHE](proc-macro2-1.0.107/LICENSE-APACHE), [LICENSE-MIT](proc-macro2-1.0.107/LICENSE-MIT) | license text included |
| quinn 0.11.12 | MIT OR Apache-2.0 | [LICENSE-APACHE](quinn-0.11.12/LICENSE-APACHE), [LICENSE-MIT](quinn-0.11.12/LICENSE-MIT) | license text included |
| quinn-proto 0.11.19 | MIT OR Apache-2.0 | [LICENSE-APACHE](quinn-proto-0.11.19/LICENSE-APACHE), [LICENSE-MIT](quinn-proto-0.11.19/LICENSE-MIT) | license text included |
| quinn-udp 0.5.16 | MIT OR Apache-2.0 | [LICENSE-APACHE](quinn-udp-0.5.16/LICENSE-APACHE), [LICENSE-MIT](quinn-udp-0.5.16/LICENSE-MIT) | license text included |
| quote 1.0.47 | MIT OR Apache-2.0 | [LICENSE-APACHE](quote-1.0.47/LICENSE-APACHE), [LICENSE-MIT](quote-1.0.47/LICENSE-MIT) | license text included |
| rand 0.10.3 | MIT OR Apache-2.0 | [COPYRIGHT](rand-0.10.3/COPYRIGHT), [LICENSE-APACHE](rand-0.10.3/LICENSE-APACHE), [LICENSE-MIT](rand-0.10.3/LICENSE-MIT) | license text included |
| rand_core 0.10.1 | MIT OR Apache-2.0 | [COPYRIGHT](rand_core-0.10.1/COPYRIGHT), [LICENSE-APACHE](rand_core-0.10.1/LICENSE-APACHE), [LICENSE-MIT](rand_core-0.10.1/LICENSE-MIT) | license text included |
| rand_pcg 0.10.2 | MIT OR Apache-2.0 | [COPYRIGHT](rand_pcg-0.10.2/COPYRIGHT), [LICENSE-APACHE](rand_pcg-0.10.2/LICENSE-APACHE), [LICENSE-MIT](rand_pcg-0.10.2/LICENSE-MIT) | license text included |
| regex 1.13.1 | MIT OR Apache-2.0 | [LICENSE-APACHE](regex-1.13.1/LICENSE-APACHE), [LICENSE-MIT](regex-1.13.1/LICENSE-MIT) | license text included |
| regex-automata 0.4.18 | MIT OR Apache-2.0 | [LICENSE-APACHE](regex-automata-0.4.18/LICENSE-APACHE), [LICENSE-MIT](regex-automata-0.4.18/LICENSE-MIT) | license text included |
| regex-syntax 0.8.11 | MIT OR Apache-2.0 | [LICENSE-APACHE](regex-syntax-0.8.11/LICENSE-APACHE), [LICENSE-MIT](regex-syntax-0.8.11/LICENSE-MIT) | license text included |
| reqwest 0.12.28 | MIT OR Apache-2.0 | [LICENSE-APACHE](reqwest-0.12.28/LICENSE-APACHE), [LICENSE-MIT](reqwest-0.12.28/LICENSE-MIT) | license text included |
| ring 0.17.14 | Apache-2.0 AND ISC | [LICENSE](ring-0.17.14/LICENSE), [LICENSE-BoringSSL](ring-0.17.14/LICENSE-BoringSSL), [LICENSE-other-bits](ring-0.17.14/LICENSE-other-bits) | license text included |
| rustc-hash 2.1.3 | Apache-2.0 OR MIT | [LICENSE-APACHE](rustc-hash-2.1.3/LICENSE-APACHE), [LICENSE-MIT](rustc-hash-2.1.3/LICENSE-MIT) | license text included |
| rustls 0.23.46 | Apache-2.0 OR ISC OR MIT | [LICENSE-APACHE](rustls-0.23.46/LICENSE-APACHE), [LICENSE-ISC](rustls-0.23.46/LICENSE-ISC), [LICENSE-MIT](rustls-0.23.46/LICENSE-MIT) | license text included |
| rustls-pki-types 1.15.1 | MIT OR Apache-2.0 | [LICENSE-APACHE](rustls-pki-types-1.15.1/LICENSE-APACHE), [LICENSE-MIT](rustls-pki-types-1.15.1/LICENSE-MIT) | license text included |
| rustls-webpki 0.103.16 | ISC | [LICENSE](rustls-webpki-0.103.16/LICENSE) | license text included |
| ryu 1.0.23 | Apache-2.0 OR BSL-1.0 | [LICENSE-APACHE](ryu-1.0.23/LICENSE-APACHE), [LICENSE-BOOST](ryu-1.0.23/LICENSE-BOOST) | license text included |
| scopeguard 1.2.0 | MIT OR Apache-2.0 | [LICENSE-APACHE](scopeguard-1.2.0/LICENSE-APACHE), [LICENSE-MIT](scopeguard-1.2.0/LICENSE-MIT) | license text included |
| serde 1.0.229 | MIT OR Apache-2.0 | [LICENSE-APACHE](serde-1.0.229/LICENSE-APACHE), [LICENSE-MIT](serde-1.0.229/LICENSE-MIT) | license text included |
| serde_core 1.0.229 | MIT OR Apache-2.0 | [LICENSE-APACHE](serde_core-1.0.229/LICENSE-APACHE), [LICENSE-MIT](serde_core-1.0.229/LICENSE-MIT) | license text included |
| serde_derive 1.0.229 | MIT OR Apache-2.0 | [LICENSE-APACHE](serde_derive-1.0.229/LICENSE-APACHE), [LICENSE-MIT](serde_derive-1.0.229/LICENSE-MIT) | license text included |
| serde_json 1.0.154 | MIT OR Apache-2.0 | [LICENSE-APACHE](serde_json-1.0.154/LICENSE-APACHE), [LICENSE-MIT](serde_json-1.0.154/LICENSE-MIT) | license text included |
| serde_path_to_error 0.1.20 | MIT OR Apache-2.0 | [LICENSE-APACHE](serde_path_to_error-0.1.20/LICENSE-APACHE), [LICENSE-MIT](serde_path_to_error-0.1.20/LICENSE-MIT) | license text included |
| serde_urlencoded 0.7.1 | MIT/Apache-2.0 | [LICENSE-APACHE](serde_urlencoded-0.7.1/LICENSE-APACHE), [LICENSE-MIT](serde_urlencoded-0.7.1/LICENSE-MIT) | license text included |
| sha2 0.10.9 | MIT OR Apache-2.0 | [LICENSE-APACHE](sha2-0.10.9/LICENSE-APACHE), [LICENSE-MIT](sha2-0.10.9/LICENSE-MIT) | license text included |
| shlex 2.0.1 | MIT OR Apache-2.0 | [LICENSE-APACHE](shlex-2.0.1/LICENSE-APACHE), [LICENSE-MIT](shlex-2.0.1/LICENSE-MIT) | license text included |
| slab 0.4.12 | MIT | [LICENSE](slab-0.4.12/LICENSE) | license text included |
| smallvec 1.16.4 | MIT OR Apache-2.0 | [LICENSE-APACHE](smallvec-1.16.4/LICENSE-APACHE), [LICENSE-MIT](smallvec-1.16.4/LICENSE-MIT) | license text included |
| socket2 0.6.5 | MIT OR Apache-2.0 | [LICENSE-APACHE](socket2-0.6.5/LICENSE-APACHE), [LICENSE-MIT](socket2-0.6.5/LICENSE-MIT) | license text included |
| spin 0.9.9 | MIT | [LICENSE](spin-0.9.9/LICENSE) | license text included |
| sqlx 0.8.6 | MIT OR Apache-2.0 | [LICENSE-APACHE](sqlx-0.8.6/LICENSE-APACHE), [LICENSE-MIT](sqlx-0.8.6/LICENSE-MIT) | license text included |
| sqlx-core 0.8.6 | MIT OR Apache-2.0 | [LICENSE-APACHE](sqlx-core-0.8.6/LICENSE-APACHE), [LICENSE-MIT](sqlx-core-0.8.6/LICENSE-MIT) | license text included |
| sqlx-macros 0.8.6 | MIT OR Apache-2.0 | [LICENSE-APACHE](sqlx-macros-0.8.6/LICENSE-APACHE), [LICENSE-MIT](sqlx-macros-0.8.6/LICENSE-MIT) | license text included |
| sqlx-macros-core 0.8.6 | MIT OR Apache-2.0 | [LICENSE-APACHE](sqlx-macros-core-0.8.6/LICENSE-APACHE), [LICENSE-MIT](sqlx-macros-core-0.8.6/LICENSE-MIT) | license text included |
| sqlx-sqlite 0.8.6 | MIT OR Apache-2.0 | [LICENSE-APACHE](sqlx-sqlite-0.8.6/LICENSE-APACHE), [LICENSE-MIT](sqlx-sqlite-0.8.6/LICENSE-MIT) | vendored driver; upstream license unchanged |
| stable_deref_trait 1.2.1 | MIT OR Apache-2.0 | [LICENSE-APACHE](stable_deref_trait-1.2.1/LICENSE-APACHE), [LICENSE-MIT](stable_deref_trait-1.2.1/LICENSE-MIT) | license text included |
| subtle 2.6.1 | BSD-3-Clause | [LICENSE](subtle-2.6.1/LICENSE) | license text included |
| syn 2.0.119 | MIT OR Apache-2.0 | [LICENSE-APACHE](syn-2.0.119/LICENSE-APACHE), [LICENSE-MIT](syn-2.0.119/LICENSE-MIT) | license text included |
| syn 3.0.9 | MIT OR Apache-2.0 | [LICENSE-APACHE](syn-3.0.9/LICENSE-APACHE), [LICENSE-MIT](syn-3.0.9/LICENSE-MIT) | license text included |
| sync_wrapper 1.0.2 | Apache-2.0 | [LICENSE](sync_wrapper-1.0.2/LICENSE) | license text included |
| synstructure 0.14.0 | MIT | [LICENSE](synstructure-0.14.0/LICENSE) | license text included |
| thiserror 2.0.21 | MIT OR Apache-2.0 | [LICENSE-APACHE](thiserror-2.0.21/LICENSE-APACHE), [LICENSE-MIT](thiserror-2.0.21/LICENSE-MIT) | license text included |
| thiserror-impl 2.0.21 | MIT OR Apache-2.0 | [LICENSE-APACHE](thiserror-impl-2.0.21/LICENSE-APACHE), [LICENSE-MIT](thiserror-impl-2.0.21/LICENSE-MIT) | license text included |
| tiktoken-rs 0.12.1 | MIT | [LICENSE](tiktoken-rs-0.12.1/LICENSE) | license text included |
| tinystr 0.8.5 | Unicode-3.0 | [LICENSE](tinystr-0.8.5/LICENSE) | license text included |
| tinyvec 1.13.3 | Zlib OR Apache-2.0 OR MIT | [LICENSE-APACHE.md](tinyvec-1.13.3/LICENSE-APACHE.md), [LICENSE-MIT.md](tinyvec-1.13.3/LICENSE-MIT.md), [LICENSE-ZLIB.md](tinyvec-1.13.3/LICENSE-ZLIB.md) | license text included |
| tokio 1.54.1 | MIT | [LICENSE](tokio-1.54.1/LICENSE) | license text included |
| tokio-macros 2.7.2 | MIT | [LICENSE](tokio-macros-2.7.2/LICENSE) | license text included |
| tokio-rustls 0.26.6 | MIT OR Apache-2.0 | [LICENSE-APACHE](tokio-rustls-0.26.6/LICENSE-APACHE), [LICENSE-MIT](tokio-rustls-0.26.6/LICENSE-MIT) | license text included |
| tokio-stream 0.1.19 | MIT | [LICENSE](tokio-stream-0.1.19/LICENSE) | license text included |
| tokio-util 0.7.20 | MIT | [LICENSE](tokio-util-0.7.20/LICENSE) | license text included |
| tower 0.5.3 | MIT | [LICENSE](tower-0.5.3/LICENSE) | license text included |
| tower-http 0.6.11 | MIT | [LICENSE](tower-http-0.6.11/LICENSE) | license text included |
| tower-layer 0.3.3 | MIT | [LICENSE](tower-layer-0.3.3/LICENSE) | license text included |
| tower-service 0.3.3 | MIT | [LICENSE](tower-service-0.3.3/LICENSE) | license text included |
| tracing 0.1.44 | MIT | [LICENSE](tracing-0.1.44/LICENSE) | license text included |
| tracing-attributes 0.1.31 | MIT | [LICENSE](tracing-attributes-0.1.31/LICENSE) | license text included |
| tracing-core 0.1.36 | MIT | [LICENSE](tracing-core-0.1.36/LICENSE) | license text included |
| try-lock 0.2.5 | MIT | [LICENSE](try-lock-0.2.5/LICENSE) | license text included |
| typenum 1.20.1 | MIT OR Apache-2.0 | [LICENSE](typenum-1.20.1/LICENSE), [LICENSE-APACHE](typenum-1.20.1/LICENSE-APACHE), [LICENSE-MIT](typenum-1.20.1/LICENSE-MIT) | license text included |
| unicode-ident 1.0.27 | (MIT OR Apache-2.0) AND Unicode-3.0 | [LICENSE-APACHE](unicode-ident-1.0.27/LICENSE-APACHE), [LICENSE-MIT](unicode-ident-1.0.27/LICENSE-MIT), [LICENSE-UNICODE](unicode-ident-1.0.27/LICENSE-UNICODE) | license text included |
| untrusted 0.9.0 | ISC | [LICENSE.txt](untrusted-0.9.0/LICENSE.txt) | license text included |
| url 2.5.8 | MIT OR Apache-2.0 | [LICENSE-APACHE](url-2.5.8/LICENSE-APACHE), [LICENSE-MIT](url-2.5.8/LICENSE-MIT) | license text included |
| utf8_iter 1.1.0 | Apache-2.0 OR MIT | [COPYRIGHT](utf8_iter-1.1.0/COPYRIGHT), [LICENSE-APACHE](utf8_iter-1.1.0/LICENSE-APACHE), [LICENSE-MIT](utf8_iter-1.1.0/LICENSE-MIT) | license text included |
| utf_types 1.0.0 | Apache-2.0 OR MIT | [COPYRIGHT](utf_types-1.0.0/COPYRIGHT), [LICENSE-APACHE](utf_types-1.0.0/LICENSE-APACHE), [LICENSE-MIT](utf_types-1.0.0/LICENSE-MIT) | license text included |
| uuid 1.29.0 | Apache-2.0 OR MIT | [LICENSE-APACHE](uuid-1.29.0/LICENSE-APACHE), [LICENSE-MIT](uuid-1.29.0/LICENSE-MIT) | license text included |
| vcpkg 0.2.15 | MIT/Apache-2.0 | [LICENSE-APACHE](vcpkg-0.2.15/LICENSE-APACHE), [LICENSE-MIT](vcpkg-0.2.15/LICENSE-MIT) | license text included |
| version_check 0.9.5 | MIT/Apache-2.0 | [LICENSE-APACHE](version_check-0.9.5/LICENSE-APACHE), [LICENSE-MIT](version_check-0.9.5/LICENSE-MIT) | license text included |
| want 0.3.2 | MIT | [LICENSE](want-0.3.2/LICENSE) | license text included |
| webpki-roots 1.0.11 | CDLA-Permissive-2.0 | [LICENSE](webpki-roots-1.0.11/LICENSE) | license text included |
| windows-link 0.2.1 | MIT OR Apache-2.0 | [license-apache-2.0](windows-link-0.2.1/license-apache-2.0), [license-mit](windows-link-0.2.1/license-mit) | license text included |
| windows-sys 0.52.0 | MIT OR Apache-2.0 | [license-apache-2.0](windows-sys-0.52.0/license-apache-2.0), [license-mit](windows-sys-0.52.0/license-mit) | license text included |
| windows-sys 0.61.2 | MIT OR Apache-2.0 | [license-apache-2.0](windows-sys-0.61.2/license-apache-2.0), [license-mit](windows-sys-0.61.2/license-mit) | license text included |
| windows-targets 0.52.6 | MIT OR Apache-2.0 | [license-apache-2.0](windows-targets-0.52.6/license-apache-2.0), [license-mit](windows-targets-0.52.6/license-mit) | license text included |
| windows_aarch64_gnullvm 0.52.6 | MIT OR Apache-2.0 | [license-apache-2.0](windows_aarch64_gnullvm-0.52.6/license-apache-2.0), [license-mit](windows_aarch64_gnullvm-0.52.6/license-mit) | license text included |
| writeable 0.6.4 | Unicode-3.0 | [LICENSE](writeable-0.6.4/LICENSE) | license text included |
| yoke 0.8.4 | Unicode-3.0 | [LICENSE](yoke-0.8.4/LICENSE) | license text included |
| yoke-derive 0.8.4 | Unicode-3.0 | [LICENSE](yoke-derive-0.8.4/LICENSE) | license text included |
| zerofrom 0.1.8 | Unicode-3.0 | [LICENSE](zerofrom-0.1.8/LICENSE) | license text included |
| zerofrom-derive 0.1.8 | Unicode-3.0 | [LICENSE](zerofrom-derive-0.1.8/LICENSE) | license text included |
| zeroize 1.9.1 | Apache-2.0 OR MIT | [LICENSE-APACHE](zeroize-1.9.1/LICENSE-APACHE), [LICENSE-MIT](zeroize-1.9.1/LICENSE-MIT) | license text included |
| zerotrie 0.2.5 | Unicode-3.0 | [LICENSE](zerotrie-0.2.5/LICENSE) | license text included |
| zerovec 0.11.9 | Unicode-3.0 | [LICENSE](zerovec-0.11.9/LICENSE) | license text included |
| zerovec-derive 0.11.7 | Unicode-3.0 | [LICENSE](zerovec-derive-0.11.7/LICENSE) | license text included |
| zmij 1.0.23 | MIT | [LICENSE-MIT](zmij-1.0.23/LICENSE-MIT) | license text included |

Summary: 179 reachable third-party packages; all have license texts included. No advisory is ignored.
