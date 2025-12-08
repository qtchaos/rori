pub mod decompress;
pub mod nbt;

use crate::decompress::{CompressionType, anvil_region, mmap_region};

use fastanvil::CompressionScheme;
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, error, info, trace, warn};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

/// Minecraft regions are 32x32 chunks
const REGION_SIZE: usize = 32;

/// Represents a Minecraft dimension
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Overworld,
    Nether,
    End,
}

impl Dimension {
    /// Get the relative path for this dimension within a world directory
    pub fn path(&self) -> &'static str {
        match self {
            Self::Overworld => "region",
            Self::Nether => "DIM-1/region",
            Self::End => "DIM1/region",
        }
    }

    /// Get a human-readable name for this dimension
    pub fn name(&self) -> &'static str {
        match self {
            Self::Overworld => "Overworld",
            Self::Nether => "Nether",
            Self::End => "End",
        }
    }
}

/// Configuration options for processing Minecraft region files.
#[derive(Debug, Clone, Default)]
pub struct Conf {
    /// If true, simulate processing without making any file modifications
    pub dry_run: bool,
    /// Minimum `InhabitedTime` value (in ticks) for chunks to be kept
    pub inhabited_time_threshold: u32,
    /// If true, delete region instead of chunks
    pub delete_regions: bool,
    /// No progress bar
    pub no_progress: bool,
}

#[derive(Debug)]
pub enum ProcessError {
    /// I/O error occurred during file operations
    IoError(std::io::Error),
    /// Error specific to region file operations
    RegionError(String),
    /// Error specific to chunk operations
    ChunkError(String),
    /// No .mca files found in directory
    NoFilesFound,
    /// Invalid region size
    InvalidRegionSize,
}

impl std::fmt::Display for ProcessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(e) => write!(f, "IO error: {e}"),
            Self::RegionError(msg) => write!(f, "Region error: {msg}"),
            Self::ChunkError(msg) => write!(f, "Chunk error: {msg}"),
            Self::NoFilesFound => write!(f, "No .mca files found"),
            Self::InvalidRegionSize => write!(f, "Invalid region size"),
        }
    }
}

impl std::error::Error for ProcessError {}

