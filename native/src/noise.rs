//! Procedural noise: 3D Perlin (with a SIMD path), 2D/3D Simplex and 3D Worley (F1).
//!
//! "Zero allocation" design: the noise is *stateless*. Instead of permutation tables
//! (which would have to be built/cached per seed) a high-quality integer hash is used
//! (`hash3`), so any seed works without allocating heap memory.
//!
//! Output ranges: Perlin/Simplex in [-1, 1]; Worley (normalized F1 distance) in [0, 1].
//! Supported coordinates: |c| < 2^30 (outside that range the i32 rounding is no longer reliable).

use crate::pool;
use rayon::prelude::*;
use wide::{f32x8, i32x8, CmpEq, CmpLt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Perlin3 = 0,
    Simplex2 = 1,
    Simplex3 = 2,
    Worley3 = 3,
}

impl Kind {
    pub fn from_i32(v: i32) -> Option<Kind> {
        match v {
            0 => Some(Kind::Perlin3),
            1 => Some(Kind::Simplex2),
            2 => Some(Kind::Simplex3),
            3 => Some(Kind::Worley3),
            _ => None,
        }
    }
}

const P1: u32 = 0x27d4_eb2d;
const P2: u32 = 0x1656_67b1;
const P3: u32 = 0x9e37_79b1;
const OCT_STEP: u32 = 0x9e37_79b9;

// ---------------------------------------------------------------- scalar hash

#[inline(always)]
fn mix(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h
}

#[inline(always)]
fn hash3(seed: u32, x: i32, y: i32, z: i32) -> u32 {
    mix(seed ^ (x as u32).wrapping_mul(P1) ^ (y as u32).wrapping_mul(P2) ^ (z as u32).wrapping_mul(P3))
}

// ---------------------------------------------------------------- Perlin 3D (scalar)

#[inline(always)]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline(always)]
fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

/// Improved Perlin gradient (12 directions + 4 repeated), table-free.
#[inline(always)]
fn grad3(h: u32, x: f32, y: f32, z: f32) -> f32 {
    let h = h & 15;
    let u = if h < 8 { x } else { y };
    let v = if h < 4 {
        y
    } else if h == 12 || h == 14 {
        x
    } else {
        z
    };
    (if h & 1 == 0 { u } else { -u }) + (if h & 2 == 0 { v } else { -v })
}

pub fn perlin3(seed: u32, x: f32, y: f32, z: f32) -> f32 {
    let (xf, yf, zf) = (x.floor(), y.floor(), z.floor());
    let (xi, yi, zi) = (xf as i32, yf as i32, zf as i32);
    let (fx, fy, fz) = (x - xf, y - yf, z - zf);
    let (u, v, w) = (fade(fx), fade(fy), fade(fz));
    let (x1, y1, z1) = (xi.wrapping_add(1), yi.wrapping_add(1), zi.wrapping_add(1));

    let n000 = grad3(hash3(seed, xi, yi, zi), fx, fy, fz);
    let n100 = grad3(hash3(seed, x1, yi, zi), fx - 1.0, fy, fz);
    let n010 = grad3(hash3(seed, xi, y1, zi), fx, fy - 1.0, fz);
    let n110 = grad3(hash3(seed, x1, y1, zi), fx - 1.0, fy - 1.0, fz);
    let n001 = grad3(hash3(seed, xi, yi, z1), fx, fy, fz - 1.0);
    let n101 = grad3(hash3(seed, x1, yi, z1), fx - 1.0, fy, fz - 1.0);
    let n011 = grad3(hash3(seed, xi, y1, z1), fx, fy - 1.0, fz - 1.0);
    let n111 = grad3(hash3(seed, x1, y1, z1), fx - 1.0, fy - 1.0, fz - 1.0);

    let r = lerp(
        w,
        lerp(v, lerp(u, n000, n100), lerp(u, n010, n110)),
        lerp(v, lerp(u, n001, n101), lerp(u, n011, n111)),
    );
    r.clamp(-1.0, 1.0)
}

