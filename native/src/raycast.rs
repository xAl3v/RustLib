//! Raycast against lists of AABBs ("slab" method) and against a voxel grid
//! (Amanatides & Woo, "A Fast Voxel Traversal Algorithm for Ray Tracing").
//!
//! The result is serialized into a `RESULT_SIZE`-byte buffer supplied by the caller
//! (native byte-order layout), so no Java object is ever created from Rust:
//!
//! ```text
//!  off  type  field
//!    0  i32   hit        (0/1)
//!    4  i32   face       (-1 none, 0=-X 1=+X 2=-Y 3=+Y 4=-Z 5=+Z, 6=origin inside)
//!    8  i32   index      (AABB: box index; voxel: -1)
//!   12  i32   (reserved)
//!   16  f64   distance   (along the normalized direction)
//!   24  f64   x, 32 y, 40 z  (hit point)
//!   48  i32   voxelX, 52 voxelY, 56 voxelZ  (voxel world coordinates; voxels only)
//!   60  i32   (reserved)
//! ```

use crate::pool;
use rayon::prelude::*;

pub const RESULT_SIZE: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub hit: bool,
    pub face: i32,
    pub index: i32,
    pub t: f64,
    pub p: [f64; 3],
    pub voxel: [i32; 3],
}

impl Hit {
    pub fn miss() -> Hit {
        Hit { hit: false, face: -1, index: -1, t: 0.0, p: [0.0; 3], voxel: [0; 3] }
    }

    pub fn write_to(&self, out: &mut [u8]) {
        let mut b = [0u8; RESULT_SIZE];
        b[0..4].copy_from_slice(&(self.hit as i32).to_ne_bytes());
        b[4..8].copy_from_slice(&self.face.to_ne_bytes());
        b[8..12].copy_from_slice(&self.index.to_ne_bytes());
        b[16..24].copy_from_slice(&self.t.to_ne_bytes());
        for a in 0..3 {
            b[24 + a * 8..32 + a * 8].copy_from_slice(&self.p[a].to_ne_bytes());
            b[48 + a * 4..52 + a * 4].copy_from_slice(&self.voxel[a].to_ne_bytes());
        }
        out[..RESULT_SIZE].copy_from_slice(&b);
    }
}

/// Normalizes `d`. `None` if it is the zero vector or non-finite.
pub fn normalize(d: [f64; 3]) -> Option<[f64; 3]> {
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if !len.is_finite() || len == 0.0 {
        return None;
    }
    Some([d[0] / len, d[1] / len, d[2] / len])
}

/// Ray vs AABB. Returns (entry t, entry face). Origin inside => (0, 6).
pub fn ray_aabb(o: [f64; 3], d: [f64; 3], min: [f64; 3], max: [f64; 3]) -> Option<(f64, i32)> {
    let (mut tmin, mut tmax, mut face) = (f64::NEG_INFINITY, f64::INFINITY, -1);
    for a in 0..3 {
        if d[a] == 0.0 {
            // Ray parallel to the slab: outside => guaranteed miss (avoids 0 * inf = NaN)
            if o[a] < min[a] || o[a] > max[a] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d[a];
        let (mut t1, mut t2) = ((min[a] - o[a]) * inv, (max[a] - o[a]) * inv);
        let mut f = (a * 2) as i32; // enters through the min face (-axis)
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
            f += 1; // enters through the max face (+axis)
        }
        if t1 > tmin {
            tmin = t1;
            face = f;
        }
        if t2 < tmax {
            tmax = t2;
        }
        if tmin > tmax {
            return None;
        }
    }
    if tmax < 0.0 {
        return None;
    }
    if tmin < 0.0 {
        return Some((0.0, 6));
    }
    Some((tmin, face))
}

