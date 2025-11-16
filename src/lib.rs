pub mod nbt;

use flate2::read::{GzDecoder, ZlibDecoder};
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, error, trace, warn};
use memmap2::MmapOptions;
use rayon::iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use std::{
    cell::RefCell,
    fs,
    io::{BufReader, Cursor, Read},
    path::{Path, PathBuf},
    time::Instant,
};

use crate::nbt::{NbtError, NbtReader, NbtTag};

/// Type alias for chunk coordinates (x, z) within a region (0-31)
type ChunkCoord = usize;

/// Minecraft regions are 32x32 chunks
const REGION_SIZE: usize = 32;

/// Region file sector size in bytes
const SECTOR_SIZE: usize = 4096;

/// Extract the "InhabitedTime" field from chunk NBT data.
/// This parser only searches for the specific field.
///
/// # Arguments
/// * `chunk_data` - Raw NBT data from a Minecraft chunk
///
/// # Returns
/// A tuple of (Option<i64>, usize) where:
/// - First element: InhabitedTime value if found, None if not found
/// - Second element: Byte position where the value was found (0 if not found)
/// - Returns Err(NbtError) if parsing failed
fn extract_inhabited_time(chunk_data: &[u8]) -> Result<(Option<i64>, usize), NbtError> {
    // Prefetch the beginning of the chunk data into cache
    if chunk_data.len() >= 64 {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            if is_x86_feature_detected!("sse") {
                std::arch::x86_64::_mm_prefetch(
                    chunk_data.as_ptr() as *const i8,
                    std::arch::x86_64::_MM_HINT_T0,
                );
            }
        }
    }

    let mut reader = NbtReader::new(chunk_data);

    // Read root compound tag
    let tag_type = NbtTag::from_u8(reader.read_u8()?)?;
    if tag_type != NbtTag::Compound {
        return Err(NbtError::InvalidFormat(
            "Root tag is not a compound".to_string(),
        ));
    }

    // Skip root tag name
    reader.skip_string()?;

    // Search through the root compound for InhabitedTime
    const INHABITED_TIME: &[u8] = b"InhabitedTime";
    reader.search_compound_for_field(INHABITED_TIME, chunk_data)
}

/// Information about a chunk's location in the region file
struct ChunkMetadata {
    x: ChunkCoord,
    z: ChunkCoord,
    compression_type: u8,
    data_start: usize,
    data_end: usize,
}

/// Configuration options for processing Minecraft region files.
#[derive(Debug, Clone)]
pub struct ProcessingOptions {
    /// If true, simulate processing without making any file modifications
    pub dry_run: bool,
    /// Minimum InhabitedTime value (in ticks) for chunks to be kept
    pub inhabited_time_threshold: u32,
    /// If true, delete entire region files when they contain no inhabited chunks
    pub delete_entire_regions: bool,
    /// Maximum bytes to decompress per chunk (0 = decompress fully)
    /// Partial decompression is faster but falls back to full if parsing fails
    pub max_decompression_bytes: usize,
}

thread_local! {
    /// Thread-local decompression buffer pool (reuse across chunks)
    static DECOMP_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(1_000_000));
    /// Thread-local partial decompression buffer (reuse for partial decompressions)
    static PARTIAL_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(8192));
}

/// Compression types used in Minecraft region files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionType {
    /// GZip compression (type 1)
    GZip,
    /// Zlib/Deflate compression (type 2)
    Zlib,
}

impl CompressionType {
    /// Parse compression type from a byte value.
    pub fn from_byte(byte: u8) -> Result<Self, ProcessError> {
        match byte {
            1 => Ok(CompressionType::GZip),
            2 => Ok(CompressionType::Zlib),
            _ => Err(ProcessError::ChunkError(format!(
                "Unknown compression type: {}",
                byte
            ))),
        }
    }
}

