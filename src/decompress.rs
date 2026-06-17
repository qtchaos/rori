use crate::ProcessError;
use crate::nbt::{TimeResult, find_inhabited_time_fast, get_inhabited_time};
use crate::timing::StageTimings;
use crate::{Chunk, ChunkMetadata, REGION_SIZE, Region};
use flate2::{Decompress as StreamDecompressor, FlushDecompress, Status};
use libdeflater::{DecompressionError, Decompressor};
use log::warn;
use memmap2::{Mmap, MmapOptions};
use rayon::iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::cell::RefCell;
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Region file sector size in bytes
const SECTOR_SIZE: usize = 4096;

const DECOMPRESS_CHUNK_SIZE: usize = 512;
const STREAM_SCAN_CHUNK_SIZE: usize = 16 * 1024;
// Nearly all modern chunk NBTs put InhabitedTime before this; rare outliers fall back to full inflate.
const STREAM_SCAN_LIMIT: usize = 64 * 1024;
// Covers the tag bytes plus its i64 value when a match crosses a stream block boundary.
const STREAM_SCAN_OVERLAP: usize = 32;
// Chunks above this size almost always miss the 512-byte probe in the real-world corpus.
const PARTIAL_COMPRESSED_SIZE_LIMIT: usize = 12 * 1024;

thread_local! {
    /// Thread-local decompression buffer pool (reuse across chunks)
    static DECOMP_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(1_000_000));
    /// Thread-local libdeflater decompressor (reuse for better performance)
    static LIBDEFLATER: RefCell<Decompressor> = RefCell::new(Decompressor::new());
    /// Thread-local streaming zlib decompressor used to stop once the needed NBT tag appears.
    static STREAM_ZLIB: RefCell<StreamDecompressor> = RefCell::new(StreamDecompressor::new(true));
    /// Thread-local raw-deflate decompressor for GZip streaming (gzip header is skipped manually).
    static STREAM_DEFLATE: RefCell<StreamDecompressor> = RefCell::new(StreamDecompressor::new(false));
    /// Thread-local buffer for streaming scan output + NBT search (reuse across chunks).
    static STREAM_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(STREAM_SCAN_LIMIT + STREAM_SCAN_OVERLAP));
}