// ---------------------------------------------------------------- Perlin 3D (SIMD, 8 lanes en X)

#[inline(always)]
fn mix8(h: i32x8) -> i32x8 {
    // `>>` on i32x8 is arithmetic: it is masked to emulate the logical shift of u32.
    let h = h ^ ((h >> 16) & i32x8::splat(0xFFFF));
    let h = h * i32x8::splat(0x7feb_352d_u32 as i32);
    let h = h ^ ((h >> 15) & i32x8::splat(0x1_FFFF));
    let h = h * i32x8::splat(0x846c_a68b_u32 as i32);
    h ^ ((h >> 16) & i32x8::splat(0xFFFF))
}

/// `yz` = precomputed contribution of (y, z) to the hash (identical to the scalar path's).
#[inline(always)]
fn hash8(seed: u32, xi: i32x8, yz: u32) -> i32x8 {
    mix8(i32x8::splat((seed ^ yz) as i32) ^ (xi * i32x8::splat(P1 as i32)))
}

#[inline(always)]
fn sel(mask: i32x8, a: f32x8, b: f32x8) -> f32x8 {
    // mask = all ones where the condition is true
    bytemuck::cast::<i32x8, f32x8>(mask).blend(a, b)
}

#[inline(always)]
fn grad8(h: i32x8, x: f32x8, y: f32x8, z: f32x8) -> f32x8 {
    let h = h & i32x8::splat(15);
    let lt8 = h.cmp_lt(i32x8::splat(8));
    let lt4 = h.cmp_lt(i32x8::splat(4));
    let is12_14 = h.cmp_eq(i32x8::splat(12)) | h.cmp_eq(i32x8::splat(14));
    let zero = i32x8::splat(0);
    let u = sel(lt8, x, y);
    let v = sel(lt4, y, sel(is12_14, x, z));
    let su = sel((h & i32x8::splat(1)).cmp_eq(zero), u, -u);
    let sv = sel((h & i32x8::splat(2)).cmp_eq(zero), v, -v);
    su + sv
}

#[inline(always)]
fn fade8(t: f32x8) -> f32x8 {
    let c6 = f32x8::splat(6.0);
    t * t * t * (t * (t * c6 - f32x8::splat(15.0)) + f32x8::splat(10.0))
}

#[inline(always)]
fn lerp8(t: f32x8, a: f32x8, b: f32x8) -> f32x8 {
    a + t * (b - a)
}

/// 3D Perlin for 8 consecutive X values with common (y, z).
#[inline(always)]
fn perlin3_x8(seed: u32, x: f32x8, y: f32, z: f32) -> f32x8 {
    let xf = x.floor();
    let xi: i32x8 = xf.trunc_int();
    let fx = x - xf;
    let (yf, zf) = (y.floor(), z.floor());
    let (yi, zi) = (yf as i32, zf as i32);
    let (fy, fz) = (y - yf, z - zf);
    let (v, w) = (fade(fy), fade(fz));
    let (u8_, v8, w8) = (fade8(fx), f32x8::splat(v), f32x8::splat(w));

    let (y1, z1) = (yi.wrapping_add(1), zi.wrapping_add(1));
    let yz = |yy: i32, zz: i32| (yy as u32).wrapping_mul(P2) ^ (zz as u32).wrapping_mul(P3);
    let xi1 = xi + i32x8::splat(1);
    let one = f32x8::splat(1.0);
    let (fxm, fym, fzm) = (fx - one, f32x8::splat(fy - 1.0), f32x8::splat(fz - 1.0));
    let (fy8, fz8) = (f32x8::splat(fy), f32x8::splat(fz));

    let n000 = grad8(hash8(seed, xi, yz(yi, zi)), fx, fy8, fz8);
    let n100 = grad8(hash8(seed, xi1, yz(yi, zi)), fxm, fy8, fz8);
    let n010 = grad8(hash8(seed, xi, yz(y1, zi)), fx, fym, fz8);
    let n110 = grad8(hash8(seed, xi1, yz(y1, zi)), fxm, fym, fz8);
    let n001 = grad8(hash8(seed, xi, yz(yi, z1)), fx, fy8, fzm);
    let n101 = grad8(hash8(seed, xi1, yz(yi, z1)), fxm, fy8, fzm);
    let n011 = grad8(hash8(seed, xi, yz(y1, z1)), fx, fym, fzm);
    let n111 = grad8(hash8(seed, xi1, yz(y1, z1)), fxm, fym, fzm);

    let r = lerp8(
        w8,
        lerp8(v8, lerp8(u8_, n000, n100), lerp8(u8_, n010, n110)),
        lerp8(v8, lerp8(u8_, n001, n101), lerp8(u8_, n011, n111)),
    );
    r.fast_max(f32x8::splat(-1.0)).fast_min(f32x8::splat(1.0))
}

