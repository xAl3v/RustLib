# RustLib API reference

> **Pre-1.0 notice:** the Java API is not yet stable and may change between releases. Pin the RustLib version you build against.

RustLib exposes a native (Rust) engine to other Fabric mods. Everything you need lives in the package `dev.rustlib`:

| Type | Purpose |
|---|---|
| `RustLibAPI` | The public API: noise, raycasting, AABB overlap queries, compression, buffer helpers. **The only class other mods should call.** |
| `RaycastResult` (+ `RaycastResult.Face`) | Immutable result of the raycast methods. |
| `NativeUnavailableException` | Thrown when the native engine is not loaded. |
| `NativeAccelerationException` | Thrown when the native code reports an error. |
| `NativeBridge.State`, `NativeBridge.isAvailable()`, `NativeBridge.statusMessage()` | Public status helpers (the rest of `NativeBridge` is an implementation detail). |

## Contents

- [Overview](#overview)
- [Adding RustLib as a dependency](#adding-rustlib-as-a-dependency)
- [Availability and fallback](#availability-and-fallback)
- [Buffer rules](#buffer-rules)
- [Memory layouts](#memory-layouts)
- [Method reference](#method-reference)
- [Types](#types)
- [Errors](#errors)
- [Threading](#threading)
- [System properties](#system-properties)
- [Limitations and performance guidance](#limitations-and-performance-guidance)
- [Recipes](#recipes)

## Overview

All bulk operations work on **direct `ByteBuffer`s**: the native code reads and writes the buffer's memory in place, with no copying. Results are either returned by value (`int`, `float`, `RaycastResult`) or written into a buffer you provide. Rust never keeps a pointer to your memory after a call returns, so there is nothing to close or free.

Typical flow:

1. Check `RustLibAPI.isAvailable()` (once, e.g. at startup).
2. Allocate buffers with `allocateFloats` / `allocateAabbs` (or your own direct buffers, following the [buffer rules](#buffer-rules)).
3. Call the method. Negative native results become a `NativeAccelerationException`; invalid Java-side arguments throw `IllegalArgumentException` / `NullPointerException`.

## Adding RustLib as a dependency

RustLib is not published to a Maven repository. Compile against the RustLib JAR and declare the dependency at runtime through Fabric.

**`fabric.mod.json` of your mod:**

```json
{
  "depends": {
    "rustlib": "*"
  }
}
```

Use `"suggests"` instead of `"depends"` if your mod also works without RustLib (see [Availability and fallback](#availability-and-fallback)). Note that if RustLib is only *suggested* and may be absent, classes that reference `RustLibAPI` must not be loaded when it is missing; keep all calls behind a small wrapper class that you only touch after checking that the mod is loaded (`FabricLoader.getInstance().isModLoaded("rustlib")`).

**`build.gradle`:** put the RustLib JAR somewhere in your project (for example `libs/`) and compile against it:

```groovy
dependencies {
    compileOnly files("libs/rustlib-<version>.jar")
}
```

`compileOnly` is correct: at runtime, Fabric provides RustLib as a separate mod. Do not bundle it inside your own JAR. Use the exact JAR file name you obtained from the RustLib release.

## Availability and fallback

The native engine is loaded lazily and can fail (unsupported OS/architecture, JAR without the binary for the current platform, ABI mismatch, `-Drustlib.disable=true`). In those cases the game continues normally and the API reports it.

```java
if (RustLibAPI.isAvailable()) {
    // fast path
} else {
    LOGGER.info("RustLib acceleration disabled: {}", RustLibAPI.status());
    // pure-Java fallback
}
```

If you call an accelerated method without checking, it throws `NativeUnavailableException` (an `IllegalStateException`) when the engine is not usable. Alternatively catch it:

```java
try {
    RustLibAPI.fillNoise(NoiseType.PERLIN_3D, out, 16, 16, 16, 0, 0, 0, 0.1f, 0.1f, 0.1f, seed, 3);
} catch (NativeUnavailableException e) {
    fillNoiseInJava(out, ...);
}
```

Pure-Java helpers (`allocateFloats`, `allocateAabbs`, `putAabb`) do not need the native engine.

## Buffer rules

These rules apply to every `ByteBuffer` argument.

1. **Direct only.** The buffer must come from `ByteBuffer.allocateDirect` (or `RustLibAPI.allocate*`). Heap buffers cause an `IllegalArgumentException`.
2. **Native byte order.** Buffers of floats, doubles and ints (noise output, AABB boxes, overlap index output) must use `ByteOrder.nativeOrder()`. The Java methods check this where it matters and throw `IllegalArgumentException` otherwise. `allocateFloats` and `allocateAabbs` already return native-order buffers. Compression buffers and voxel grids are raw bytes, so order is irrelevant for them.
3. **Capacity and base address are used; `position` and `limit` are ignored.** The engine always sees the buffer from its base address to its capacity. It never reads or changes `position`/`limit`.
4. **Sub-views with `slice()`.** To operate on part of a buffer, create a view with `slice()` (or `slice(index, length)`). A slice's base address is the start of the view and its capacity is the view's length. **`slice()` resets the byte order to big-endian**, so call `.order(ByteOrder.nativeOrder())` on the slice:

   ```java
   ByteBuffer part = big.slice(offsetBytes, lengthBytes).order(ByteOrder.nativeOrder());
   ```
5. **No concurrent mutation.** Do not modify a buffer from another thread while a call is using it. Different threads may use different buffers concurrently.
6. **Alignment.** Float and int buffers must be 4-byte aligned; AABB (double) buffers must be 8-byte aligned. Buffers from `allocateDirect` are aligned; when slicing, use offsets that are multiples of 4 (floats/ints) or 8 (doubles). A misaligned buffer is rejected with error code -1.
7. **No overlap.** Input and output buffers of one call must not share memory (this includes two slices of the same buffer that intersect). Overlapping buffers are rejected with error code -1.

## Memory layouts

### AABB buffer

Each box is **6 doubles (48 bytes)**, in this order: `minX, minY, minZ, maxX, maxY, maxZ`. Box `i` starts at byte `i * 48`. `RustLibAPI.AABB_BYTES` is `48`. Native byte order, 8-byte aligned.

### Voxel grid

One byte per voxel; **non-zero means solid**, zero means empty. Index of voxel `(x, y, z)`:

```
index = (y * sizeZ + z) * sizeX + x
```

The grid needs `sizeX * sizeY * sizeZ` bytes. Voxel `(0, 0, 0)` has world coordinate `(gridX, gridY, gridZ)` and each voxel is 1 unit wide.

### Noise grid

Floats in native byte order (4 bytes each). Index of cell `(x, y, z)`:

```
index = x + sizeX * (y + sizeY * z)      // byte offset = index * 4
```

The sampling coordinate of cell `(i, j, k)` is `origin + (i, j, k) * step`.

### Overlap index output

`int` (4-byte) indices in native byte order; capacity for `outIndices.capacity() / 4` indices.

## Method reference

### State and configuration

#### `static boolean isAvailable()`

Returns `true` if the native engine is loaded and usable. Triggers loading if no attempt was made yet. Never throws.

#### `static String status()`

Human-readable text: the loaded engine version and platform, or the reason why the engine is unavailable. Never throws.

#### `static boolean isZstdAvailable()`

`true` if the engine is available **and** the native library was built with the `zstd` cargo feature. Zstd methods require this.

#### `static boolean setThreadCount(int threads)`

Sets the number of threads of the native thread pool, which is used internally for large workloads (large noise grids, large AABB lists).

- `threads`: `0` means automatic (half of the logical cores, at least 1). Negative values are treated as `0`. The pool is capped at 256 threads.
- Returns `true` if the setting was applied; `false` if the pool already exists (it is created on the first parallel computation), in which case the setting has no effect. Call it early, before any heavy work.
- Throws `NativeUnavailableException` if the engine is not available.

### Buffer helpers

#### `static final int AABB_BYTES = 48`

Bytes per AABB.

#### `static ByteBuffer allocateFloats(int count)`

Allocates a direct, native-order buffer for `count` floats (`count * 4` bytes). Throws `ArithmeticException` on `int` overflow.

#### `static ByteBuffer allocateAabbs(int count)`

Allocates a direct, native-order buffer for `count` AABBs (`count * 48` bytes). Throws `ArithmeticException` on `int` overflow.

#### `static void putAabb(ByteBuffer boxes, int index, double minX, double minY, double minZ, double maxX, double maxY, double maxZ)`

Writes box number `index` into `boxes` (absolute puts at byte offset `index * 48`). The buffer must be native-order and large enough, otherwise `IndexOutOfBoundsException` is thrown by `ByteBuffer`. It does not touch `position`/`limit`. Does not require the native engine.

```java
ByteBuffer boxes = RustLibAPI.allocateAabbs(2);
RustLibAPI.putAabb(boxes, 0, 0, 0, 0, 1, 1, 1);
RustLibAPI.putAabb(boxes, 1, 5, 0, 0, 6, 2, 1);
```

### Noise

#### `enum RustLibAPI.NoiseType`

| Constant | Description | Output range |
|---|---|---|
| `PERLIN_3D` | Improved 3D Perlin noise. Has a SIMD path in `fillNoise`. | [-1, 1] |
| `SIMPLEX_2D` | 2D Simplex noise; the `z` coordinate is ignored. | [-1, 1] |
| `SIMPLEX_3D` | 3D Simplex noise. | [-1, 1] |
| `WORLEY_3D` | 3D Worley (cellular) noise, F1 = distance to the nearest feature point. | [0, 1] |

All noise is **deterministic**: the same type, seed and coordinates always produce the same value. It is stateless (no per-seed tables), so any `int` seed works. Coordinates should satisfy `|c| < 2^30`. These are RustLib's own implementations and are **not** compatible with vanilla Minecraft noise.

#### `static float sampleNoise3D(float x, float y, float z, int seed)`

3D Perlin noise at one point. Range [-1, 1]. Throws `NativeUnavailableException` if the engine is unavailable. For many points use `fillNoise`.

#### `static float sampleNoise(NoiseType type, float x, float y, float z, int seed)`

Noise of the given type at one point (no fBm). `SIMPLEX_2D` ignores `z`. Throws `NullPointerException` if `type` is `null`. Returns `NaN` if the native code hit an internal error; do not rely on that for normal operation.

#### `static void fillNoise(NoiseType type, ByteBuffer out, int sizeX, int sizeY, int sizeZ, float originX, float originY, float originZ, float stepX, float stepY, float stepZ, int seed, int octaves)`

Fills `out` with fractional Brownian motion (fBm) noise for a `sizeX * sizeY * sizeZ` grid. Layout and coordinates: see [Noise grid](#noise-grid).

| Parameter | Description |
|---|---|
| `type` | Noise type (not `null`). |
| `out` | Direct, native-order buffer, at least `4 * sizeX * sizeY * sizeZ` bytes, 4-byte aligned. Only the first `4 * sizeX * sizeY * sizeZ` bytes are written. |
| `sizeX/Y/Z` | Grid dimensions, each `> 0`. |
| `originX/Y/Z` | World-space coordinate of cell (0, 0, 0). Must be finite. |
| `stepX/Y/Z` | Coordinate distance between neighbouring cells on each axis. Must be finite. |
| `seed` | Noise seed. |
| `octaves` | Number of fBm octaves, `1` to `8`. Lacunarity 2, gain 0.5. Values outside this range are clamped by the native code. |

The octave sum is normalized by the sum of the amplitudes, so the output stays within the same range as the single-sample noise of that type ([-1, 1], or [0, 1] for Worley). Each octave uses a different derived seed.

Exceptions: `NullPointerException` (null `type` or `out`); `IllegalArgumentException` (non-direct buffer, non-native byte order, a dimension `<= 0`, or `out` too small); `NativeAccelerationException` (code -1: non-finite origin/step, misaligned buffer); `NativeUnavailableException`.

```java
ByteBuffer out = RustLibAPI.allocateFloats(16 * 16 * 16);
RustLibAPI.fillNoise(NoiseType.PERLIN_3D, out, 16, 16, 16,
        cx * 16f, 0f, cz * 16f, 0.02f, 0.02f, 0.02f, seed, 4);
float v = out.getFloat((x + 16 * (y + 16 * z)) * 4);
```

### Raycasting and collisions

#### `static RaycastResult raycastAabb(ByteBuffer boxes, int count, double ox, double oy, double oz, double dx, double dy, double dz, double maxDistance)`

Casts a ray against the first `count` boxes of `boxes` and returns the nearest hit.

| Parameter | Description |
|---|---|
| `boxes` | Direct, native-order AABB buffer, 8-byte aligned (see [AABB buffer](#aabb-buffer)). |
| `count` | Number of boxes to test, `0 <= count` and `count * 48 <= boxes.capacity()`. |
| `ox, oy, oz` | Ray origin. Finite. |
| `dx, dy, dz` | Ray direction. Finite and non-zero. It **does not need to be normalized**. |
| `maxDistance` | Maximum distance, `>= 0`, measured along the normalized direction. Hits at exactly `maxDistance` count. |

Semantics:

- The returned `distance` is measured along the **normalized** direction, so the hit point is `origin + normalize(direction) * distance`.
- The nearest box wins; on an exact tie in distance the **lowest box index** wins.
- If the origin is already inside a box, the result has `face == Face.INSIDE` and `distance == 0`.
- No hit within `maxDistance` (or `count == 0`) returns `RaycastResult.MISS`.
- Large lists are processed in parallel internally; the result is the same as a sequential scan.

Exceptions: `NullPointerException`; `IllegalArgumentException` (non-direct, wrong byte order, `count` out of range); `NativeAccelerationException` (code -1: NaN/infinite values, zero direction, negative `maxDistance`, misaligned buffer); `NativeUnavailableException`.

```java
RaycastResult r = RustLibAPI.raycastAabb(boxes, 2, 0, 0.5, 0.5, 1, 0, 0, 100);
if (r.hit()) System.out.println("box " + r.boxIndex() + " at " + r.distance());
```

#### `static RaycastResult raycastVoxels(ByteBuffer grid, int sizeX, int sizeY, int sizeZ, int gridX, int gridY, int gridZ, double ox, double oy, double oz, double dx, double dy, double dz, double maxDistance)`

Casts a ray through a voxel grid using the Amanatides-Woo algorithm and returns the first solid voxel.

| Parameter | Description |
|---|---|
| `grid` | Direct buffer, 1 byte per voxel, non-zero = solid (see [Voxel grid](#voxel-grid)). Only the first `sizeX * sizeY * sizeZ` bytes are used. |
| `sizeX/Y/Z` | Grid dimensions, each `> 0`, with `sizeX * sizeY * sizeZ <= grid.capacity()`. |
| `gridX/Y/Z` | World coordinate of voxel (0, 0, 0). |
| `ox..oz`, `dx..dz`, `maxDistance` | As for `raycastAabb`. |

Semantics:

- Origin, hit point and the returned `voxelX/Y/Z` are in **world coordinates**: `voxelX/Y/Z` already include the `gridX/Y/Z` offset.
- The origin may be outside the grid; the ray is clipped to the grid's bounds. A ray that leaves the grid without hitting a solid voxel is a miss.
- If the origin is inside a solid voxel, the result has `face == Face.INSIDE` and `distance == 0`.
- `face` is the face of the voxel through which the ray entered it (for a ray entering the grid from outside, the face of the grid it crossed).
- `boxIndex` is `-1` for voxel hits.
- Miss: `RaycastResult.MISS`.

Exceptions: `NullPointerException`; `IllegalArgumentException` (non-direct buffer, or dimensions invalid / larger than the buffer); `NativeAccelerationException` (code -1: non-finite values, zero direction, negative `maxDistance`); `NativeUnavailableException`.

#### `static int findOverlappingAabbs(ByteBuffer boxes, int count, double minX, double minY, double minZ, double maxX, double maxY, double maxZ, ByteBuffer outIndices)`

Finds which of the first `count` boxes overlap the query box, writing their indices (ascending) into `outIndices` as native-order `int`s.

- Overlap is **strict**: boxes that merely touch on a face, edge or corner do not count.
- **Returns the total number of overlapping boxes**, which can be larger than the capacity of `outIndices`. Only `outIndices.capacity() / 4` indices are written; if the return value exceeds that, the list is **truncated** and you should call again with a larger buffer (or just use the count).
- `outIndices` must be direct, native-order, 4-byte aligned and must not overlap `boxes`. A zero-capacity buffer is allowed if you only want the count.
- Query values must be finite.

Exceptions: `NullPointerException`; `IllegalArgumentException` (non-direct, wrong byte order, `count` out of range); `NativeAccelerationException` (code -1: non-finite query, misaligned or overlapping buffers); `NativeUnavailableException`.

```java
ByteBuffer found = ByteBuffer.allocateDirect(64 * 4).order(ByteOrder.nativeOrder());
int total = RustLibAPI.findOverlappingAabbs(boxes, n, 0, 0, 0, 1, 2, 1, found);
int stored = Math.min(total, found.capacity() / 4);
```

### Compression

All compression methods work on direct buffers using `[0, srcLength)` of `src` and writing from the start of `dst` (base address; `position` is ignored). They return the **number of bytes written** to `dst`. `src` and `dst` must not overlap. LZ4 is the *block* format (no frame header, the original size is not stored: keep it yourself).

Common exceptions: `NullPointerException`; `IllegalArgumentException` (non-direct buffer, `srcLength < 0` or `srcLength > src.capacity()`); `NativeAccelerationException`; `NativeUnavailableException`.

#### `static int lz4MaxCompressedSize(int inputLength)`

Number of bytes that is always enough to hold the LZ4 compression of `inputLength` bytes. Use it to size `dst` for `lz4Compress`. `NativeAccelerationException` (-1) if `inputLength < 0` or the result does not fit in an `int`.

#### `static int lz4Compress(ByteBuffer src, int srcLength, ByteBuffer dst)`

Compresses `srcLength` bytes. Error -2 if `dst` is too small (use `lz4MaxCompressedSize` to be safe).

#### `static int lz4Decompress(ByteBuffer src, int srcLength, ByteBuffer dst)`

Decompresses an LZ4 block of `srcLength` bytes. `dst` must be at least as large as the original data. Error -2 if `dst` is too small, error -3 if the input is corrupt.

#### `static int zstdCompress(ByteBuffer src, int srcLength, ByteBuffer dst, int level)`

Zstandard compression. `level` is clamped to `1..22`. Requires a build with the `zstd` feature: check `isZstdAvailable()` first; otherwise error -4 is thrown. Error -2 if `dst` is too small. There is no Zstd max-size helper; allocate `dst` generously (for example `srcLength + srcLength / 128 + 1024`, which is a conservative size) and handle -2.

#### `static int zstdDecompress(ByteBuffer src, int srcLength, ByteBuffer dst)`

Decompresses a Zstandard frame. Requires the `zstd` feature (error -4 otherwise). Any failure, including an output buffer that is too small, is reported as error -3.

## Types

### `record RaycastResult(boolean hit, Face face, int boxIndex, double distance, double x, double y, double z, int voxelX, int voxelY, int voxelZ)`

| Component | Meaning |
|---|---|
| `hit` | `true` if something was hit. If `false`, the other components are not meaningful. |
| `face` | Face through which the ray entered the box/voxel (`Face`). |
| `boxIndex` | Index of the hit box (`raycastAabb`), or `-1` (`raycastVoxels`). |
| `distance` | Distance from the origin along the **normalized** direction. |
| `x, y, z` | World-space hit point. |
| `voxelX/Y/Z` | World coordinates of the hit voxel (`raycastVoxels` only; `0` for AABB hits). |

`RaycastResult.MISS` is the shared miss instance: `hit == false`, `face == Face.NONE`, `boxIndex == -1`, `distance == +Infinity`, everything else `0`. Compare with `result.hit()`, not by identity.

### `enum RaycastResult.Face`

`NEG_X, POS_X, NEG_Y, POS_Y, NEG_Z, POS_Z, INSIDE, NONE`

- `NEG_X` / `POS_X` ... : the ray entered through the face on the negative/positive side of that axis. A ray travelling in `+X` therefore enters a box through `NEG_X`.
- `INSIDE`: the origin was already inside the box or solid voxel (`distance == 0`).
- `NONE`: no hit.

### `final class NativeUnavailableException extends IllegalStateException`

Thrown by API methods when the native engine is not loaded. The message contains the reason (also available from `RustLibAPI.status()`).

### `final class NativeAccelerationException extends RuntimeException`

Thrown when the native code returns an error. `int code()` returns the native error code; the message includes the operation name, a description and the code. See [Errors](#errors).

### `NativeBridge` (status only)

`NativeBridge` is an implementation detail, apart from these public members:

- `enum NativeBridge.State { NOT_LOADED, LOADED, FAILED, DISABLED }`
- `static State state()`: current loading state. `DISABLED` means `-Drustlib.disable=true`; `FAILED` means loading was attempted and failed.
- `static boolean isAvailable()`: `state() == LOADED`. Unlike `RustLibAPI.isAvailable()`, it does **not** trigger loading.
- `static String statusMessage()`: description of the state.

Prefer `RustLibAPI.isAvailable()` and `RustLibAPI.status()`.

## Errors

Java-side argument checks throw `IllegalArgumentException` / `NullPointerException` before the native code runs. Errors detected by the native code are negative return codes turned into `NativeAccelerationException`:

| Code | Meaning |
|---|---|
| `-1` | Invalid arguments: null or non-direct buffer, bad size, misalignment, NaN/infinity, zero direction vector, negative `maxDistance`, or overlapping buffers. |
| `-2` | Output buffer too small. |
| `-3` | Corrupt input data (decompression). |
| `-4` | Function not included in this native build (for example Zstd without the `zstd` feature). |
| `-99` | An internal panic was caught in Rust. The game stays stable; details go to stderr. This indicates a bug: please report it. |

## Threading

- All `RustLibAPI` methods are thread-safe and may be called from any thread.
- Raycast results use a per-thread scratch buffer, so concurrent raycasts do not interfere.
- Large noise grids and large AABB lists are processed on a dedicated native thread pool (named `rustlib-N`), separate from rayon's global pool. Configure its size with `setThreadCount` before first use.
- Never mutate a buffer from another thread while a call that uses it is running.

## System properties

| Property | Effect |
|---|---|
| `-Drustlib.disable=true` | Do not load the native library; `isAvailable()` returns `false` and `NativeBridge.state()` is `DISABLED`. |
| `-Drustlib.native.path=/path/to/librustlib.so` | Load this binary instead of the one bundled in the JAR (use `rustlib.dll` / `librustlib.dylib` on Windows / macOS). The file must exist. |

## Limitations and performance guidance

- **Batch your work.** Each call crosses JNI. Prefer one `fillNoise` for a whole chunk over thousands of `sampleNoise` calls, and one `raycastAabb` / `findOverlappingAabbs` over a large box list rather than many tiny ones. For small inputs, plain Java can be as fast or faster; benchmark your own workload before committing.
- Keep and reuse direct buffers. Allocating direct buffers is comparatively expensive and they are freed by the GC, not immediately.
- Noise is not vanilla-compatible. Do not use it where you must reproduce vanilla world generation.
- Precision: noise uses `float`; raycasts and AABBs use `double`. Noise coordinates should stay within `|c| < 2^30`.
- Zstd exists only in builds compiled with the `zstd` feature.
- Supported platforms: Windows, Linux and macOS on x86_64 and aarch64 (64-bit JVMs only), and only when the RustLib JAR contains the binary for that platform.
- Fabric only for now; Forge/NeoForge ports are planned.
- The API is pre-1.0 and may change.

## Recipes

The examples assume these imports:

```java
import dev.rustlib.*;
import dev.rustlib.RustLibAPI.NoiseType;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
```

### 1. Fill a chunk-sized noise grid

```java
static final int N = 16;

float[][][] heightNoise(int chunkX, int chunkZ, int seed) {
    ByteBuffer buf = RustLibAPI.allocateFloats(N * N * N);
    RustLibAPI.fillNoise(NoiseType.SIMPLEX_3D, buf, N, N, N,
            chunkX * (float) N, 0f, chunkZ * (float) N,   // origin in world units
            0.05f, 0.05f, 0.05f,                          // step per block
            seed, 4);                                      // 4 octaves

    float[][][] result = new float[N][N][N];
    for (int z = 0; z < N; z++)
        for (int y = 0; y < N; y++)
            for (int x = 0; x < N; x++)
                result[x][y][z] = buf.getFloat((x + N * (y + N * z)) * 4);
    return result;
}
```

You can also read straight from the buffer (`buf.getFloat(i * 4)`) without copying into arrays.

### 2. Raycast against AABBs

```java
ByteBuffer boxes = RustLibAPI.allocateAabbs(3);
RustLibAPI.putAabb(boxes, 0, 10, 0, 0, 11, 2, 1);   // far
RustLibAPI.putAabb(boxes, 1, 3, 0, 0, 4, 2, 1);     // near
RustLibAPI.putAabb(boxes, 2, 0, 5, 0, 1, 6, 1);     // off to the side

// Direction (2, 0, 0) is not normalized; that is fine.
RaycastResult r = RustLibAPI.raycastAabb(boxes, 3,
        0.0, 0.5, 0.5,      // origin
        2.0, 0.0, 0.0,      // direction
        64.0);              // max distance

if (r.hit()) {
    // r.boxIndex() == 1, r.face() == Face.NEG_X, r.distance() == 3.0
    System.out.printf("hit box %d via %s at (%.2f, %.2f, %.2f)%n",
            r.boxIndex(), r.face(), r.x(), r.y(), r.z());
}
```

### 3. Voxel raycast

```java
int sx = 16, sy = 16, sz = 16;
ByteBuffer grid = ByteBuffer.allocateDirect(sx * sy * sz);   // 1 byte per voxel
int gx = 32, gy = 64, gz = -16;                              // world coordinate of voxel (0,0,0)

// Make the voxel at world (40, 66, -10) solid:
int lx = 40 - gx, ly = 66 - gy, lz = -10 - gz;
grid.put((ly * sz + lz) * sx + lx, (byte) 1);

RaycastResult r = RustLibAPI.raycastVoxels(grid, sx, sy, sz, gx, gy, gz,
        30.5, 66.5, -9.5,       // origin (world coordinates, may be outside the grid)
        1.0, 0.0, 0.0,          // direction
        32.0);

if (r.hit()) {
    // r.voxelX() == 40, r.voxelY() == 66, r.voxelZ() == -10 (world coordinates)
    // r.face() == Face.NEG_X
}
```

### 4. Overlap query

```java
int n = 1000;
ByteBuffer boxes = RustLibAPI.allocateAabbs(n);
// ... fill with RustLibAPI.putAabb(boxes, i, ...) ...

ByteBuffer out = ByteBuffer.allocateDirect(256 * 4).order(ByteOrder.nativeOrder());
int total = RustLibAPI.findOverlappingAabbs(boxes, n, 0, 0, 0, 2, 2, 2, out);

int stored = Math.min(total, out.capacity() / 4);
for (int i = 0; i < stored; i++) {
    int boxIndex = out.getInt(i * 4);
    // handle collision with box boxIndex
}
if (total > stored) {
    // truncated: allocate a buffer for `total` indices and call again
}
```

### 5. LZ4 round trip

```java
byte[] original = ...;                                  // your data
ByteBuffer src = ByteBuffer.allocateDirect(original.length);
src.put(original);

ByteBuffer packed = ByteBuffer.allocateDirect(RustLibAPI.lz4MaxCompressedSize(original.length));
int packedLen = RustLibAPI.lz4Compress(src, original.length, packed);

// Store packedLen and original.length alongside the data: LZ4 blocks do not record the original size.
ByteBuffer restored = ByteBuffer.allocateDirect(original.length);
int restoredLen = RustLibAPI.lz4Decompress(packed, packedLen, restored);   // == original.length

byte[] back = new byte[restoredLen];
restored.get(0, back);   // absolute bulk get
```

For Zstd, guard with `RustLibAPI.isZstdAvailable()` and fall back to LZ4 otherwise.
