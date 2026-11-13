//! FFI utilities: error codes, panic guard and DirectByteBuffer access.
//!
//! MEMORY-SAFETY RULES (summary)
//! 1. No panic may cross the FFI boundary: every exported function goes through `ffi()`,
//!    which uses `catch_unwind` and returns `ERR_PANIC` instead of aborting the JVM.
//! 2. Rust NEVER owns Java memory and never retains pointers after the call returns.
//!    DirectByteBuffers are allocated and freed by the JVM (Cleaner); here they are only "borrowed".
//! 3. No JNI global references (`NewGlobalRef`) are created and no Java objects are returned:
//!    the result travels as an `int` or is written into a caller-provided buffer.
//!    That way there is nothing to free and nothing that can leak.
//! 4. Everything coming from Java is validated (nulls, sizes, alignment, NaN/inf, overlaps)
//!    before slices are built; sizes are computed with checked arithmetic.

use jni::objects::JByteBuffer;
use jni::JNIEnv;
use std::panic::{catch_unwind, AssertUnwindSafe};

pub const ERR_INVALID: i32 = -1; // invalid arguments (null, size, NaN, alignment...)
pub const ERR_BUFFER: i32 = -2; // output buffer too small
pub const ERR_DATA: i32 = -3; // corrupt input data (decompression)
pub const ERR_UNSUPPORTED: i32 = -4; // function not compiled in (e.g. `zstd` feature disabled)
pub const ERR_PANIC: i32 = -99; // panic caught in Rust

/// Runs `f`, catching panics. `Err(code)` is returned to Java as-is.
pub fn ffi<F: FnOnce() -> Result<i32, i32>>(f: F) -> i32 {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => v,
        Ok(Err(code)) => code,
        Err(_) => ERR_PANIC,
    }
}

/// Raw view (pointer + length) of a DirectByteBuffer borrowed from the JVM.
///
/// The buffer's base address and *capacity* are used; `position`/`limit` are ignored
/// (for sub-views, use `slice()` in Java, which adjusts the base address).
#[derive(Clone, Copy)]
pub struct Raw {
    ptr: *mut u8,
    len: usize,
}

impl Raw {
    pub fn get(env: &JNIEnv, buf: &JByteBuffer) -> Result<Raw, i32> {
        // Returns Err if the buffer is null or NOT direct (JNI returns NULL).
        let ptr = env.get_direct_buffer_address(buf).map_err(|_| ERR_INVALID)?;
        let len = env.get_direct_buffer_capacity(buf).map_err(|_| ERR_INVALID)?;
        if ptr.is_null() {
            return Err(ERR_INVALID);
        }
        Ok(Raw { ptr, len })
    }

    pub fn len(&self) -> usize {
        self.len
    }

    /// SAFETY: the JVM keeps the buffer alive for the duration of the native call (the
    /// parameter is a live local reference) and Java must not modify it from another thread meanwhile.
    pub fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// SAFETY: same as `bytes()`, and the caller guarantees (via `overlaps`) that no
    /// other slice is alive over the same memory.
    #[allow(clippy::mut_from_ref)]
    pub fn bytes_mut(&self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }

    /// Do the two buffers share memory? (avoids creating overlapping `&` and `&mut` = UB)
    pub fn overlaps(&self, other: &Raw) -> bool {
        let (a0, a1) = (self.ptr as usize, self.ptr as usize + self.len);
        let (b0, b1) = (other.ptr as usize, other.ptr as usize + other.len);
        a0 < b1 && b0 < a1
    }
}

/// `n * elem` with an overflow check and a check that it fits in `cap`.
pub fn checked_bytes(n: usize, elem: usize, cap: usize) -> Result<usize, i32> {
    let bytes = n.checked_mul(elem).ok_or(ERR_INVALID)?;
    if bytes > cap {
        return Err(ERR_INVALID);
    }
    Ok(bytes)
}

/// Reinterprets bytes as `[T]`; fails (without UB) if alignment or size do not match.
pub fn cast<T: bytemuck::AnyBitPattern>(b: &[u8]) -> Result<&[T], i32> {
    bytemuck::try_cast_slice(b).map_err(|_| ERR_INVALID)
}

pub fn cast_mut<T: bytemuck::NoUninit + bytemuck::AnyBitPattern>(
    b: &mut [u8],
) -> Result<&mut [T], i32> {
    bytemuck::try_cast_slice_mut(b).map_err(|_| ERR_INVALID)
}

pub fn all_finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}
