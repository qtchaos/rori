pub mod decompress;
pub mod nbt;
pub mod timing;

use crate::decompress::{
    CompressionType, MappedRegion, ScannedChunk, anvil_region, mmap_region_for_processing,
};
use crate::timing::{StageTimings, log_timing_summary};

use fastanvil::CompressionScheme;
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, error, info, trace, warn};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use std::{
    fs,
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    time::Instant,
};

/// Minecraft regions are 32x32 chunks
const REGION_SIZE: usize = 32;
/// Region file sector size in bytes
pub(crate) const SECTOR_SIZE: usize = 4096;

/// Represents a Minecraft dimension
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dimension {
    Overworld,
    Nether,
    End,
}

/// World storage format version
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorldFormat {
    /// Pre-1.26: region/ at root, DIM-1/, DIM1/
    Legacy,
    /// 1.26+: dimensions/minecraft/overworld/region/ etc.
    Modern,
}

impl Dimension {
    /// Get the region directory path within a world directory for the given format.
    #[must_use]
    pub const fn region_path(&self, format: WorldFormat) -> &'static str {
        match (self, format) {
            (Self::Overworld, WorldFormat::Legacy) => "region",
            (Self::Nether, WorldFormat::Legacy) => "DIM-1/region",
            (Self::End, WorldFormat::Legacy) => "DIM1/region",
            (Self::Overworld, WorldFormat::Modern) => "dimensions/minecraft/overworld/region",
            (Self::Nether, WorldFormat::Modern) => "dimensions/minecraft/the_nether/region",
            (Self::End, WorldFormat::Modern) => "dimensions/minecraft/the_end/region",
        }
    }

    /// Get a human-readable name for this dimension
    #[must_use]
    pub const fn name(&self) -> &'static str {
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
    const fn merge(&mut self, other: Self) {
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
    /// Timing samples for major processing stages
    pub timings: StageTimings,
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChunkMetadata {
    pub(crate) compression: CompressionType,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) x: usize,
    pub(crate) z: usize,
}