/// Nearest box hit by the ray (`d` must be normalized).
/// `boxes` = [minX, minY, minZ, maxX, maxY, maxZ] * n. Ties: lowest index.
pub fn raycast_aabbs(boxes: &[f64], o: [f64; 3], d: [f64; 3], max_dist: f64) -> Hit {
    let test = |(i, c): (usize, &[f64])| -> Option<(f64, usize, i32)> {
        let (t, f) = ray_aabb(o, d, [c[0], c[1], c[2]], [c[3], c[4], c[5]])?;
        (t <= max_dist).then_some((t, i, f))
    };
    let better = |a: (f64, usize, i32), b: (f64, usize, i32)| -> (f64, usize, i32) {
        if b.0 < a.0 || (b.0 == a.0 && b.1 < a.1) {
            b
        } else {
            a
        }
    };

    let best = if boxes.len() / 6 >= 2048 {
        pool::pool().install(|| {
            boxes.par_chunks_exact(6).enumerate().filter_map(test).reduce_with(better)
        })
    } else {
        boxes.chunks_exact(6).enumerate().filter_map(test).reduce(better)
    };

    match best {
        None => Hit::miss(),
        Some((t, i, f)) => Hit {
            hit: true,
            face: f,
            index: i as i32,
            t,
            p: [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t],
            voxel: [0; 3],
        },
    }
}