/// Decompress chunk data using the specified compression type.
///
/// # Arguments
/// * `compression` - The compression type to use
/// * `compressed_data` - The compressed chunk data
/// * `max_bytes` - Maximum bytes to decompress (0 = decompress fully)
///
/// # Returns
/// The decompressed data, or an error if decompression fails
pub fn decompress_chunk(
    compression: CompressionType,
    compressed_data: &[u8],
    max_bytes: usize,
) -> Result<Vec<u8>, ProcessError> {
    if max_bytes == 0 {
        // Full decompression requested
        return decompress_full(compression, compressed_data);
    }

    // Try partial decompression using thread-local buffer
    PARTIAL_BUF.with(|buf_cell| {
        let mut buf = buf_cell.borrow_mut();
        
        // Ensure buffer has the right capacity
        let current_capacity = buf.capacity();
        if current_capacity < max_bytes {
            buf.reserve(max_bytes - current_capacity);
        }
        buf.clear();
        buf.resize(max_bytes, 0);
        
        let bytes_read = match compression {
            CompressionType::GZip => {
                let mut decoder = GzDecoder::new(Cursor::new(compressed_data));
                decoder.read(&mut buf).map_err(|e| {
                    ProcessError::ChunkError(format!("GZip decompression failed: {}", e))
                })?
            }
            CompressionType::Zlib => {
                let mut decoder = ZlibDecoder::new(Cursor::new(compressed_data));
                decoder.read(&mut buf).map_err(|e| {
                    ProcessError::ChunkError(format!("Zlib decompression failed: {}", e))
                })?
            }
        };

        buf.truncate(bytes_read);
        Ok(buf.clone())
    })
}

/// Decompress chunk data fully using thread-local buffer for efficiency.
pub fn decompress_full(
    compression: CompressionType,
    compressed_data: &[u8],
) -> Result<Vec<u8>, ProcessError> {
    DECOMP_BUF.with(|buf_cell| {
        let mut buf = buf_cell.borrow_mut();
        buf.clear();

        match compression {
            CompressionType::GZip => {
                let mut decoder = GzDecoder::new(Cursor::new(compressed_data));
                decoder.read_to_end(&mut buf).map_err(|e| {
                    ProcessError::ChunkError(format!("GZip decompression failed: {}", e))
                })?;
            }
            CompressionType::Zlib => {
                let mut decoder = ZlibDecoder::new(Cursor::new(compressed_data));
                decoder.read_to_end(&mut buf).map_err(|e| {
                    ProcessError::ChunkError(format!("Zlib decompression failed: {}", e))
                })?;
            }
        }
        Ok(buf.clone())
    })
}

#[derive(Debug)]
pub enum ProcessError {
    /// I/O error occurred during file operations
    IoError(std::io::Error),
    /// Error specific to region file operations
    RegionError(String),
    /// Error specific to chunk operations
    ChunkError(String),
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessError::IoError(e) => write!(f, "IO error: {}", e),
            ProcessError::RegionError(msg) => write!(f, "Region error: {}", msg),
            ProcessError::ChunkError(msg) => write!(f, "Chunk error: {}", msg),
        }
    }
}

impl std::error::Error for ProcessError {}

impl From<std::io::Error> for ProcessError {
    fn from(error: std::io::Error) -> Self {
        ProcessError::IoError(error)
    }
}

#[derive(Debug, Default)]
struct ChunkStats {
    /// Total number of chunks processed
    total_chunks: u32,
    /// Number of chunks that meet the inhabited time threshold
    inhabited_chunks: u32,
}

impl ChunkStats {
    /// Merge another ChunkStats into this one
    fn merge(&mut self, other: ChunkStats) {
        self.total_chunks += other.total_chunks;
        self.inhabited_chunks += other.inhabited_chunks;
    }
}

#[derive(Debug, Default)]
struct RegionStats {
    /// Whether this region was deleted (1) or kept (0)
    deleted: u32,
    /// Aggregate chunk statistics
    chunk_stats: ChunkStats,
    /// Statistics about InhabitedTime positions in chunks
    position_stats: PositionStats,
}

/// Statistics about where InhabitedTime appears in decompressed chunks
#[derive(Debug, Default)]
struct PositionStats {
    /// Minimum byte position where InhabitedTime was found
    min_position: usize,
    /// Maximum byte position where InhabitedTime was found
    max_position: usize,
    /// Sum of all positions (for calculating average)
    sum_positions: usize,
    /// Count of chunks where InhabitedTime was found
    count: usize,
}

/// Results from processing a directory of region files
#[derive(Debug)]
pub struct ProcessingResult {
    /// Total number of regions processed
    pub total_regions: u32,
    /// Total number of chunks processed
    pub total_chunks: u32,
    /// Number of chunks that met the inhabited time threshold
    pub inhabited_chunks: u32,
    /// Number of regions that were deleted
    pub deleted_regions: u32,
    /// Minimum byte position where InhabitedTime was found
    pub min_position: usize,
    /// Maximum byte position where InhabitedTime was found
    pub max_position: usize,
    /// Average byte position where InhabitedTime was found
    pub avg_position: usize,
    /// Count of chunks where InhabitedTime was found
    pub position_count: usize,
}