pub(crate) struct MappedRegion {
    pub(crate) region: Region,
    pub(crate) mmap: Mmap,
    pub(crate) chunks: Vec<ScannedChunk>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScannedChunk {
    pub(crate) metadata: ChunkMetadata,
    pub(crate) inhabited_time: i64,
}

#[derive(Debug)]
struct ChunkScanResult {
    metadata: ChunkMetadata,
    compressed: Option<Vec<u8>>,
    inhabited_time: i64,
    timings: StageTimings,
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
    let mut elapsed = Duration::default();
    decompress_partial_with(
        compression,
        compressed_data,
        bite_size,
        false,
        &mut elapsed,
        <[u8]>::to_vec,
    )
}

#[inline]
fn decompress_partial_with<R>(
    compression: CompressionType,
    compressed_data: &[u8],
    bite_size: usize,
    timings_enabled: bool,
    elapsed: &mut Duration,
    mut on_decompressed: impl FnMut(&[u8]) -> R,
) -> Result<R, ProcessError> {
    DECOMP_BUF.with(|buf_cell| {
        LIBDEFLATER.with(|decompressor_cell| {
            let mut buf = buf_cell.borrow_mut();
            let mut decompressor = decompressor_cell.borrow_mut();

            buf.resize(bite_size, 0);
            let start = timings_enabled.then(Instant::now);

            let result = match compression {
                CompressionType::Zlib => {
                    match decompressor.zlib_decompress(compressed_data, &mut buf) {
                        Ok(size) => Ok(size.min(bite_size)),
                        Err(DecompressionError::InsufficientSpace) => Ok(bite_size),
                        Err(_) => Err(ProcessError::ChunkError("Zlib decompression failed".into())),
                    }
                }
                CompressionType::GZip => {
                    match decompressor.gzip_decompress(compressed_data, &mut buf) {
                        Ok(size) => Ok(size.min(bite_size)),
                        Err(DecompressionError::InsufficientSpace) => Ok(bite_size),
                        Err(_) => Err(ProcessError::ChunkError("GZip decompression failed".into())),
                    }
                }
            };

            if let Some(start) = start {
                *elapsed += start.elapsed();
            }

            let size = result?;
            Ok(on_decompressed(&buf[..size]))
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
    let mut elapsed = Duration::default();
    decompress_with(
        compression,
        compressed_data,
        false,
        &mut elapsed,
        <[u8]>::to_vec,
    )
}

#[inline]
fn decompress_with<R>(
    compression: CompressionType,
    compressed_data: &[u8],
    timings_enabled: bool,
    elapsed: &mut Duration,
    mut on_decompressed: impl FnMut(&[u8]) -> R,
) -> Result<R, ProcessError> {
    DECOMP_BUF.with(|buf_cell| {
        LIBDEFLATER.with(|decompressor_cell| {
            let mut buf = buf_cell.borrow_mut();
            let mut decompressor = decompressor_cell.borrow_mut();

            // Estimate decompressed size
            let estimated_size = compressed_data.len().saturating_mul(10).max(1);
            buf.resize(estimated_size, 0);
            let start = timings_enabled.then(Instant::now);

            match compression {
                CompressionType::Zlib => loop {
                    match decompressor.zlib_decompress(compressed_data, &mut buf) {
                        Ok(size) => {
                            if let Some(start) = start {
                                *elapsed += start.elapsed();
                            }
                            break Ok(on_decompressed(&buf[..size]));
                        }
                        Err(DecompressionError::InsufficientSpace) => {
                            let new_size = buf.len().saturating_mul(2);
                            if new_size == buf.len() {
                                if let Some(start) = start {
                                    *elapsed += start.elapsed();
                                }
                                break Err(ProcessError::ChunkError(
                                    "Zlib decompression output too large".into(),
                                ));
                            }
                            buf.resize(new_size, 0);
                        }
                        Err(DecompressionError::BadData) => {
                            if let Some(start) = start {
                                *elapsed += start.elapsed();
                            }
                            break Err(ProcessError::ChunkError("Bad data".into()));
                        }
                    }
                },
                CompressionType::GZip => {
                    loop {
                        match decompressor.gzip_decompress(compressed_data, &mut buf) {
                            Ok(size) => {
                                if let Some(start) = start {
                                    *elapsed += start.elapsed();
                                }
                                break Ok(on_decompressed(&buf[..size]));
                            }
                            Err(DecompressionError::InsufficientSpace) => {
                                // Double buffer size and try again
                                let new_size = buf.len().saturating_mul(2);
                                if new_size == buf.len() {
                                    if let Some(start) = start {
                                        *elapsed += start.elapsed();
                                    }
                                    break Err(ProcessError::ChunkError(
                                        "GZip decompression output too large".into(),
                                    ));
                                }
                                buf.resize(new_size, 0);
                            }
                            Err(_) => {
                                if let Some(start) = start {
                                    *elapsed += start.elapsed();
                                }
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
    Ok(mmap_region_inner(region_path, min_inhabited_ticks, true, true)?.region)
}

pub(crate) fn mmap_region_for_processing(
    region_path: &PathBuf,
    min_inhabited_ticks: u32,
) -> Result<MappedRegion, ProcessError> {
    mmap_region_inner(region_path, min_inhabited_ticks, false, false)
}

fn mmap_region_inner(
    region_path: &PathBuf,
    min_inhabited_ticks: u32,
    build_chunk_grid: bool,
    copy_compressed: bool,
) -> Result<MappedRegion, ProcessError> {
    let timings_enabled = log::log_enabled!(log::Level::Debug);
    let mut timings = StageTimings::default();
    let mmap_start = timings_enabled.then(Instant::now);
    let file = fs::File::open(region_path).map_err(|_| ProcessError::NoFilesFound)?;

    // handling mmap requires care regarding file truncation/SIGBUS
    let mmap = unsafe {
        MmapOptions::new()
            .map(&file)
            .map_err(ProcessError::IoError)?
    };
    if let Some(start) = mmap_start {
        timings.mmap += start.elapsed();
    }

    let raw_data: &[u8] = &mmap[..];

    // Validate header size (4KB offsets + 4KB timestamps)
    if raw_data.len() < 8192 || raw_data.is_empty() {
        warn!("Mmap region too small: {}", region_path.display());
        return Err(ProcessError::InvalidRegionSize);
    }

    let header_start = timings_enabled.then(Instant::now);
    let chunks = parse_header(raw_data);
    if let Some(start) = header_start {
        timings.parse_header += start.elapsed();
    }

    let scan_start = timings_enabled.then(Instant::now);
    let results = scan_chunks(
        raw_data,
        &chunks,
        min_inhabited_ticks,
        copy_compressed,
        timings_enabled,
    );
    if let Some(start) = scan_start {
        timings.scan_chunks += start.elapsed();
    }

    for result in &results {
        timings.merge(result.timings);
    }

    let assemble_start = timings_enabled.then(Instant::now);
    let mut regions = Region::new();
    regions.stats.chunks.total = u32::try_from(results.len()).unwrap_or(u32::MAX);
    regions.stats.chunks.inhabited = u32::try_from(
        results
            .iter()
            .filter(|result| result.inhabited_time > i64::from(min_inhabited_ticks))
            .count(),
    )
    .unwrap_or(u32::MAX);
    let mut scanned_chunks = Vec::with_capacity(results.len());

    // Set compression type from first valid chunk
    if let Some(first_chunk) = results.first() {
        regions.compression = first_chunk.metadata.compression;
    }

    for result in results {
        scanned_chunks.push(ScannedChunk {
            metadata: result.metadata,
            inhabited_time: result.inhabited_time,
        });

        if build_chunk_grid {
            regions.chunks[result.metadata.x][result.metadata.z] = Some(Chunk {
                data: result.compressed,
                inhabited_time: result.inhabited_time,
            });
        }
    }

    if let Some(start) = assemble_start {
        timings.assemble_region += start.elapsed();
    }
    regions.stats.timings = timings;

    Ok(MappedRegion {
        region: regions,
        mmap,
        chunks: scanned_chunks,
    })
}

/// Processing using `fastanvil` / standard IO.
/// Slower than `mmap_region`
pub fn anvil_region(region_path: &PathBuf) -> Result<Region, ProcessError> {
    let timings_enabled = log::log_enabled!(log::Level::Debug);
    let anvil_start = timings_enabled.then(Instant::now);
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
    if let Some(start) = anvil_start {
        regions.stats.timings.anvil_fallback = start.elapsed();
    }
    Ok(regions)
}

fn scan_chunks(
    raw_region: &[u8],
    chunks: &[ChunkMetadata],
    min_inhabited_ticks: u32,
    copy_compressed: bool,
    timings_enabled: bool,
) -> Vec<ChunkScanResult> {
    if rayon::current_thread_index().is_some() {
        chunks
            .iter()
            .map(|info| {
                scan_chunk(
                    raw_region,
                    info,
                    min_inhabited_ticks,
                    copy_compressed,
                    timings_enabled,
                )
            })
            .collect()
    } else {
        chunks
            .par_iter()
            .map(|info| {
                scan_chunk(
                    raw_region,
                    info,
                    min_inhabited_ticks,
                    copy_compressed,
                    timings_enabled,
                )
            })
            .collect()
    }
}

fn scan_chunk(
    raw_region: &[u8],
    meta: &ChunkMetadata,
    min_inhabited_ticks: u32,
    copy_compressed: bool,
    timings_enabled: bool,
) -> ChunkScanResult {
    let (time_res, mut timings) =
        process_chunk_time(raw_region, meta, DECOMPRESS_CHUNK_SIZE, timings_enabled).unwrap_or_else(|| {
                let timings = StageTimings {
                    scan_failures: 1,
                    ..StageTimings::default()
                };
                warn!(
                    "Error processing chunk {:?}, chunk size: {}",
                    meta,
                    meta.end - meta.start
                );
                (TimeResult::default(), timings)
            });
    let inhabited_time = time_res
        .time
        .unwrap_or_else(|| i64::from(min_inhabited_ticks).saturating_add(1));

    let compressed = if copy_compressed {
        let copy_start = timings_enabled.then(Instant::now);
        let data = raw_region[meta.start..meta.end].to_vec();
        if let Some(start) = copy_start {
            timings.payload_copy += start.elapsed();
        }
        Some(data)
    } else {
        None
    };

    ChunkScanResult {
        metadata: *meta,
        compressed,
        inhabited_time,
        timings,
    }
}

/// Decompress and parse a single chunk from raw bytes.
fn process_chunk_time(
    raw_region: &[u8],
    meta: &ChunkMetadata,
    bite_size: usize,
    timings_enabled: bool,
) -> Option<(TimeResult, StageTimings)> {
    let mut timings = StageTimings::default();
    let compressed_data = &raw_region[meta.start..meta.end];
    // ponytail: inlined should_try_partial_probe, add const fn back if multiple callers appear
    let try_partial = bite_size > 0
        && (!matches!(meta.compression, CompressionType::Zlib)
            || compressed_data.len() <= PARTIAL_COMPRESSED_SIZE_LIMIT);

    if try_partial {
        let mut partial_decompress = Duration::default();
        let stats = decompress_partial_with(
            meta.compression,
            compressed_data,
            bite_size,
            timings_enabled,
            &mut partial_decompress,
            |decompressed| {
                let nbt_start = timings_enabled.then(Instant::now);
                let stats = find_inhabited_time_fast(decompressed)
                    .map_or_else(|| get_inhabited_time(decompressed), Ok);
                if let Some(start) = nbt_start {
                    timings.partial_nbt += start.elapsed();
                }
                stats
            },
        );
        timings.partial_decompress += partial_decompress;

        if let Ok(Ok(stats)) = stats {
            timings.partial_hits += 1;
            return Some((stats, timings));
        }
    } else if bite_size > 0 {
        timings.partial_skips += 1;
    }

    // If get_inhabited_time failed (Err), we fall through

    match stream_find_inhabited_time(meta.compression, compressed_data, timings_enabled, &mut timings) {
        Ok(Some(stats)) => {
            timings.stream_hits += 1;
            return Some((stats, timings));
        }
        Ok(None) | Err(_) => {
            timings.stream_misses += 1;
        }
    }

    timings.full_fallbacks += 1;
    let mut full_decompress = Duration::default();
    let stats = decompress_with(
        meta.compression,
        compressed_data,
        timings_enabled,
        &mut full_decompress,
        |decompressed| {
            let nbt_start = timings_enabled.then(Instant::now);
            let stats = find_inhabited_time_fast(decompressed)
                .map_or_else(|| get_inhabited_time(decompressed), Ok);
            if let Some(start) = nbt_start {
                timings.full_nbt += start.elapsed();
            }
            stats
        },
    )
    .ok()?
    .ok()?;
    timings.full_decompress += full_decompress;
    timings.full_hits += 1;

    Some((stats, timings))
}

fn stream_find_inhabited_time(
    compression: CompressionType,
    compressed_data: &[u8],
    timings_enabled: bool,
    timings: &mut StageTimings,
) -> Result<Option<TimeResult>, ProcessError> {
    match compression {
        CompressionType::Zlib => stream_decompress_inner(
            compressed_data,
            0,
            true,
            timings_enabled,
            timings,
        ),
        CompressionType::GZip => {
            let offset = gzip_body_offset(compressed_data)?;
            stream_decompress_inner(
                compressed_data,
                offset,
                false,
                timings_enabled,
                timings,
            )
        }
    }
}

fn stream_decompress_inner(
    compressed_data: &[u8],
    input_offset: usize,
    is_zlib: bool,
    timings_enabled: bool,
    timings: &mut StageTimings,
) -> Result<Option<TimeResult>, ProcessError> {
    let stream_cell = if is_zlib { &STREAM_ZLIB } else { &STREAM_DEFLATE };

    stream_cell.with(|stream_cell| {
        STREAM_BUF.with(|buf_cell| {
            let mut stream = stream_cell.borrow_mut();
            let mut buf = buf_cell.borrow_mut();

            stream.reset(is_zlib);
            buf.resize(STREAM_SCAN_LIMIT + STREAM_SCAN_OVERLAP, 0);
            let out = &mut buf[..];
            let mut written = 0usize;
            let mut first = true;

            loop {
                let total_out = written;

                if total_out >= STREAM_SCAN_LIMIT {
                    return Ok(None);
                }

                let remaining_limit = STREAM_SCAN_LIMIT - total_out;
                let output_len = remaining_limit.min(STREAM_SCAN_CHUNK_SIZE);
                let current_offset = if first {
                    first = false;
                    input_offset
                } else {
                    usize::try_from(stream.total_in())
                        .map_err(|_| ProcessError::ChunkError("stream total_in overflow".into()))?
                };

                if current_offset > compressed_data.len() {
                    return Ok(None);
                }

                let before_out = stream.total_out();
                let decompress_start = timings_enabled.then(Instant::now);
                let status = stream
                    .decompress(
                        &compressed_data[current_offset..],
                        &mut out[written..written + output_len],
                        FlushDecompress::None,
                    )
                    .map_err(|_| {
                        ProcessError::ChunkError("Streaming decompression failed".into())
                    })?;
                if let Some(start) = decompress_start {
                    timings.stream_decompress += start.elapsed();
                }

                let produced = usize::try_from(stream.total_out() - before_out)
                    .map_err(|_| ProcessError::ChunkError("stream output overflow".into()))?;
                let produced_bytes = u64::try_from(produced)
                    .map_err(|_| ProcessError::ChunkError("stream byte count overflow".into()))?;
                timings.stream_output_bytes =
                    timings.stream_output_bytes.saturating_add(produced_bytes);
                written += produced;

                if produced > 0 {
                    let nbt_start = timings_enabled.then(Instant::now);
                    let search_end = written;
                    let found_time = find_inhabited_time_fast(&out[..search_end]);
                    if let Some(start) = nbt_start {
                        timings.stream_nbt += start.elapsed();
                    }

                    if found_time.is_some() {
                        return Ok(found_time);
                    }
                }

                match status {
                    Status::StreamEnd => return Ok(None),
                    Status::Ok | Status::BufError => {
                        if produced == 0 {
                            return Ok(None);
                        }
                    }
                }
            }
        })
    })
}

/// Skip the `GZip` header and return the byte offset where raw deflate data begins.
/// `GZip` header: 2B magic | 1B method | 1B flags | 4B mtime | 1B extra-flags | 1B OS | [optional fields]
fn gzip_body_offset(data: &[u8]) -> Result<usize, ProcessError> {
    if data.len() < 10 || data[0] != 0x1F || data[1] != 0x8B {
        return Err(ProcessError::ChunkError("Invalid gzip magic".into()));
    }
    let flags = data[3];
    let mut offset = 10usize;
    // Optional: extra field (FEXTRA)
    if flags & 0x04 != 0 {
        if offset + 2 > data.len() {
            return Err(ProcessError::ChunkError("Truncated gzip FEXTRA length".into()));
        }
        let xlen = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        offset += 2 + xlen;
    }
    // Optional: original filename (FNAME)
    if flags & 0x08 != 0 {
        while offset < data.len() && data[offset] != 0 {
            offset += 1;
        }
        offset += 1; // skip the null terminator
    }
    // Optional: comment (FCOMMENT)
    if flags & 0x10 != 0 {
        while offset < data.len() && data[offset] != 0 {
            offset += 1;
        }
        offset += 1;
    }
    // Optional: header CRC (FHCRC)
    if flags & 0x02 != 0 {
        offset += 2;
    }
    if offset >= data.len() {
        return Err(ProcessError::ChunkError("GZip header extends past data".into()));
    }
    Ok(offset)
}

/// Helper to parse the 4KB header and extract valid chunk metadata.
fn parse_header(data: &[u8]) -> Vec<ChunkMetadata> {
    // Collect all (x,z) slot indices first so we can switch between
    // serial and parallel iteration without reallocating per slot.
    let slots: Vec<(usize, usize)> = (0..REGION_SIZE)
        .flat_map(|z| (0..REGION_SIZE).map(move |x| (x, z)))
        .collect();

    if rayon::current_thread_index().is_some() {
        slots
            .into_iter()
            .filter_map(|(x, z)| parse_header_slot(data, x, z))
            .collect()
    } else {
        slots
            .into_par_iter()
            .filter_map(|(x, z)| parse_header_slot(data, x, z))
            .collect()
    }
}

#[inline]
fn parse_header_slot(data: &[u8], x: usize, z: usize) -> Option<ChunkMetadata> {
    let offset_idx = (x + z * REGION_SIZE) * 4;
    if offset_idx + 4 > data.len() {
        return None;
    }

    let raw = u32::from_be(unsafe {
        std::ptr::read_unaligned(data.as_ptr().add(offset_idx).cast::<u32>())
    });

    let sector = (raw >> 8) as usize;
    let length_sectors = (raw & 0xFF) as usize;

    if sector == 0 || length_sectors == 0 {
        return None;
    }

    let byte_offset = sector * SECTOR_SIZE;
    if byte_offset > data.len().saturating_sub(4) {
        return None;
    }

    let len_bytes = &data[byte_offset..byte_offset + 4];
    let exact_len =
        u32::from_be_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]])
            as usize;

    if exact_len == 0 || byte_offset + 4 + exact_len > data.len() {
        return None;
    }

    let compression = match CompressionType::from_byte(data[byte_offset + 4]) {
        Ok(ct) => ct,
        Err(err) => {
            log::error!("Failed to parse compression type: {err}");
            return None;
        }
    };

    let data_start = byte_offset + 5;
    let data_end = byte_offset + 4 + exact_len;

    if data_end <= data.len() && data_start <= data_end {
        Some(ChunkMetadata {
            x,
            z,
            compression,
            start: data_start,
            end: data_end,
        })
    } else {
        None
    }
}