impl From<std::io::Error> for ProcessError {
    fn from(error: std::io::Error) -> Self {
        Self::IoError(error)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ChunkStats {
    /// Total number of chunks processed in the region
    pub total: u32,
    /// Number of chunks in the region that meet the inhabited time threshold
    pub inhabited: u32,
}

impl ChunkStats {
    /// Merge another `ChunkStats` into this one
    const fn merge(&mut self, other: &Self) {
        self.total += other.total;
        self.inhabited += other.inhabited;
    }
}

#[derive(Debug, Default)]
pub struct RegionStats {
    /// Whether this region was deleted
    pub deleted: bool,
    /// Aggregate chunk statistics
    pub chunks: ChunkStats,
}

/// Results from processing a single dimension
#[derive(Debug)]
pub struct DimensionResult {
    pub dimension: Dimension,
    pub regions: Vec<Region>,
    pub total_regions: usize,
    pub deleted_regions: usize,
    pub total_chunk_stats: ChunkStats,
}

/// Results from processing a world (potentially multiple dimensions)
#[derive(Debug)]
pub struct ProcessingResult {
    pub dimension_results: Vec<DimensionResult>,
    pub total_regions: usize,
    pub deleted_regions: usize,
    pub total_chunk_stats: ChunkStats,
}

#[derive(Debug)]
pub struct Region {
    // 32x32 grid of chunk data
    pub chunks: Vec<Vec<Option<Chunk>>>,
    pub stats: RegionStats,
    pub compression: CompressionType,
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub data: Option<Vec<u8>>,
    pub inhabited_time: i64,
}

#[derive(Debug, Clone)]
struct ChunkMetadata {
    compression: CompressionType,
    start: usize,
    end: usize,
    x: usize,
    z: usize,
}

impl Region {
    fn new() -> Self {
        Self {
            chunks: vec![vec![None; REGION_SIZE]; REGION_SIZE],
            stats: RegionStats {
                deleted: false,
                chunks: ChunkStats::default(),
            },
            compression: CompressionType::Zlib, // Default to Zlib
        }
    }

    pub fn clear_chunk_data(&mut self) {
        for row in &mut self.chunks {
            for c in row.iter_mut().flatten() {
                c.data = None;
            }
        }
    }
}

/// Process a Minecraft world, automatically detecting and processing specified dimensions.
///
/// # Arguments
/// * `path` - Path to the world directory or specific region directory
/// * `config` - Processing configuration options
/// * `dimensions` - Which dimensions to process
pub fn process_world(
    path: &Path,
    config: &Conf,
    dimensions: &[Dimension],
) -> Result<ProcessingResult, ProcessError> {
    // Check if path is a world directory or a direct region directory
    let is_world_dir =
        path.join("region").exists() || path.join("DIM-1").exists() || path.join("DIM1").exists();

    if is_world_dir {
        // Process as world directory with multiple dimensions
        process_world_directory(path, config, dimensions)
    } else {
        // Process as single region directory (backward compatibility)
        info!("Processing as single region directory");
        let result = process_directory(path, config, &Dimension::Overworld)?;

        Ok(ProcessingResult {
            dimension_results: vec![DimensionResult {
                dimension: Dimension::Overworld,
                regions: result.regions,
                total_regions: result.total_regions,
                deleted_regions: result.deleted_regions,
                total_chunk_stats: result.total_chunk_stats,
            }],
            total_regions: result.total_regions,
            deleted_regions: result.deleted_regions,
            total_chunk_stats: result.total_chunk_stats,
        })
    }
}

/// Process a world directory with multiple dimensions
fn process_world_directory(
    world_path: &Path,
    config: &Conf,
    dimensions: &[Dimension],
) -> Result<ProcessingResult, ProcessError> {
    let mut dimension_results = Vec::new();
    let mut total_regions = 0;
    let mut deleted_regions = 0;
    let mut total_chunk_stats = ChunkStats::default();

    for &dimension in dimensions {
        let dimension_path = world_path.join(dimension.path());

        if !dimension_path.exists() {
            debug!(
                "{} dimension not found at {}, skipping",
                dimension.name(),
                dimension_path.display()
            );
            continue;
        }

        match process_directory(&dimension_path, config, &dimension) {
            Ok(result) => {
                total_regions += result.total_regions;
                deleted_regions += result.deleted_regions;
                total_chunk_stats.merge(&result.total_chunk_stats);

                dimension_results.push(DimensionResult {
                    dimension,
                    regions: result.regions,
                    total_regions: result.total_regions,
                    deleted_regions: result.deleted_regions,
                    total_chunk_stats: result.total_chunk_stats,
                });
            }
            Err(ProcessError::NoFilesFound) => {
                debug!("No region files found in {} dimension", dimension.name());
            }
            Err(e) => {
                warn!("Failed to process {} dimension: {}", dimension.name(), e);
            }
        }
    }

    if dimension_results.is_empty() {
        return Err(ProcessError::NoFilesFound);
    }

    Ok(ProcessingResult {
        dimension_results,
        total_regions,
        deleted_regions,
        total_chunk_stats,
    })
}

/// Internal result type for processing a single directory
#[derive(Debug)]
struct DirectoryResult {
    regions: Vec<Region>,
    total_regions: usize,
    deleted_regions: usize,
    total_chunk_stats: ChunkStats,
}

/// Process all Minecraft region files in a directory.
///
/// # Arguments
/// * `path` - Path to the directory containing .mca files
/// * `config` - Processing configuration options
fn process_directory(
    path: &Path,
    config: &Conf,
    dimension: &Dimension,
) -> Result<DirectoryResult, ProcessError> {
    let start = Instant::now();
    let region_files = find_region_files(path)?;
    debug!(
        "Found {} region files in {} (took {:.2?})",
        region_files.len(),
        path.display(),
        start.elapsed()
    );

    if region_files.is_empty() {
        warn!("No .mca files found in directory: {}", path.display());
        return Err(ProcessError::NoFilesFound);
    }

    let pb = if !config.no_progress {
        let pb = ProgressBar::new(region_files.len() as u64);
        pb.set_style(
            ProgressStyle::with_template(&format!(
                "[{{elapsed_precise}}] [{{bar:40}}] {{pos}}/{{len}} {} - {{msg}}",
                dimension.name()
            ))
            .unwrap_or_else(|e| {
                warn!("Failed to set progress bar style: {e}, using default");
                ProgressStyle::default_bar()
            }),
        );
        Some(pb)
    } else {
        None
    };

    // Process regions in parallel
    let regions: Vec<Region> = region_files
        .par_iter()
        .filter_map(|region_path| match process_region(region_path, config) {
            Ok(region) => {
                debug!("Processed region {}", region_path.display());
                if let Some(ref pb) = pb {
                    pb.inc(1);
                }
                Some(region)
            }
            Err(err) => {
                error!("Failed to process region: {err}");
                if let Some(ref pb) = pb {
                    pb.inc(1);
                }
                None
            }
        })
        .collect();

    if let Some(pb) = pb {
        pb.finish_with_message("done");
    }

    // Aggregate results
    let mut total_regions = 0;
    let mut deleted_regions = 0;
    let mut total_chunk_stats = ChunkStats::default();

    for region in &regions {
        total_regions += 1;
        if region.stats.deleted {
            deleted_regions += 1;
        }

        total_chunk_stats.merge(&region.stats.chunks);
    }

    Ok(DirectoryResult {
        regions,
        total_regions,
        deleted_regions,
        total_chunk_stats,
    })
}

/// Find all .mca (Minecraft Anvil) region files in a directory.
fn find_region_files(path: &Path) -> Result<Vec<PathBuf>, ProcessError> {
    let entries = fs::read_dir(path)?;

    let regions: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("mca"))
        })
        .collect();

    Ok(regions)
}

