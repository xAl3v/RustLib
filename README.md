# RustLib

RustLib is a library mod for Minecraft: Java Edition (Fabric). It ships a native engine written in **Rust** and exposes it through a small, clean Java API, so that other mods can offload procedural noise, raycasting / AABB collision queries and data compression out of the JVM.

RustLib does nothing on its own: it is a dependency for mod developers. If a player installs it only because another mod requires it, there is nothing to configure.

> **Status:** the Java API is pre-1.0 and may change between releases.

## Features

- **Noise generation:** 3D Perlin, 2D/3D Simplex and 3D Worley (cellular) noise, with fBm (1 to 8 octaves). The Perlin grid path uses SIMD; large grids are filled in parallel.
- **Raycasting:** ray vs. a batch of AABBs (nearest hit) and Amanatides-Woo traversal over a voxel grid.
- **AABB overlap queries:** one query box against thousands of boxes in a single call.
- **Compression:** LZ4 block compression/decompression, plus Zstandard in builds compiled with the `zstd` cargo feature.
- **Zero-copy data exchange:** all bulk functions work directly on direct `ByteBuffer`s (off-heap memory); nothing is copied and Rust never owns Java memory.
- **Safe degradation:** if the native library cannot be loaded (unsupported platform, `-Drustlib.disable=true`, ...), the game keeps running and `RustLibAPI.isAvailable()` returns `false`, so your mod can fall back to Java code.
- **Platforms:** Windows, Linux and macOS on x86_64 and aarch64.

## Requirements

- Minecraft: Java Edition
- Fabric Loader (and a Minecraft version matching `fabric.mod.json`)
- Java 25

Only Fabric is supported for now. Forge and NeoForge ports are planned.

## Limitations

- The noise functions are RustLib's own implementations. They are deterministic for a given seed, but they are **not** compatible with vanilla Minecraft's noise and will not reproduce vanilla terrain.
- Every call crosses the JNI boundary, which has a fixed overhead. Native code pays off for batch work (filling grids, large box lists, big buffers); for a handful of single-point queries, plain Java may well be as fast or faster. Benchmark your own workload.
- Zstd is only available if the native library was built with the `zstd` cargo feature (it is off by default because it needs a C compiler). Check `RustLibAPI.isZstdAvailable()`.
- The API is pre-1.0 and may change.

## Quick usage

Add RustLib as a dependency of your mod (see [docs/API.md](docs/API.md#adding-rustlib-as-a-dependency)) and call `RustLibAPI`:

```java
import dev.rustlib.RustLibAPI;
import dev.rustlib.RustLibAPI.NoiseType;
import java.nio.ByteBuffer;

if (RustLibAPI.isAvailable()) {
    ByteBuffer out = RustLibAPI.allocateFloats(16 * 16 * 16);
    RustLibAPI.fillNoise(NoiseType.SIMPLEX_3D, out, 16, 16, 16,
            0f, 0f, 0f,            // origin
            0.05f, 0.05f, 0.05f,   // step per cell
            seed, 4);              // seed, octaves
    float v = out.getFloat(0);
} else {
    // RustLibAPI.status() explains why; use your own Java fallback here.
}
```

The full reference, including buffer rules, memory layouts, error codes and complete recipes, is in **[docs/API.md](docs/API.md)**.

System properties:

| Property | Effect |
|---|---|
| `-Drustlib.disable=true` | Do not load the native library at all. |
| `-Drustlib.native.path=/path/to/librustlib.so` | Load this specific binary instead of the one bundled in the JAR (useful during development). |

## Project layout

```
native/                      Native layer (Rust crate, cdylib "rustlib")
  src/lib.rs                 JNI exports (the FFI boundary)
  src/util.rs                catch_unwind, error codes, DirectByteBuffer access
  src/noise.rs               3D Perlin (SIMD, 8 lanes), 2D/3D Simplex, 3D Worley, fBm, grid fill
  src/raycast.rs             Ray vs. AABB (slabs) and Amanatides-Woo voxel traversal
  src/aabb.rs                Batched AABB overlap queries
  src/compress.rs            LZ4 (always) and Zstd (`zstd` feature)
  src/pool.rs                Dedicated rayon thread pool
src/main/java/dev/rustlib/   Java layer (Fabric mod)
  RustLibMod                 Entrypoint: loads the native library in onInitialize
  NativeBridge               OS/arch detection, extraction from the JAR + SHA-256 check, System.load, `native` methods
  RustLibAPI                 Public API (the only class other mods should use)
  RaycastResult              Result type of the raycast methods
  NativeUnavailableException / NativeAccelerationException
docs/API.md                  API reference
```

## Building

Prerequisites: a JDK 25, a Rust toolchain (`cargo`), and Gradle.

```bash
cd native && cargo test                  # Rust tests (SIMD == scalar, raycast, LZ4, ...)
gradle wrapper --gradle-version 9.2.0    # once, to generate the Gradle wrapper
./gradlew build                          # builds the Rust library for YOUR platform and packages it into the JAR
./gradlew runClient                      # start a development client
```

On Windows use `.\gradlew.bat build` and `.\gradlew.bat runClient`.

A local build contains the native library for your platform only. For releases, the multi-platform CI workflow (`.github/workflows/build.yml`) compiles Windows, Linux and macOS (x86_64 and aarch64) and packages a single JAR containing `natives/<os>-<arch>/<lib>` for every platform.

To include Zstandard, build the native crate with `cargo build --release --features zstd` (requires a C compiler). It is disabled by default; without it the Zstd functions fail with a clear error.

## License

RustLib is distributed under the **GNU LGPL v3.0** (`LICENSE`; the GPL v3 text it incorporates by reference is in `COPYING`). Notices for third-party dependencies are in `THIRD_PARTY_NOTICES.md`.

- You may use RustLib as a dependency of your mod, even if your mod is closed source.
- If you modify RustLib and distribute the result, your changes must be published under the LGPLv3, linking to this repository and stating what you changed.
- The **name "RustLib" and its logo are not licensed** by the LGPL: no permission is granted to use them to name or promote derived projects.

## Memory safety / FFI summary

- No panic crosses the FFI boundary: every exported function runs inside `catch_unwind` and returns an error code (`panic = "unwind"` is mandatory).
- Rust never owns Java memory: direct `ByteBuffer`s are *borrowed* for the duration of a call; there are no `GlobalRef`s and no retained pointers.
- Results are returned by value or written into a caller-supplied buffer (no `NewObject`): nothing to free, nothing to leak.
- Everything is validated: nulls, non-direct buffers, sizes (checked arithmetic), alignment, NaN/infinity and overlapping buffers.
- Loading is verifiable: SHA-256 of the extracted binary, an ABI version check and a smoke test. On failure the library enters a degraded mode with clear log messages.
