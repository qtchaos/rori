use crate::ProcessError;
use crate::nbt::{TimeResult, get_inhabited_time};
use crate::{Chunk, ChunkMetadata, REGION_SIZE, Region};
use libdeflater::{DecompressionError, Decompressor};
use log::warn;
use memmap2::MmapOptions;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use std::cell::RefCell;
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;

/// Region file sector size in bytes
const SECTOR_SIZE: usize = 4096;

const DECOMPRESS_CHUNK_SIZE: usize = 512;

thread_local! {
    /// Thread-local decompression buffer pool (reuse across chunks)
    static DECOMP_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(1_000_000));
    /// Thread-local libdeflater decompressor (reuse for better performance)
    static LIBDEFLATER: RefCell<Decompressor> = RefCell::new(Decompressor::new());
}

/// Compression types used in Minecraft region files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionType {
    /// `GZip` compression (type 1)
    GZip,
    /// `Zlib` compression (type 2)
    Zlib,
}

impl CompressionType {
    /// Parse compression type from a byte value.
    #[inline(always)]
    pub fn from_byte(byte: u8) -> Result<Self, ProcessError> {
        match byte {
            1 => Ok(Self::GZip),
            2 => Ok(Self::Zlib),
            _ => Err(ProcessError::ChunkError("Unknown compression type".into())),
        }
    }
}

/// Decompress chunk data partially using the specified compression type.
/// This is used for early NBT field extraction without full decompression.
///
/// # Arguments
/// * `compression` - The compression type to use
/// * `compressed_data` - The compressed chunk data
/// * `bite_size` - Maximum bytes to decompress
///
/// # Returns
/// The partially decompressed data on success.
#[inline]
pub fn decompress_partial(
    compression: CompressionType,
    compressed_data: &[u8],
    bite_size: usize,
) -> Result<Vec<u8>, ProcessError> {
    DECOMP_BUF.with(|buf_cell| {
        LIBDEFLATER.with(|decompressor_cell| {
            let mut buf = buf_cell.borrow_mut();
            let mut decompressor = decompressor_cell.borrow_mut();

            let current_cap = buf.capacity();
            if current_cap < bite_size {
                buf.reserve(bite_size - current_cap);
            }
            unsafe {
                buf.set_len(bite_size);
            }

            match compression {
                CompressionType::Zlib => {
                    match decompressor.zlib_decompress(compressed_data, &mut buf) {
                        Ok(size) => {
                            buf.truncate(size.min(bite_size));
                            Ok(buf.clone())
                        }
                        Err(DecompressionError::InsufficientSpace) => {
                            // We got as much as we could in bite_size
                            Ok(buf.clone())
                        }
                        Err(_) => Err(ProcessError::ChunkError("Zlib decompression failed".into())),
                    }
                }
                CompressionType::GZip => {
                    match decompressor.gzip_decompress(compressed_data, &mut buf) {
                        Ok(size) => {
                            buf.truncate(size.min(bite_size));
                            Ok(buf.clone())
                        }
                        Err(DecompressionError::InsufficientSpace) => {
                            // We got as much as we could in bite_size
                            Ok(buf.clone())
                        }
                        Err(_) => Err(ProcessError::ChunkError("GZip decompression failed".into())),
                    }
                }
            }
        })
    })
}

