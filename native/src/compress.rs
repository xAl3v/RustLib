//! Binary block compression / decompression: LZ4 (always) and Zstd (`zstd` feature).
//! Operates directly on DirectByteBuffer slices: zero intermediate copies.

use crate::util::{ERR_BUFFER, ERR_DATA};

pub fn lz4_max_size(len: usize) -> usize {
    lz4_flex::block::get_maximum_output_size(len)
}

pub fn lz4_compress(src: &[u8], dst: &mut [u8]) -> Result<usize, i32> {
    lz4_flex::block::compress_into(src, dst).map_err(|_| ERR_BUFFER)
}

pub fn lz4_decompress(src: &[u8], dst: &mut [u8]) -> Result<usize, i32> {
    lz4_flex::block::decompress_into(src, dst).map_err(|e| match e {
        lz4_flex::block::DecompressError::OutputTooSmall { .. } => ERR_BUFFER,
        _ => ERR_DATA,
    })
}

#[cfg(feature = "zstd")]
pub fn zstd_compress(src: &[u8], dst: &mut [u8], level: i32) -> Result<usize, i32> {
    zstd::bulk::compress_to_buffer(src, dst, level.clamp(1, 22)).map_err(|_| ERR_BUFFER)
}

#[cfg(feature = "zstd")]
pub fn zstd_decompress(src: &[u8], dst: &mut [u8]) -> Result<usize, i32> {
    zstd::bulk::decompress_to_buffer(src, dst).map_err(|_| ERR_DATA)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lz4_roundtrip() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let mut comp = vec![0u8; lz4_max_size(data.len())];
        let n = lz4_compress(&data, &mut comp).unwrap();
        assert!(n < data.len());
        let mut back = vec![0u8; data.len()];
        let m = lz4_decompress(&comp[..n], &mut back).unwrap();
        assert_eq!(&back[..m], &data[..]);
    }

    #[test]
    fn lz4_errors() {
        let data = vec![7u8; 1000];
        let mut tiny = [0u8; 4];
        assert_eq!(lz4_compress(&data, &mut tiny), Err(ERR_BUFFER));
        let mut out = [0u8; 16];
        assert!(lz4_decompress(&[0xFF, 0xFF, 0xFF], &mut out).is_err());
    }

    #[cfg(feature = "zstd")]
    #[test]
    fn zstd_roundtrip() {
        let data: Vec<u8> = (0..50_000u32).map(|i| (i % 97) as u8).collect();
        let mut comp = vec![0u8; data.len() + 1024];
        let n = zstd_compress(&data, &mut comp, 3).unwrap();
        let mut back = vec![0u8; data.len()];
        let m = zstd_decompress(&comp[..n], &mut back).unwrap();
        assert_eq!(&back[..m], &data[..]);
    }
}
