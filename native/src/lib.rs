//! rustlib — native engine for Fabric mods.
//!
//! JNI boundary. Associated Java class: `dev.rustlib.NativeBridge` (`static native` methods).
//!
//! FFI BEST PRACTICES applied in this file (see also `util.rs`):
//! - `extern "system"` + `#[no_mangle]`: correct ABI on Windows (stdcall on x86) and Unix.
//! - The whole body runs inside `ffi()`: a panic never crosses into the JVM.
//! - No mutable global state except the thread pool (OnceLock): there is nothing to "free".
//! - Bulk functions operate on DirectByteBuffers (the JVM's off-heap memory):
//!   Rust reads/writes it in place, without copying it or owning it.
//! - Java exceptions are never thrown from Rust; negative codes are returned
//!   (`util::ERR_*`) and the Java layer translates them into exceptions with a clear message.
//! - "Structured" results (RaycastResult) are serialized into a caller-provided buffer:
//!   this avoids NewObject/FindClass/GlobalRef, the typical source of leaks and JNI overhead.

pub mod aabb;
pub mod compress;
pub mod noise;
pub mod pool;
pub mod raycast;
pub mod util;

use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jdouble, jfloat, jint, jstring};
use jni::JNIEnv;
use noise::Kind;
use raycast::RESULT_SIZE;
use util::*;

/// Must match `NativeBridge.EXPECTED_ABI`. Bump it if any signature/layout changes.
const ABI_VERSION: jint = 1;

const FEATURE_ZSTD: jint = 1;

// ============================================================================ meta

#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeAbiVersion(_env: JNIEnv, _c: JClass) -> jint {
    ABI_VERSION
}

#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeFeatures(_env: JNIEnv, _c: JClass) -> jint {
    if cfg!(feature = "zstd") {
        FEATURE_ZSTD
    } else {
        0
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeVersion(env: JNIEnv, _c: JClass) -> jstring {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        env.new_string(env!("CARGO_PKG_VERSION")).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// Sets the pool's thread count (0 = automatic). Returns 0 = applied, 1 = the pool was already created.
#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeInit(_env: JNIEnv, _c: JClass, threads: jint) -> jint {
    ffi(|| Ok(if pool::configure(threads.max(0) as usize) { 0 } else { 1 }))
}

// ============================================================================ noise

/// native_sample_noise_3d(x, y, z, seed) -> f32  (3D Perlin). NaN if a panic occurs.
#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeSampleNoise3D(
    _env: JNIEnv,
    _c: JClass,
    x: jfloat,
    y: jfloat,
    z: jfloat,
    seed: jint,
) -> jfloat {
    std::panic::catch_unwind(|| noise::perlin3(seed as u32, x, y, z)).unwrap_or(f32::NAN)
}

#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeSampleNoise(
    _env: JNIEnv,
    _c: JClass,
    kind: jint,
    x: jfloat,
    y: jfloat,
    z: jfloat,
    seed: jint,
) -> jfloat {
    std::panic::catch_unwind(|| match Kind::from_i32(kind) {
        Some(k) => noise::sample(k, seed as u32, x, y, z),
        None => f32::NAN,
    })
    .unwrap_or(f32::NAN)
}

/// Fills a DirectByteBuffer of floats (native byte order) with 3D fBm noise. Returns the number of values.
#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeFillNoise(
    env: JNIEnv,
    _c: JClass,
    kind: jint,
    out: JByteBuffer,
    nx: jint,
    ny: jint,
    nz: jint,
    ox: jfloat,
    oy: jfloat,
    oz: jfloat,
    sx: jfloat,
    sy: jfloat,
    sz: jfloat,
    seed: jint,
    octaves: jint,
) -> jint {
    ffi(|| {
        let kind = Kind::from_i32(kind).ok_or(ERR_INVALID)?;
        if nx <= 0 || ny <= 0 || nz <= 0 {
            return Err(ERR_INVALID);
        }
        if ![ox, oy, oz, sx, sy, sz].iter().all(|v| v.is_finite()) {
            return Err(ERR_INVALID);
        }
        let raw = Raw::get(&env, &out)?;
        let total = (nx as usize)
            .checked_mul(ny as usize)
            .and_then(|v| v.checked_mul(nz as usize))
            .ok_or(ERR_INVALID)?;
        let bytes = checked_bytes(total, 4, raw.len())?;
        let dst = cast_mut::<f32>(&mut raw.bytes_mut()[..bytes])?;
        noise::fill_grid(kind, dst, nx as usize, ny as usize, [ox, oy, oz], [sx, sy, sz], seed as u32, octaves.max(1) as u32);
        Ok(total as jint)
    })
}

