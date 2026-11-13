# Third-party notices

RustLib is distributed under **LGPL-3.0-only** (see `LICENSE` and `COPYING`).
The native binary bundled in the JAR links the following Rust libraries. All of them are permissively licensed and compatible with the LGPLv3. Each crate keeps its own copyright notice and license text in its repository / crates.io package.

| Crate | License (per its `Cargo.toml`) | Purpose |
|---|---|---|
| jni (+ jni-sys, cesu8, combine, log, thiserror, bytes, memchr) | MIT/Apache-2.0 | JNI interface |
| rayon (+ rayon-core, crossbeam-*, either) | MIT OR Apache-2.0 | Parallelism |
| lz4_flex (+ twox-hash) | MIT | LZ4 compression |
| wide (+ safe_arch) | Zlib OR Apache-2.0 OR MIT | Portable SIMD |
| bytemuck | Zlib OR Apache-2.0 OR MIT | Safe byte reinterpretation |
| windows-sys / windows-targets (Windows only) | MIT OR Apache-2.0 | System bindings |
| zstd, zstd-safe, zstd-sys (only when built with the `zstd` feature) | BSD-3-Clause | Zstandard compression (includes the Zstandard C library, BSD-3-Clause) |

Build tools (syn, quote, proc-macro2, cc, etc.) are not included in the binary.

> Before publishing a release, generate the complete list with license texts and copyright notices (for example with `cargo about` or `cargo-bundle-licenses`) and replace this summary.