// ---------------------------------------------------------------- Simplex 2D / 3D

const F2: f32 = 0.366_025_4; // (sqrt(3) - 1) / 2
const G2: f32 = 0.211_324_87; // (3 - sqrt(3)) / 6
const F3: f32 = 1.0 / 3.0;
const G3: f32 = 1.0 / 6.0;

#[inline(always)]
fn grad2(h: u32, x: f32, y: f32) -> f32 {
    match h & 7 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x,
        5 => -x,
        6 => y,
        _ => -y,
    }
}

pub fn simplex2(seed: u32, xin: f32, yin: f32) -> f32 {
    let s = (xin + yin) * F2;
    let (i, j) = ((xin + s).floor(), (yin + s).floor());
    let t = (i + j) * G2;
    let (x0, y0) = (xin - (i - t), yin - (j - t));
    let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
    let (x1, y1) = (x0 - i1 as f32 + G2, y0 - j1 as f32 + G2);
    let (x2, y2) = (x0 - 1.0 + 2.0 * G2, y0 - 1.0 + 2.0 * G2);
    let (ii, jj) = (i as i32, j as i32);

    let corner = |dx: f32, dy: f32, di: i32, dj: i32| -> f32 {
        let t = 0.5 - dx * dx - dy * dy;
        if t < 0.0 {
            0.0
        } else {
            let t2 = t * t;
            t2 * t2 * grad2(hash3(seed, ii.wrapping_add(di), jj.wrapping_add(dj), 0), dx, dy)
        }
    };
    let n = corner(x0, y0, 0, 0) + corner(x1, y1, i1, j1) + corner(x2, y2, 1, 1);
    (70.0 * n).clamp(-1.0, 1.0)
}

pub fn simplex3(seed: u32, xin: f32, yin: f32, zin: f32) -> f32 {
    let s = (xin + yin + zin) * F3;
    let (i, j, k) = ((xin + s).floor(), (yin + s).floor(), (zin + s).floor());
    let t = (i + j + k) * G3;
    let (x0, y0, z0) = (xin - (i - t), yin - (j - t), zin - (k - t));

    let (i1, j1, k1, i2, j2, k2) = if x0 >= y0 {
        if y0 >= z0 {
            (1, 0, 0, 1, 1, 0)
        } else if x0 >= z0 {
            (1, 0, 0, 1, 0, 1)
        } else {
            (0, 0, 1, 1, 0, 1)
        }
    } else if y0 < z0 {
        (0, 0, 1, 0, 1, 1)
    } else if x0 < z0 {
        (0, 1, 0, 0, 1, 1)
    } else {
        (0, 1, 0, 1, 1, 0)
    };

    let (ii, jj, kk) = (i as i32, j as i32, k as i32);
    let corner = |dx: f32, dy: f32, dz: f32, di: i32, dj: i32, dk: i32| -> f32 {
        let t = 0.6 - dx * dx - dy * dy - dz * dz;
        if t < 0.0 {
            0.0
        } else {
            let t2 = t * t;
            let h = hash3(seed, ii.wrapping_add(di), jj.wrapping_add(dj), kk.wrapping_add(dk));
            t2 * t2 * grad3(h, dx, dy, dz)
        }
    };
    let n = corner(x0, y0, z0, 0, 0, 0)
        + corner(x0 - i1 as f32 + G3, y0 - j1 as f32 + G3, z0 - k1 as f32 + G3, i1, j1, k1)
        + corner(
            x0 - i2 as f32 + 2.0 * G3,
            y0 - j2 as f32 + 2.0 * G3,
            z0 - k2 as f32 + 2.0 * G3,
            i2,
            j2,
            k2,
        )
        + corner(x0 - 1.0 + 3.0 * G3, y0 - 1.0 + 3.0 * G3, z0 - 1.0 + 3.0 * G3, 1, 1, 1);
    (32.0 * n).clamp(-1.0, 1.0)
}