impl PositionStats {
    fn update(&mut self, position: usize) {
        if self.count == 0 {
            self.min_position = position;
            self.max_position = position;
        } else {
            self.min_position = self.min_position.min(position);
            self.max_position = self.max_position.max(position);
        }
        self.sum_positions += position;
        self.count += 1;
    }

    fn merge(&mut self, other: &PositionStats) {
        if other.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = PositionStats {
                min_position: other.min_position,
                max_position: other.max_position,
                sum_positions: other.sum_positions,
                count: other.count,
            };
        } else {
            self.min_position = self.min_position.min(other.min_position);
            self.max_position = self.max_position.max(other.max_position);
            self.sum_positions += other.sum_positions;
            self.count += other.count;
        }
    }

    fn average(&self) -> usize {
        if self.count == 0 {
            0
        } else {
            self.sum_positions / self.count
        }
    }
}

/// Process all Minecraft region files in a directory.
///
/// # Arguments
/// * `path` - Path to the directory containing .mca files
/// * `options` - Processing configuration options
///
/// # Returns
/// ProcessingResult with statistics on success, or a ProcessError if processing fails
pub fn process_directory(
    path: &Path,
    options: &ProcessingOptions,
) -> Result<ProcessingResult, ProcessError> {
    let start = Instant::now();
    let regions = find_region_files(path)?;
    debug!(
        "Found {} region files in {} (took {:.2?})",
        regions.len(),
        path.display(),
        start.elapsed()
    );

    if regions.is_empty() {
        warn!("No .mca files found in directory: {}", path.display());
        return Ok(ProcessingResult {
            total_regions: 0,
            total_chunks: 0,
            inhabited_chunks: 0,
            deleted_regions: 0,
            min_position: 0,
            max_position: 0,
            avg_position: 0,
            position_count: 0,
        });
    }

    let pb = ProgressBar::new(regions.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("[{elapsed_precise}] [{bar:40}] {pos}/{len} {msg}").unwrap(),
    );

    // Process regions in parallel
    let results: Vec<Result<RegionStats, ProcessError>> = regions
        .par_iter()
        .map(|region_path| {
            let res = process_region(region_path, options);
            pb.inc(1);
            res
        })
        .collect();

    pb.finish_with_message("done");

    // Aggregate results
    let mut total_regions = 0u32;
    let mut deleted_regions = 0u32;
    let mut total_chunk_stats = ChunkStats::default();
    let mut total_position_stats = PositionStats::default();

    for result in results {
        match result {
            Ok(stats) => {
                total_regions += 1;
                deleted_regions += stats.deleted;
                total_chunk_stats.merge(stats.chunk_stats);
                total_position_stats.merge(&stats.position_stats);
            }
            Err(e) => {
                error!("Region processing error: {}", e);
            }
        }
    }

    Ok(ProcessingResult {
        total_regions,
        total_chunks: total_chunk_stats.total_chunks,
        inhabited_chunks: total_chunk_stats.inhabited_chunks,
        deleted_regions,
        min_position: total_position_stats.min_position,
        max_position: total_position_stats.max_position,
        avg_position: total_position_stats.average(),
        position_count: total_position_stats.count,
    })
}

/// Find all .mca (Minecraft Anvil) region files in a directory.
fn find_region_files(path: &Path) -> Result<Vec<PathBuf>, ProcessError> {
    let entries = fs::read_dir(path)?;

    let regions: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map_or(false, |ext| ext.eq_ignore_ascii_case("mca"))
        })
        .collect();

    Ok(regions)
}