// ============================================================================ raycast / AABB

/// native_raycast_aabb: ray against N boxes. 1 = hit, 0 = miss, <0 = error.
/// `out` receives the serialized RaycastResult (see raycast.rs, RESULT_SIZE bytes).
#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeRaycastAabb(
    env: JNIEnv,
    _c: JClass,
    boxes: JByteBuffer,
    count: jint,
    ox: jdouble,
    oy: jdouble,
    oz: jdouble,
    dx: jdouble,
    dy: jdouble,
    dz: jdouble,
    max_dist: jdouble,
    out: JByteBuffer,
) -> jint {
    ffi(|| {
        if count < 0 || !all_finite(&[ox, oy, oz, dx, dy, dz, max_dist]) || max_dist < 0.0 {
            return Err(ERR_INVALID);
        }
        let d = raycast::normalize([dx, dy, dz]).ok_or(ERR_INVALID)?;
        let (b, o) = (Raw::get(&env, &boxes)?, Raw::get(&env, &out)?);
        if o.len() < RESULT_SIZE || b.overlaps(&o) {
            return Err(ERR_INVALID);
        }
        let bytes = checked_bytes(count as usize, 48, b.len())?;
        let boxes = cast::<f64>(&b.bytes()[..bytes])?;
        let hit = raycast::raycast_aabbs(boxes, [ox, oy, oz], d, max_dist);
        hit.write_to(o.bytes_mut());
        Ok(hit.hit as jint)
    })
}

/// Amanatides-Woo raycast over a voxel grid (1 byte/voxel, != 0 is solid).
/// Index (y*sz + z)*sx + x. `gx,gy,gz` = world coordinate of voxel (0,0,0).
#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeRaycastVoxels(
    env: JNIEnv,
    _c: JClass,
    grid: JByteBuffer,
    sx: jint,
    sy: jint,
    sz: jint,
    gx: jint,
    gy: jint,
    gz: jint,
    ox: jdouble,
    oy: jdouble,
    oz: jdouble,
    dx: jdouble,
    dy: jdouble,
    dz: jdouble,
    max_dist: jdouble,
    out: JByteBuffer,
) -> jint {
    ffi(|| {
        if sx <= 0 || sy <= 0 || sz <= 0 || !all_finite(&[ox, oy, oz, dx, dy, dz, max_dist]) || max_dist < 0.0 {
            return Err(ERR_INVALID);
        }
        let d = raycast::normalize([dx, dy, dz]).ok_or(ERR_INVALID)?;
        let (g, o) = (Raw::get(&env, &grid)?, Raw::get(&env, &out)?);
        if o.len() < RESULT_SIZE || g.overlaps(&o) {
            return Err(ERR_INVALID);
        }
        let cells = (sx as usize)
            .checked_mul(sy as usize)
            .and_then(|v| v.checked_mul(sz as usize))
            .ok_or(ERR_INVALID)?;
        let bytes = checked_bytes(cells, 1, g.len())?;
        let hit = raycast::raycast_voxels(&g.bytes()[..bytes], [sx, sy, sz], [gx, gy, gz], [ox, oy, oz], d, max_dist);
        hit.write_to(o.bytes_mut());
        Ok(hit.hit as jint)
    })
}