/// Voxel grid traversal (Amanatides-Woo).
/// - `grid`: sx*sy*sz bytes, index (y*sz + z)*sx + x; byte != 0 => solid.
/// - `g`: world coordinate of the min corner of voxel (0,0,0).
/// - `d` normalized; `max_dist` in world units.
pub fn raycast_voxels(
    grid: &[u8],
    dims: [i32; 3],
    g: [i32; 3],
    o: [f64; 3],
    d: [f64; 3],
    max_dist: f64,
) -> Hit {
    let lo = [o[0] - g[0] as f64, o[1] - g[1] as f64, o[2] - g[2] as f64];
    let dimf = [dims[0] as f64, dims[1] as f64, dims[2] as f64];

    // 1) Clip the ray against the grid's box (the origin may be outside).
    let Some((t0, entry_face)) = ray_aabb(lo, d, [0.0; 3], dimf) else {
        return Hit::miss();
    };
    if t0 > max_dist {
        return Hit::miss();
    }
    let p = [lo[0] + d[0] * t0, lo[1] + d[1] * t0, lo[2] + d[2] * t0];

    // 2) Amanatides-Woo initialization.
    let mut vox = [0i32; 3];
    let mut step = [0i32; 3];
    let mut t_max = [f64::INFINITY; 3];
    let mut t_delta = [f64::INFINITY; 3];
    for a in 0..3 {
        vox[a] = (p[a].floor() as i32).clamp(0, dims[a] - 1);
        if d[a] > 0.0 {
            step[a] = 1;
            t_delta[a] = 1.0 / d[a];
            t_max[a] = t0 + ((vox[a] + 1) as f64 - p[a]) / d[a];
        } else if d[a] < 0.0 {
            step[a] = -1;
            t_delta[a] = -1.0 / d[a];
            t_max[a] = t0 + (p[a] - vox[a] as f64) / -d[a];
        }
    }

    let mut face = if t0 > 0.0 { entry_face } else { 6 };
    let mut t = t0;
    loop {
        let idx = (vox[1] as usize * dims[2] as usize + vox[2] as usize) * dims[0] as usize + vox[0] as usize;
        if grid[idx] != 0 {
            return Hit {
                hit: true,
                face,
                index: -1,
                t,
                p: [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t],
                voxel: [vox[0] + g[0], vox[1] + g[1], vox[2] + g[2]],
            };
        }
        // Advance along the axis whose plane is crossed first.
        let a = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] {
            0
        } else if t_max[1] <= t_max[2] {
            1
        } else {
            2
        };
        if t_max[a] > max_dist {
            return Hit::miss();
        }
        t = t_max[a];
        vox[a] += step[a];
        if vox[a] < 0 || vox[a] >= dims[a] {
            return Hit::miss(); // left the grid
        }
        t_max[a] += t_delta[a];
        face = if step[a] > 0 { (a * 2) as i32 } else { (a * 2 + 1) as i32 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aabb_hit_nearest_and_face() {
        let boxes = [
            5.0, -1.0, -1.0, 6.0, 1.0, 1.0, // index 0 (far)
            2.0, -1.0, -1.0, 3.0, 1.0, 1.0, // index 1 (near)
        ];
        let h = raycast_aabbs(&boxes, [0.0; 3], [1.0, 0.0, 0.0], 100.0);
        assert!(h.hit && h.index == 1 && h.face == 0 && (h.t - 2.0).abs() < 1e-12);
        assert!((h.p[0] - 2.0).abs() < 1e-12);
    }

    #[test]
    fn aabb_miss_parallel_and_maxdist() {
        let boxes = [2.0, 5.0, -1.0, 3.0, 6.0, 1.0];
        assert!(!raycast_aabbs(&boxes, [0.0; 3], [1.0, 0.0, 0.0], 100.0).hit);
        let boxes = [2.0, -1.0, -1.0, 3.0, 1.0, 1.0];
        assert!(!raycast_aabbs(&boxes, [0.0; 3], [1.0, 0.0, 0.0], 1.0).hit);
    }

    #[test]
    fn aabb_origin_inside() {
        let boxes = [-1.0; 3].iter().chain([1.0; 3].iter()).copied().collect::<Vec<_>>();
        let h = raycast_aabbs(&boxes, [0.0; 3], [0.0, 1.0, 0.0], 10.0);
        assert!(h.hit && h.face == 6 && h.t == 0.0);
    }

    fn grid_with(dims: [usize; 3], solid: &[(usize, usize, usize)]) -> Vec<u8> {
        let mut g = vec![0u8; dims[0] * dims[1] * dims[2]];
        for &(x, y, z) in solid {
            g[(y * dims[2] + z) * dims[0] + x] = 1;
        }
        g
    }

    #[test]
    fn voxel_basic_hit_from_outside() {
        let g = grid_with([8, 8, 8], &[(5, 2, 2)]);
        // Origin outside the grid (world with offset 10), looking toward +X
        let h = raycast_voxels(&g, [8, 8, 8], [10, 0, 0], [2.0, 2.5, 2.5], [1.0, 0.0, 0.0], 100.0);
        assert!(h.hit);
        assert_eq!(h.voxel, [15, 2, 2]);
        assert_eq!(h.face, 0);
        assert!((h.t - (15.0 - 2.0)).abs() < 1e-9);
    }

    #[test]
    fn voxel_diagonal_and_miss() {
        let g = grid_with([8, 8, 8], &[(4, 4, 4)]);
        let d = normalize([1.0, 1.0, 1.0]).unwrap();
        let h = raycast_voxels(&g, [8, 8, 8], [0, 0, 0], [0.5, 0.5, 0.5], d, 100.0);
        assert!(h.hit && h.voxel == [4, 4, 4]);
        let h = raycast_voxels(&g, [8, 8, 8], [0, 0, 0], [0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 100.0);
        assert!(!h.hit);
        let h = raycast_voxels(&g, [8, 8, 8], [0, 0, 0], [0.5, 0.5, 0.5], d, 1.0);
        assert!(!h.hit);
    }

    #[test]
    fn voxel_starting_inside_solid() {
        let g = grid_with([4, 4, 4], &[(1, 1, 1)]);
        let h = raycast_voxels(&g, [4, 4, 4], [0, 0, 0], [1.5, 1.5, 1.5], [0.0, 1.0, 0.0], 10.0);
        assert!(h.hit && h.face == 6 && h.t == 0.0);
    }

    #[test]
    fn voxel_negative_direction() {
        let g = grid_with([8, 8, 8], &[(1, 3, 3)]);
        let h = raycast_voxels(&g, [8, 8, 8], [0, 0, 0], [7.5, 3.5, 3.5], [-1.0, 0.0, 0.0], 100.0);
        assert!(h.hit && h.voxel == [1, 3, 3] && h.face == 1);
    }
}