// ---------------------------------------------------------------- Worley 3D (F1)

/// Distance to the nearest feature point (27 neighboring cells), clamped to [0, 1].
pub fn worley3(seed: u32, x: f32, y: f32, z: f32) -> f32 {
    let (xf, yf, zf) = (x.floor(), y.floor(), z.floor());
    let (xi, yi, zi) = (xf as i32, yf as i32, zf as i32);
    let (fx, fy, fz) = (x - xf, y - yf, z - zf);
    let mut best = f32::MAX;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let h = hash3(seed, xi.wrapping_add(dx), yi.wrapping_add(dy), zi.wrapping_add(dz));
                let px = (h & 0x3FF) as f32 * (1.0 / 1023.0);
                let py = ((h >> 10) & 0x3FF) as f32 * (1.0 / 1023.0);
                let pz = ((h >> 20) & 0x3FF) as f32 * (1.0 / 1023.0);
                let (ex, ey, ez) = (dx as f32 + px - fx, dy as f32 + py - fy, dz as f32 + pz - fz);
                best = best.min(ex * ex + ey * ey + ez * ez);
            }
        }
    }
    best.sqrt().min(1.0)
}

// ---------------------------------------------------------------- generic API

pub fn sample(kind: Kind, seed: u32, x: f32, y: f32, z: f32) -> f32 {
    match kind {
        Kind::Perlin3 => perlin3(seed, x, y, z),
        Kind::Simplex2 => simplex2(seed, x, y),
        Kind::Simplex3 => simplex3(seed, x, y, z),
        Kind::Worley3 => worley3(seed, x, y, z),
    }
}

fn fbm_norm(octaves: u32) -> f32 {
    let (mut sum, mut amp) = (0.0f32, 1.0f32);
    for _ in 0..octaves {
        sum += amp;
        amp *= 0.5;
    }
    1.0 / sum
}

/// fBm (lacunarity 2, gain 0.5), normalized by the sum of amplitudes.
pub fn fbm(kind: Kind, seed: u32, x: f32, y: f32, z: f32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut freq) = (0.0f32, 1.0f32, 1.0f32);
    for o in 0..octaves {
        let s = seed.wrapping_add(o.wrapping_mul(OCT_STEP));
        sum += sample(kind, s, x * freq, y * freq, z * freq) * amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum * fbm_norm(octaves)
}