/// Query box vs N boxes. Returns the TOTAL number of overlaps (may exceed the capacity
/// of `out`, which receives at most capacity/4 i32 indices).
#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeAabbOverlaps(
    env: JNIEnv,
    _c: JClass,
    boxes: JByteBuffer,
    count: jint,
    min_x: jdouble,
    min_y: jdouble,
    min_z: jdouble,
    max_x: jdouble,
    max_y: jdouble,
    max_z: jdouble,
    out: JByteBuffer,
) -> jint {
    ffi(|| {
        let q = [min_x, min_y, min_z, max_x, max_y, max_z];
        if count < 0 || !all_finite(&q) {
            return Err(ERR_INVALID);
        }
        let (b, o) = (Raw::get(&env, &boxes)?, Raw::get(&env, &out)?);
        if b.overlaps(&o) {
            return Err(ERR_INVALID);
        }
        let bytes = checked_bytes(count as usize, 48, b.len())?;
        let boxes = cast::<f64>(&b.bytes()[..bytes])?;
        let out_bytes = (o.len() / 4) * 4;
        let out = cast_mut::<i32>(&mut o.bytes_mut()[..out_bytes])?;
        Ok(aabb::overlaps(boxes, q, out) as jint)
    })
}

// ============================================================================ compression

#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeLz4MaxSize(_env: JNIEnv, _c: JClass, len: jint) -> jint {
    ffi(|| {
        if len < 0 {
            return Err(ERR_INVALID);
        }
        i32::try_from(compress::lz4_max_size(len as usize)).map_err(|_| ERR_INVALID)
    })
}

/// Prepares (src, dst): validates lengths and absence of overlap. `src_len` <= src capacity.
fn io_pair<'a>(env: &JNIEnv, src: &JByteBuffer, src_len: jint, dst: &JByteBuffer) -> Result<(&'a [u8], &'a mut [u8]), i32> {
    if src_len < 0 {
        return Err(ERR_INVALID);
    }
    let (s, d) = (Raw::get(env, src)?, Raw::get(env, dst)?);
    if src_len as usize > s.len() || s.overlaps(&d) {
        return Err(ERR_INVALID);
    }
    // SAFETY: slices over memory that the JVM keeps alive for the duration of the call, with no overlap.
    // The 'a lifetime is a local fiction: the slices do not escape the FFI function.
    unsafe {
        let s: &'a [u8] = std::mem::transmute(&s.bytes()[..src_len as usize]);
        let d: &'a mut [u8] = std::mem::transmute(d.bytes_mut());
        Ok((s, d))
    }
}

fn to_jint(n: usize) -> Result<jint, i32> {
    i32::try_from(n).map_err(|_| ERR_INVALID)
}

#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeLz4Compress(
    env: JNIEnv,
    _c: JClass,
    src: JByteBuffer,
    src_len: jint,
    dst: JByteBuffer,
) -> jint {
    ffi(|| {
        let (s, d) = io_pair(&env, &src, src_len, &dst)?;
        to_jint(compress::lz4_compress(s, d)?)
    })
}

#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeLz4Decompress(
    env: JNIEnv,
    _c: JClass,
    src: JByteBuffer,
    src_len: jint,
    dst: JByteBuffer,
) -> jint {
    ffi(|| {
        let (s, d) = io_pair(&env, &src, src_len, &dst)?;
        to_jint(compress::lz4_decompress(s, d)?)
    })
}

#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeZstdCompress(
    env: JNIEnv,
    _c: JClass,
    src: JByteBuffer,
    src_len: jint,
    dst: JByteBuffer,
    level: jint,
) -> jint {
    ffi(|| {
        let (s, d) = io_pair(&env, &src, src_len, &dst)?;
        #[cfg(feature = "zstd")]
        {
            to_jint(compress::zstd_compress(s, d, level)?)
        }
        #[cfg(not(feature = "zstd"))]
        {
            let _ = (s, d, level);
            Err(ERR_UNSUPPORTED)
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_dev_rustlib_NativeBridge_nativeZstdDecompress(
    env: JNIEnv,
    _c: JClass,
    src: JByteBuffer,
    src_len: jint,
    dst: JByteBuffer,
) -> jint {
    ffi(|| {
        let (s, d) = io_pair(&env, &src, src_len, &dst)?;
        #[cfg(feature = "zstd")]
        {
            to_jint(compress::zstd_decompress(s, d)?)
        }
        #[cfg(not(feature = "zstd"))]
        {
            let _ = (s, d);
            Err(ERR_UNSUPPORTED)
        }
    })
}