impl Region {
    fn new() -> Self {
        Self {
            chunks: vec![vec![None; REGION_SIZE]; REGION_SIZE],
            stats: RegionStats {
                deleted: false,
                chunks: ChunkStats::default(),
                timings: StageTimings::default(),
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
///
/// Detect the world storage format by probing known directory layouts.
#[must_use]
pub fn detect_world_format(path: &Path) -> Option<WorldFormat> {
    // Check modern layout first (more specific)
    if path.join("dimensions/minecraft/overworld/region").exists()
        || path.join("dimensions/minecraft/the_nether/region").exists()
        || path.join("dimensions/minecraft/the_end/region").exists()
    {
        return Some(WorldFormat::Modern);
    }
    // Check legacy layout
    if path.join("region").exists() || path.join("DIM-1").exists() || path.join("DIM1").exists() {
        return Some(WorldFormat::Legacy);
    }
    None
}

pub fn process_world(
    path: &Path,
    config: &Conf,
    dimensions: &[Dimension],
) -> Result<ProcessingResult, ProcessError> {
    process_world_with_format(path, config, dimensions, None)
}

/// Process a Minecraft world with an explicit or auto-detected storage format.
pub fn process_world_with_format(
    path: &Path,
    config: &Conf,
    dimensions: &[Dimension],
    format: Option<WorldFormat>,
) -> Result<ProcessingResult, ProcessError> {
    // Check if path is a world directory, a Spigot server root, or a direct region directory
    let format = format.or_else(|| detect_world_format(path));
    let is_server_root = path.join("server.properties").exists();

    if let Some(format) = format {
        // Process as vanilla world directory with multiple dimensions
        let context = match format {
            WorldFormat::Legacy => "Processing as legacy world directory",
            WorldFormat::Modern => "Processing as modern world directory",
        };
        process_dimensions(
            path,
            config,
            dimensions,
            |root, dim| root.join(dim.region_path(format)),
            context,
        )
    } else if is_server_root {
        // Process as Spigot/Bukkit/Paper server root (world_nether, world_the_end layout)
        let level_name = {
            let props_path = path.join("server.properties");
            let contents = fs::read_to_string(&props_path).unwrap_or_default();
            let mut name = String::from("world");
            for line in contents.lines() {
                if let Some(value) = line.strip_prefix("level-name=") {
                    let trimmed = value.trim().trim_matches('"');
                    if !trimmed.is_empty() {
                        name = trimmed.to_string();
                        break;
                    }
                }
            }
            name
        };
        process_dimensions(
            path,
            config,
            dimensions,
            |root, dim| {
                let path = match dim {
                    Dimension::Overworld => format!("{level_name}/region"),
                    Dimension::Nether => format!("{level_name}_nether/region"),
                    Dimension::End => format!("{level_name}_the_end/region"),
                };
                root.join(path)
            },
            &format!("Processing as server root, level-name: {level_name}"),
        )
    } else {
        // Process as single region directory (backward compatibility)
        info!("Processing as single region directory");
        let result = process_directory(path, config, Dimension::Overworld)?;

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

/// Process dimensions using a path-resolver to locate each dimension's region directory.
fn process_dimensions(
    root: &Path,
    config: &Conf,
    dimensions: &[Dimension],
    resolve_path: impl Fn(&Path, Dimension) -> PathBuf,
    context_msg: &str,
) -> Result<ProcessingResult, ProcessError> {
    info!("{context_msg}");

    let mut dimension_results = Vec::new();
    let mut total_regions = 0;
    let mut deleted_regions = 0;
    let mut total_chunk_stats = ChunkStats::default();

    for &dimension in dimensions {
        let dimension_path = resolve_path(root, dimension);

        if !dimension_path.exists() {
            debug!(
                "{} dimension not found at {}, skipping",
                dimension.name(),
                dimension_path.display()
            );
            continue;
        }

        match process_directory(&dimension_path, config, dimension) {
            Ok(result) => {
                total_regions += result.total_regions;
                deleted_regions += result.deleted_regions;
                total_chunk_stats.merge(result.total_chunk_stats);

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
    dimension: Dimension,
) -> Result<DirectoryResult, ProcessError> {
    let start = Instant::now();
    let region_files: Vec<PathBuf> = fs::read_dir(path)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|p| {
            p.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("mca"))
        })
        .collect();
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

    let pb = if config.no_progress {
        None
    } else {
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
    };

    // Process regions in parallel
    let regions: Vec<Region> = region_files
        .par_iter()
        .filter_map(|region_path| match process_region(region_path, config) {
            Ok(region) => {
                // debug!("Processed region {}", region_path.display());
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
    let mut total_timings = StageTimings::default();

    for region in &regions {
        total_regions += 1;
        if region.stats.deleted {
            deleted_regions += 1;
        }

        total_chunk_stats.merge(region.stats.chunks);
        total_timings.merge(region.stats.timings);
    }

    log_timing_summary(
        &format!("{} timing summary for {}", dimension.name(), path.display()),
        &format!("regions={total_regions}"),
        total_chunk_stats,
        total_timings,
        log::Level::Debug,
    );

    Ok(DirectoryResult {
        regions,
        total_regions,
        deleted_regions,
        total_chunk_stats,
    })
}

/// Process a single Minecraft region file, apply file system changes and return some information about it.
pub fn process_region(region_path: &PathBuf, config: &Conf) -> Result<Region, ProcessError> {
    trace!("Processing region: {}", region_path.display());

    // try mmap
    if let Ok(mapped_region) =
        mmap_region_for_processing(region_path, config.inhabited_time_threshold)
    {
        return process_mapped_region(region_path, mapped_region, config);
    }

    // fallback if mmap failed
    trace!(
        "Mmap failed/skipped, using fallback for: {}",
        region_path.display()
    );

    let Ok(mut region) = anvil_region(region_path) else {
        return Err(ProcessError::RegionError(format!(
            "Failed to obtain region data for {}",
            region_path.display()
        )));
    };
    let fs_start = log::log_enabled!(log::Level::Debug).then(Instant::now);
    apply_filesystem_changes(region_path, &mut region, config)?;
    if let Some(start) = fs_start {
        region.stats.timings.filesystem += start.elapsed();
    }

    // Clear the uncompressed data so that we dont flood memory
    region.clear_chunk_data();

    trace!(
        "Region {} stats: deleted={} chunks={:?}",
        region_path.display(),
        region.stats.deleted,
        region.stats.chunks
    );
    log_timing_summary(
        &format!("Timing for {}", region_path.display()),
        "",
        region.stats.chunks,
        region.stats.timings,
        log::Level::Trace,
    );

    Ok(region)
}

fn process_mapped_region(
    region_path: &Path,
    mapped_region: MappedRegion,
    config: &Conf,
) -> Result<Region, ProcessError> {
    let MappedRegion {
        mut region,
        mmap,
        chunks,
    } = mapped_region;

    let fs_start = log::log_enabled!(log::Level::Debug).then(Instant::now);
    let total_chunks = region.stats.chunks.total;
    let inhabited_chunks = region.stats.chunks.inhabited;
    if !config.dry_run
        && !config.delete_regions
        && total_chunks > 0
        && inhabited_chunks > 0
        && total_chunks > inhabited_chunks
    {
        let deleteable_chunks = region
            .stats
            .chunks
            .total
            .saturating_sub(region.stats.chunks.inhabited);
        rewrite_region_from_mmap(region_path, &chunks, &mmap, config.inhabited_time_threshold)?;
        drop(mmap);
        trace!(
            "Deleted {} chunks from {}",
            deleteable_chunks,
            region_path.display()
        );
    } else {
        drop(mmap);
        apply_filesystem_changes(region_path, &mut region, config)?;
    }
    if let Some(start) = fs_start {
        region.stats.timings.filesystem += start.elapsed();
    }

    // Clear the uncompressed data so that we dont flood memory
    region.clear_chunk_data();

    trace!(
        "Region {} stats: deleted={} chunks={:?}",
        region_path.display(),
        region.stats.deleted,
        region.stats.chunks
    );
    log_timing_summary(
        &format!("Timing for {}", region_path.display()),
        "",
        region.stats.chunks,
        region.stats.timings,
        log::Level::Trace,
    );

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
            trace!("Deleted empty region file: {}", region_path.display());
            data.stats.deleted = true;
        }
        return Ok(());
    }

    // Case 1: Delete entire region
    if config.delete_regions {
        if !has_chunks {
            if config.dry_run {
                trace!("Would delete region file: {}", region_path.display());
            } else {
                fs::remove_file(region_path)?;
                trace!("Deleted region file: {}", region_path.display());
            }
            data.stats.deleted = true;
        }
    } else {
        // Case 2: Rewrite region (compaction)
        if !config.dry_run && deleteable_chunks > 0 {
            if has_chunks {
                // Some chunks meet threshold - rewrite to remove the others
                rewrite_region(
                    region_path,
                    &data.chunks,
                    config.inhabited_time_threshold,
                    data.compression,
                )?;
                trace!(
                    "Deleted {} chunks from {}",
                    deleteable_chunks,
                    region_path.display()
                );
            } else {
                // No chunks meet threshold - delete entire region
                fs::remove_file(region_path)?;
                trace!(
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
    compression: CompressionType,
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
                        match compression {
                            CompressionType::GZip => CompressionScheme::Gzip,
                            CompressionType::Zlib => CompressionScheme::Zlib,
                        },
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

fn rewrite_region_from_mmap(
    region_path: &Path,
    chunks: &[ScannedChunk],
    raw_region: &[u8],
    inhabited_time_threshold: u32,
) -> Result<(), ProcessError> {
    let file = std::fs::OpenOptions::new().write(true).open(region_path)?;

    let mut header = [0u8; 8192];
    let mut timestamps = [0u8; 4096];
    timestamps.copy_from_slice(&raw_region[4096..8192]);

    let mut body = Vec::with_capacity(raw_region.len());
    let mut next_sector = 2usize;

    for chunk in chunks {
        if chunk.inhabited_time <= i64::from(inhabited_time_threshold) {
            continue;
        }

        let meta = chunk.metadata;
        let compressed = raw_region.get(meta.start..meta.end).ok_or_else(|| {
            ProcessError::ChunkError(format!(
                "Chunk ({}, {}) points outside region data",
                meta.x, meta.z
            ))
        })?;

        let exact_len = compressed.len() + 1;
        let exact_len_u32 = u32::try_from(exact_len)
            .map_err(|_| ProcessError::ChunkError("Chunk payload too large".into()))?;
        let sector_count = exact_len.div_ceil(SECTOR_SIZE);
        if sector_count > 255 {
            warn!("Chunk ({}, {}) too large, skipping", meta.x, meta.z);
            continue;
        }

        body.extend_from_slice(&exact_len_u32.to_be_bytes());
        body.push(match meta.compression {
            CompressionType::GZip => 1,
            CompressionType::Zlib => 2,
        });
        body.extend_from_slice(compressed);
        body.resize(body.len() + (sector_count * SECTOR_SIZE - exact_len), 0);

        let idx = (meta.x + meta.z * REGION_SIZE) * 4;
        let location = (next_sector << 8) | sector_count;
        header[idx..idx + 4]
            .copy_from_slice(&u32::try_from(location).unwrap_or(u32::MAX).to_be_bytes());

        next_sector += sector_count;
    }

    // Write body first, harmless if we crash here, old header still points to valid data.
    let end = 8192 + body.len();
    file.write_all_at(&body, 8192)?;
    // Flush body to stable storage before writing the header (commit point).
    file.sync_data()?;

    // Header and timestamps act as the commit record, write them last.
    file.write_all_at(&header, 0)?;
    file.write_all_at(&timestamps, 4096)?;
    file.sync_data()?;

    // Truncate tail, idempotent, safe to replay after a crash.
    file.set_len(
        u64::try_from(end)
            .map_err(|_| ProcessError::RegionError("Region file too large to truncate".into()))?,
    )?;

    Ok(())
}