/// Decompress chunk data fully using thread-local buffer
///
/// # Errors
/// Returns `Err(ProcessError::ChunkError)` when decompression fails (e.g. `GZip` or `Zlib` read errors),
/// or `Err(ProcessError::IoError)` when underlying I/O errors occur while reading buffers.
#[inline]
pub fn decompress(
    compression: CompressionType,
    compressed_data: &[u8],
) -> Result<Vec<u8>, ProcessError> {
    DECOMP_BUF.with(|buf_cell| {
        LIBDEFLATER.with(|decompressor_cell| {
            let mut buf = buf_cell.borrow_mut();
            let mut decompressor = decompressor_cell.borrow_mut();

            // Estimate decompressed size
            let estimated_size = compressed_data.len() * 10;
            let current_cap = buf.capacity();
            if current_cap < estimated_size {
                buf.reserve(estimated_size - current_cap);
            }
            unsafe {
                buf.set_len(estimated_size);
            }

            match compression {
                CompressionType::Zlib => loop {
                    match decompressor.zlib_decompress(compressed_data, &mut buf) {
                        Ok(size) => {
                            buf.truncate(size);
                            break Ok(buf.clone());
                        }
                        Err(DecompressionError::InsufficientSpace) => {
                            let new_size = buf.len() * 2;
                            let current_cap = buf.capacity();
                            if current_cap < new_size {
                                buf.reserve(new_size - current_cap);
                            }
                            unsafe {
                                buf.set_len(new_size);
                            }
                        }
                        Err(DecompressionError::BadData) => {
                            break Err(ProcessError::ChunkError("Bad data".into()));
                        }
                    }
                },
                CompressionType::GZip => {
                    loop {
                        match decompressor.gzip_decompress(compressed_data, &mut buf) {
                            Ok(size) => {
                                buf.truncate(size);
                                break Ok(buf.clone());
                            }
                            Err(DecompressionError::InsufficientSpace) => {
                                // Double buffer size and try again
                                let new_size = buf.len() * 2;
                                buf.resize(new_size, 0);
                            }
                            Err(_) => {
                                break Err(ProcessError::ChunkError(
                                    "GZip decompression failed".into(),
                                ));
                            }
                        }
                    }
                }
            }
        })
    })
}

/// Process the region file using Memory Mapping and Parallel streams.
/// Returns None if mmap fails or file validation fails.
pub fn mmap_region(
    region_path: &PathBuf,
    min_inhabited_ticks: u32,
) -> Result<Region, ProcessError> {
    let file = fs::File::open(region_path).map_err(|_| ProcessError::NoFilesFound)?;

    // handling mmap requires care regarding file truncation/SIGBUS
    let mmap = unsafe {
        let mut opts = MmapOptions::new();
        #[cfg(target_os = "linux")]
        opts.populate();
        opts.map(&file).map_err(ProcessError::IoError)?
    };

    let raw_data: &[u8] = &mmap[..];

    // Validate header size (4KB offsets + 4KB timestamps)
    if raw_data.len() < 8192 || raw_data.is_empty() {
        warn!("Mmap region too small: {}", region_path.display());
        return Err(ProcessError::InvalidRegionSize);
    }

    let chunks = parse_header(raw_data);
    let results: Vec<(usize, usize, Option<Vec<u8>>, TimeResult, CompressionType)> = chunks
        .into_par_iter()
        .map(
            #[allow(clippy::single_match_else)]
            |info| match process_chunk(raw_data, &info, DECOMPRESS_CHUNK_SIZE) {
                Some((compressed, time_res)) => {
                    (info.x, info.z, compressed, time_res, info.compression)
                }
                None => {
                    warn!(
                        "Error processing chunk {:?}, chunk size: {}",
                        info,
                        info.end - info.start
                    );
                    (
                        info.x,
                        info.z,
                        None,
                        TimeResult::default(),
                        CompressionType::Zlib,
                    )
                }
            },
        )
        .collect();

    let mut regions = Region::new();
    regions.stats.chunks.total = u32::try_from(results.len()).unwrap_or(u32::MAX);
    let mut inhabited_chunks = 0;

    // Set compression type from first valid chunk
    if let Some((_, _, _, _, first_compression)) = results.first() {
        regions.compression = *first_compression;
    }

    for (x, z, compressed, time_res, _compression) in results {
        let inhabited_time = time_res
            .time
            .unwrap_or_else(|| i64::from(min_inhabited_ticks) + 1);

        regions.chunks[x][z] = Some(Chunk {
            data: compressed,
            inhabited_time,
        });

        // Increment for stats later on...
        if inhabited_time > i64::from(min_inhabited_ticks) {
            inhabited_chunks += 1;
        }
    }

    regions.stats.chunks.inhabited = inhabited_chunks;

    Ok(regions)
}