/// Process a single region file.
///
/// Reads chunks from the region, checks their InhabitedTime, and either:
/// - Deletes the entire region if no chunks meet the threshold (when delete_entire_regions is true)
/// - Rebuilds the region with only chunks that meet the threshold
///
/// # Arguments
/// * `region_path` - Path to the .mca region file
/// * `options` - Processing configuration options
fn process_region(
    region_path: &Path,
    options: &ProcessingOptions,
) -> Result<RegionStats, ProcessError> {
    trace!("Processing region: {}", region_path.display());
    let file = fs::File::open(region_path)?;

    // Storage for chunks that meet the inhabited time threshold (32x32 grid)
    let mut chunks: Vec<Vec<Option<Vec<u8>>>> = vec![vec![None; REGION_SIZE]; REGION_SIZE];

    // Try mmap and parse the region header directly. On failure, fall back to fastanvil.
    let mut chunk_stats = ChunkStats::default();
    let mut deleted_count = 0;
    let mut position_stats = PositionStats::default();

    // If mmap fails OR any access violations occur (caught by fallback to fastanvil),
    // we gracefully degrade to buffered I/O. In production, ensure exclusive access
    // to region files during processing.
    let mmap_result = unsafe {
        let mut opts = MmapOptions::new();

        // On Linux, populate pages during mapping to catch truncation/SIGBUS early.
        // This trades slightly slower mmap() for safer access and better error handling.
        #[cfg(target_os = "linux")]
        opts.populate();

        opts.map(&file)
    };

    if let Ok(mmap) = mmap_result {
        let data: &[u8] = &mmap[..];

        // Validate minimum size and accessibility before proceeding.
        // Region files must have at least 8 KiB (4 KiB header + 4 KiB timestamps)
        // This helps catch truncation/corruption early before SIGBUS occurs.
        if data.len() >= 8192 && !data.is_empty() {
            // Collect chunk metadata (coordinates + compressed data location)
            // Pre-allocate with estimated capacity for better performance
            let mut chunk_infos: Vec<ChunkMetadata> = Vec::with_capacity(256);

            // Parse region header: 1024 u32 big-endian offsets (4 KiB total)
            for x in 0..REGION_SIZE {
                for z in 0..REGION_SIZE {
                    let idx = x + z * REGION_SIZE;
                    let header_offset = idx * 4;
                    if header_offset + 4 > data.len() {
                        continue;
                    }
                    let raw = u32::from_be_bytes([
                        data[header_offset],
                        data[header_offset + 1],
                        data[header_offset + 2],
                        data[header_offset + 3],
                    ]);
                    let sector = raw >> 8;
                    if sector == 0 {
                        continue;
                    }
                    let byte_offset = (sector as usize) * SECTOR_SIZE;
                    if byte_offset + 4 > data.len() {
                        continue;
                    }
                    let length = u32::from_be_bytes([
                        data[byte_offset],
                        data[byte_offset + 1],
                        data[byte_offset + 2],
                        data[byte_offset + 3],
                    ]) as usize;
                    if length == 0 || byte_offset + 4 + length > data.len() {
                        continue;
                    }

                    // compression type is the first byte after length
                    let compression_type = data[byte_offset + 4];
                    let data_start = byte_offset + 5;
                    let data_end = byte_offset + 4 + length;
                    if data_end > data.len() || data_start > data_end {
                        continue;
                    }

                    chunk_infos.push(ChunkMetadata {
                        x,
                        z,
                        compression_type,
                        data_start,
                        data_end,
                    });
                }
            }

            // Parallel decompression + parsing of all chunks
            let results: Vec<_> = chunk_infos
                .into_par_iter()
                .filter_map(|chunk_info| {
                    let compressed_data = &data[chunk_info.data_start..chunk_info.data_end];

                    // Parse compression type
                    let compression = match CompressionType::from_byte(chunk_info.compression_type)
                    {
                        Ok(c) => c,
                        Err(_) => return None,
                    };

                    // Try partial decompression first (if configured)
                    let mut decompressed = match decompress_chunk(
                        compression,
                        compressed_data,
                        options.max_decompression_bytes,
                    ) {
                        Ok(v) => v,
                        Err(_) => return None, // skip on decompression error
                    };

                    // Parse NBT to extract inhabited time
                    let mut inhabited_time = extract_inhabited_time(&decompressed);

                    // If parsing failed and we used partial decompression, retry with full decompression
                    if inhabited_time.is_err() && options.max_decompression_bytes != 0 {
                        if let Ok(full_decomp) = decompress_full(compression, compressed_data) {
                            decompressed = full_decomp;
                            inhabited_time = extract_inhabited_time(&decompressed);
                        }
                    }

                    let inhabited_time = match inhabited_time {
                        Ok(t) => t,
                        Err(_) => return None,
                    };

                    Some((chunk_info.x, chunk_info.z, decompressed, inhabited_time))
                })
                .collect();

            // Aggregate results
            for (x, z, decompressed, (inhabited_time_value, position)) in results {
                chunk_stats.total_chunks += 1;

                if let Some(time) = inhabited_time_value {
                    if time > options.inhabited_time_threshold as i64 {
                        chunk_stats.inhabited_chunks += 1;
                        chunks[x][z] = Some(decompressed);

                        // Track position statistics
                        if position > 0 {
                            position_stats.update(position);
                        }
                    } else {
                        deleted_count += 1;
                    }
                } else {
                    deleted_count += 1;
                }
            }
        } else {
            // File too small or empty - log warning and fall back
            warn!(
                "Memory-mapped region file is too small or empty (size: {} bytes), \
                 falling back to buffered I/O: {}",
                data.len(),
                region_path.display()
            );
        }
    } else {
        // mmap failed - fall back silently (common on some filesystems)
        debug!(
            "Memory mapping failed for {}, using buffered I/O",
            region_path.display()
        );
    }

    // Fallback: use fastanvil region reader if mmap failed or data was invalid
    if chunk_stats.total_chunks == 0 {
        let file2 = fs::File::open(region_path)?;
        let mut mca = fastanvil::Region::from_stream(BufReader::new(file2)).map_err(|e| {
            ProcessError::RegionError(format!(
                "Failed to create region from {}: {}",
                region_path.display(),
                e
            ))
        })?;

        // Iterate through all chunks to determine which to keep
        for x in 0..REGION_SIZE {
            for z in 0..REGION_SIZE {
                if let Ok(Some(chunk_data)) = mca.read_chunk(x, z) {
                    chunk_stats.total_chunks += 1;

                    let (inhabited_time_value, position) = extract_inhabited_time(&chunk_data)
                        .map_err(|e| {
                            ProcessError::ChunkError(format!("Failed to process chunk: {}", e))
                        })?;

                    if let Some(time) = inhabited_time_value {
                        if time > options.inhabited_time_threshold as i64 {
                            chunk_stats.inhabited_chunks += 1;
                            chunks[x][z] = Some(chunk_data.to_vec());

                            // Track position statistics
                            if position > 0 {
                                position_stats.update(position);
                            }
                        } else {
                            deleted_count += 1;
                        }
                    } else {
                        deleted_count += 1;
                    }
                }
            }
        }
    }

    let mut region_stats = RegionStats {
        deleted: 0,
        chunk_stats: ChunkStats::default(),
        position_stats: PositionStats::default(),
    };
    region_stats.chunk_stats.merge(chunk_stats);
    region_stats.position_stats.merge(&position_stats);

    if options.delete_entire_regions {
        // In region deletion mode, delete the entire region if no inhabited chunks
        if region_stats.chunk_stats.inhabited_chunks == 0
            && region_stats.chunk_stats.total_chunks > 0
        {
            if !options.dry_run {
                fs::remove_file(region_path)?;
                debug!("Deleted region file: {}", region_path.display());
            } else {
                debug!("Would delete region file: {}", region_path.display());
            }
            region_stats.deleted = 1;
        }
    } else {
        // In chunk deletion mode, rebuild the region with only inhabited chunks
        if !options.dry_run && deleted_count > 0 {
            let temp_path = format!("{}-temp.mca", region_path.display());
            let temp_file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .read(true)
                .truncate(true)
                .open(&temp_path)?;

            let mut new_region = fastanvil::Region::new(temp_file).map_err(|e| {
                ProcessError::RegionError(format!("Failed to create new region: {}", e))
            })?;

            // Write only the chunks that meet the threshold to the new region
            for x in 0..REGION_SIZE {
                for z in 0..REGION_SIZE {
                    if let Some(Some(chunk_data)) = chunks.get(x).and_then(|row| row.get(z)) {
                        if let Err(e) = new_region.write_chunk(x, z, chunk_data) {
                            warn!("Failed to write chunk ({}, {}) to new region: {}", x, z, e);
                        }
                    }
                }
            }

            // Replace original file with the compacted version
            fs::rename(&temp_path, region_path)?;

            debug!(
                "Deleted {} chunks from {} (compacted)",
                deleted_count,
                region_path.display()
            );
        }
    }

    trace!("Region {} stats: {:?}", region_path.display(), region_stats);

    Ok(region_stats)
}
