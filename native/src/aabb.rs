//! Batched AABB collisions: one query box against N boxes.

/// Writes into `out` the indices of the boxes that overlap `q` (strict overlap:
/// faces that merely touch do NOT count). Returns the total number of overlaps, which may be
/// greater than `out.len()` (`snprintf` style): the caller detects truncation by comparing.
///
/// `boxes` = [minX, minY, minZ, maxX, maxY, maxZ] * n. Simple loop with no costly branches,
/// designed so that LLVM can auto-vectorize it.
pub fn overlaps(boxes: &[f64], q: [f64; 6], out: &mut [i32]) -> usize {
    let mut total = 0usize;
    for (i, c) in boxes.chunks_exact(6).enumerate() {
        let hit = (c[0] < q[3]) & (c[3] > q[0]) & (c[1] < q[4]) & (c[4] > q[1]) & (c[2] < q[5]) & (c[5] > q[2]);
        if hit {
            if total < out.len() {
                out[total] = i as i32;
            }
            total += 1;
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_strict_and_truncation() {
        let boxes = [
            0.0, 0.0, 0.0, 1.0, 1.0, 1.0, // 0: overlaps
            1.0, 0.0, 0.0, 2.0, 1.0, 1.0, // 1: only touches the face
            0.5, 0.5, 0.5, 3.0, 3.0, 3.0, // 2: overlaps
        ];
        let q = [0.5, 0.0, 0.0, 1.0, 1.0, 1.0];
        let mut out = [0i32; 1];
        let n = overlaps(&boxes, q, &mut out);
        assert_eq!(n, 2);
        assert_eq!(out[0], 0);
    }
}