/// Processing using `fastanvil` / standard IO.
/// Slower than `mmap_region`
pub fn anvil_region(region_path: &PathBuf) -> Result<Region, ProcessError> {
    let file = fs::File::open(region_path)?;
    let mut mca = fastanvil::Region::from_stream(BufReader::new(file))
        .map_err(|_| ProcessError::RegionError("Failed to create region".into()))?;

    let mut regions = Region::new();

    for x in 0..REGION_SIZE {
        for z in 0..REGION_SIZE {
            if let Ok(Some(chunk_data)) = mca.read_chunk(x, z)
                && let Ok(time_result) = get_inhabited_time(&chunk_data)
            {
                let Some(inhabited_time) = time_result.time else {
                    return Err(ProcessError::RegionError("Chunk time is None".into()));
                };

                regions.chunks[x][z] = Some(Chunk {
                    data: None, // fastanvil doesn't provide compressed data
                    inhabited_time,
                });
            }
        }
    }
    Ok(regions)
}

/// Decompress and parse a single chunk from raw bytes.
fn process_chunk(
    raw_region: &[u8],
    meta: &ChunkMetadata,
    bite_size: usize,
) -> Option<(Option<Vec<u8>>, TimeResult)> {
    let compressed_data = &raw_region[meta.start..meta.end];
    if bite_size > 0
        && let Ok(decompressed) = decompress_partial(meta.compression, compressed_data, bite_size)
        && let Ok(stats) = get_inhabited_time(&decompressed)
    {
        return Some((Some(compressed_data.to_vec()), stats));
    }

    // If get_inhabited_time failed (Err), we fall through

    let decompressed = decompress(meta.compression, compressed_data).ok()?;
    let stats = get_inhabited_time(&decompressed).ok()?;

    Some((Some(compressed_data.to_vec()), stats))
}

/// Helper to parse the 4KB header and extract valid chunk metadata.
fn parse_header(data: &[u8]) -> Vec<ChunkMetadata> {
    let mut chunks = Vec::with_capacity(256);

    for x in 0..REGION_SIZE {
        for z in 0..REGION_SIZE {
            let offset_idx = (x + z * REGION_SIZE) * 4;
            if offset_idx + 4 > data.len() {
                continue;
            }

            let loc_bytes = &data[offset_idx..offset_idx + 4];
            let raw = u32::from_be_bytes([loc_bytes[0], loc_bytes[1], loc_bytes[2], loc_bytes[3]]);

            let sector = (raw >> 8) as usize;
            let length_sectors = (raw & 0xFF) as usize;

            if sector == 0 || length_sectors == 0 {
                continue;
            }

            let byte_offset = sector * SECTOR_SIZE;
            if byte_offset + 4 > data.len() {
                continue;
            }

            let len_bytes = &data[byte_offset..byte_offset + 4];
            let exact_len =
                u32::from_be_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]])
                    as usize;

            if exact_len == 0 || byte_offset + 4 + exact_len > data.len() {
                continue;
            }

            let compression = match CompressionType::from_byte(data[byte_offset + 4]) {
                Ok(ct) => ct,
                Err(err) => {
                    log::error!("Failed to parse compression type: {err}");
                    continue;
                }
            };

            let data_start = byte_offset + 5;
            let data_end = byte_offset + 4 + exact_len;

            if data_end <= data.len() && data_start <= data_end {
                chunks.push(ChunkMetadata {
                    x,
                    z,
                    compression,
                    start: data_start,
                    end: data_end,
                });
            }
        }
    }
    chunks
}
