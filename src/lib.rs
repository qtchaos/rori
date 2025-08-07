mod parser;

use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, error, info, trace, warn};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use std::{
    fs,
    io::BufReader,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Debug)]
pub enum ProcessError {
    IoError(std::io::Error),
    RegionError(String),
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
    total_chunks: u32,
    inhabited_chunks: u32,
}

impl ChunkStats {
    fn merge(&mut self, other: ChunkStats) {
        self.total_chunks += other.total_chunks;
        self.inhabited_chunks += other.inhabited_chunks;
    }
}

#[derive(Debug, Default)]
struct RegionStats {
    total_regions: u32,
    deleted_regions: u32,
    chunk_stats: ChunkStats,
}

impl RegionStats {
    fn merge(&mut self, other: RegionStats) {
        self.total_regions += other.total_regions;
        self.deleted_regions += other.deleted_regions;
        self.chunk_stats.merge(other.chunk_stats);
    }
}

pub fn process_directory(
    path: &Path,
    dry_run: bool,
    inhabited_time: u32,
    delete_regions: bool,
) -> Result<(), ProcessError> {
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
        return Ok(());
    }

    let pb = ProgressBar::new(regions.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("[{elapsed_precise}] [{bar:40}] {pos}/{len} {msg}").unwrap(),
    );

    // Process regions in parallel
    let results: Vec<Result<RegionStats, ProcessError>> = regions
        .par_iter()
        .map(|region_path| {
            let res = process_region(region_path, dry_run, inhabited_time, delete_regions);
            pb.inc(1);
            res
        })
        .collect();

    pb.finish_with_message("done");

    // Aggregate results
    let mut total_stats = RegionStats::default();
    for result in results {
        match result {
            Ok(stats) => total_stats.merge(stats),
            Err(e) => {
                error!("Region processing error: {}", e);
            }
        }
    }

    let inhabited_percentage = if total_stats.chunk_stats.total_chunks > 0 {
        (total_stats.chunk_stats.inhabited_chunks as f64)
            / total_stats.chunk_stats.total_chunks as f64
            * 100.0
    } else {
        0.0
    };

    info!(
        "Total processed: {} regions, {} chunks",
        total_stats.total_regions, total_stats.chunk_stats.total_chunks
    );
    info!(
        "Inhabited chunks: {} ({}%)",
        total_stats.chunk_stats.inhabited_chunks, inhabited_percentage
    );
    Ok(())
}

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

fn process_region(
    region_path: &Path,
    dry_run: bool,
    threshold: u32,
    delete_regions: bool,
) -> Result<RegionStats, ProcessError> {
    trace!("Processing region: {}", region_path.display());

    let file = fs::File::open(region_path)?;
    let reader = BufReader::new(file);

    let mut chunks: Vec<Vec<Option<Vec<u8>>>> = vec![vec![None; 32]; 32];
    let mut mca = fastanvil::Region::from_stream(reader).map_err(|e| {
        ProcessError::RegionError(format!(
            "Failed to create region from {}: {}",
            region_path.display(),
            e
        ))
    })?;

    let mut chunk_stats = ChunkStats::default();
    let mut deleted_count = 0;

    // First pass: determine which chunks to keep
    for x in 0..32 {
        for z in 0..32 {
            if let Ok(Some(chunk_data)) = mca.read_chunk(x, z) {
                chunk_stats.total_chunks += 1;

                let inhabited_time = parser::process_chunk(&chunk_data).map_err(|e| {
                    ProcessError::ChunkError(format!("Failed to process chunk: {}", e))
                })?;

                if inhabited_time.is_some() && inhabited_time.unwrap() > threshold as i64 {
                    chunk_stats.inhabited_chunks += 1;
                    chunks[x][z] = Some(chunk_data.clone());
                } else {
                    deleted_count += 1;
                }
            }
        }
    }

    let mut region_stats = RegionStats {
        total_regions: 1,
        deleted_regions: 0,
        chunk_stats: ChunkStats::default(),
    };
    region_stats.chunk_stats.merge(chunk_stats);

    if delete_regions {
        // In region deletion mode, delete the entire region if no inhabited chunks
        if region_stats.chunk_stats.inhabited_chunks == 0
            && region_stats.chunk_stats.total_chunks > 0
        {
            if !dry_run {
                fs::remove_file(region_path)?;
                debug!("Deleted region file: {}", region_path.display());
            } else {
                debug!("Would delete region file: {}", region_path.display());
            }
            region_stats.deleted_regions = 1;
        }
    } else {
        // In chunk deletion mode, rebuild the region with only inhabited chunks
        if !dry_run && deleted_count > 0 {
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

            // Re-read and write only the chunks we want to keep
            for x in 0..32 {
                for z in 0..32 {
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