fn perlin_row(seed: u32, out: &mut [f32], x0: f32, dx: f32, y: f32, z: f32, octaves: u32) {
    let n = out.len();
    let norm = f32x8::splat(fbm_norm(octaves));
    let lane = f32x8::from([0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    let mut i = 0;
    while i + 8 <= n {
        // same arithmetic as the scalar path: x0 + idx * dx
        let xs = f32x8::splat(x0) + (f32x8::splat(i as f32) + lane) * f32x8::splat(dx);
        let (mut sum, mut amp, mut freq) = (f32x8::splat(0.0), 1.0f32, 1.0f32);
        for o in 0..octaves {
            let s = seed.wrapping_add(o.wrapping_mul(OCT_STEP));
            sum = sum + perlin3_x8(s, xs * f32x8::splat(freq), y * freq, z * freq) * f32x8::splat(amp);
            amp *= 0.5;
            freq *= 2.0;
        }
        out[i..i + 8].copy_from_slice(&(sum * norm).to_array());
        i += 8;
    }
    for k in i..n {
        out[k] = fbm(Kind::Perlin3, seed, x0 + k as f32 * dx, y, z, octaves);
    }
}

/// Fills `out` (layout x + nx*(y + ny*z), X fastest) with fBm noise.
/// Parallelizes by rows on the dedicated pool; does not allocate heap memory.
pub fn fill_grid(
    kind: Kind,
    out: &mut [f32],
    nx: usize,
    ny: usize,
    origin: [f32; 3],
    step: [f32; 3],
    seed: u32,
    octaves: u32,
) {
    let octaves = octaves.clamp(1, 8);
    let min_rows = (4096 / nx.max(1)).max(1);
    pool::pool().install(|| {
        out.par_chunks_mut(nx)
            .enumerate()
            .with_min_len(min_rows)
            .for_each(|(row, chunk)| {
                let (y, z) = (row % ny, row / ny);
                let yy = origin[1] + y as f32 * step[1];
                let zz = origin[2] + z as f32 * step[2];
                if kind == Kind::Perlin3 {
                    perlin_row(seed, chunk, origin[0], step[0], yy, zz, octaves);
                } else {
                    for (x, slot) in chunk.iter_mut().enumerate() {
                        *slot = fbm(kind, seed, origin[0] + x as f32 * step[0], yy, zz, octaves);
                    }
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_in_range() {
        for kind in [Kind::Perlin3, Kind::Simplex2, Kind::Simplex3, Kind::Worley3] {
            for i in 0..2000 {
                let (x, y, z) = (i as f32 * 0.137 - 100.0, i as f32 * 0.071, i as f32 * -0.213);
                let a = sample(kind, 1234, x, y, z);
                assert_eq!(a, sample(kind, 1234, x, y, z));
                assert!(a.is_finite() && (-1.0..=1.0).contains(&a), "{kind:?} {a}");
            }
        }
    }

    #[test]
    fn different_seeds_differ() {
        let a = perlin3(1, 3.3, 4.4, 5.5);
        let b = perlin3(2, 3.3, 4.4, 5.5);
        assert_ne!(a, b);
    }

    #[test]
    fn perlin_is_zero_on_lattice() {
        assert_eq!(perlin3(7, 3.0, -2.0, 9.0), 0.0);
    }

    #[test]
    fn simd_row_matches_scalar() {
        for octaves in [1u32, 4] {
            let mut row = vec![0.0f32; 37]; // not a multiple of 8: exercises the scalar remainder
            perlin_row(99, &mut row, -5.3, 0.173, 2.9, -1.7, octaves);
            for (k, v) in row.iter().enumerate() {
                let s = fbm(Kind::Perlin3, 99, -5.3 + k as f32 * 0.173, 2.9, -1.7, octaves);
                assert!((v - s).abs() < 1e-4, "k={k} simd={v} scalar={s}");
            }
        }
    }

    #[test]
    fn fill_grid_matches_per_sample() {
        let (nx, ny, nz) = (20usize, 5usize, 4usize);
        let mut out = vec![0.0f32; nx * ny * nz];
        fill_grid(Kind::Simplex3, &mut out, nx, ny, [1.0, 2.0, 3.0], [0.1, 0.2, 0.3], 5, 3);
        let idx = |x, y, z| x + nx * (y + ny * z);
        let expect = fbm(Kind::Simplex3, 5, 1.0 + 7.0 * 0.1, 2.0 + 3.0 * 0.2, 3.0 + 2.0 * 0.3, 3);
        assert!((out[idx(7, 3, 2)] - expect).abs() < 1e-6);
    }
}