/// Process a single Minecraft region file, apply file system changes and return some information about it.
pub fn process_region(region_path: &PathBuf, config: &Conf) -> Result<Region, ProcessError> {
    trace!("Processing region: {}", region_path.display());

    // try mmap
    let mut data = mmap_region(region_path, config.inhabited_time_threshold);

    // fallback if mmap failed
    if data.is_err() {
        trace!(
            "Mmap failed/skipped, using fallback for: {}",
            region_path.display()
        );

        data = anvil_region(region_path);
    }

    let Ok(mut region) = data else {
        return Err(ProcessError::RegionError(format!(
            "Failed to obtain region data for {}",
            region_path.display()
        )));
    };

    apply_filesystem_changes(region_path, &mut region, config)?;

    // Clear the uncompressed data so that we dont flood memory
    region.clear_chunk_data();

    trace!("Region {} stats: {:?}", region_path.display(), region.stats);

    Ok(region)
}

/// Decides whether to delete the file, rewrite it, or do nothing based on stats.
fn apply_filesystem_changes(
    region_path: &Path,
    data: &mut Region,
    config: &Conf,
) -> Result<(), ProcessError> {
    let has_chunks = data.stats.chunks.inhabited > 0;
    let total_chunks = data.stats.chunks.total;
    let deleteable_chunks = data.stats.chunks.total - data.stats.chunks.inhabited;
    // Handle empty regions - just skip them or delete if appropriate
    if total_chunks == 0 {
        if !config.dry_run && config.delete_regions {
            fs::remove_file(region_path)?;
            debug!("Deleted empty region file: {}", region_path.display());
            data.stats.deleted = true;
        }
        return Ok(());
    }

    // Case 1: Delete entire region
    if config.delete_regions {
        if !has_chunks {
            if config.dry_run {
                debug!("Would delete region file: {}", region_path.display());
            } else {
                fs::remove_file(region_path)?;
                debug!("Deleted region file: {}", region_path.display());
            }
            data.stats.deleted = true;
        }
    } else {
        // Case 2: Rewrite region (compaction)
        if !config.dry_run && deleteable_chunks > 0 {
            if has_chunks {
                // Some chunks meet threshold - rewrite to remove the others
                rewrite_region(region_path, &data.chunks, config.inhabited_time_threshold)?;
                debug!(
                    "Deleted {} chunks from {}",
                    deleteable_chunks,
                    region_path.display()
                );
            } else {
                // No chunks meet threshold - delete entire region
                fs::remove_file(region_path)?;
                debug!(
                    "Deleted region file (no chunks met threshold): {}",
                    region_path.display()
                );
                data.stats.deleted = true;
            }
        }
    }

    Ok(())
}

/// Writes a new .mca file containing only the chunks that meet the inhabited time threshold.
/// Assumes at least one chunk meets the threshold (caller should verify before calling).
fn rewrite_region(
    region_path: &Path,
    chunks: &[Vec<Option<Chunk>>],
    inhabited_time_threshold: u32,
) -> Result<(), ProcessError> {
    let temp_path = format!("{}-temp.mca", region_path.display());
    let temp_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .truncate(true)
        .open(&temp_path)?;

    let mut new_region = fastanvil::Region::new(temp_file)
        .map_err(|e| ProcessError::RegionError(format!("Failed to create new region: {e}")))?;

    // Write only the chunks that meet the threshold to the new region
    for x in 0..REGION_SIZE {
        for z in 0..REGION_SIZE {
            if let Some(Some(chunk_data)) = chunks.get(x).and_then(|row| row.get(z)) {
                // Only write chunks that meet the inhabited time threshold
                if chunk_data.inhabited_time > i64::from(inhabited_time_threshold)
                    && let Some(ref data) = chunk_data.data
                    && let Err(e) = new_region.write_compressed_chunk(
                        x,
                        z,
                        CompressionScheme::Zlib,
                        data.as_slice(),
                    )
                {
                    warn!("Failed to write chunk ({x}, {z}) to new region: {e}");
                }
            }
        }
    }

    fs::rename(&temp_path, region_path)?;
    Ok(())
}
