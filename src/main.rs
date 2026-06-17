use std::{path::PathBuf, process};

use clap::{Parser, ValueEnum};
use log::{debug, error, info, warn};
use rori::{Conf, Dimension, WorldFormat, process_world_with_format};

#[derive(Debug, Clone, ValueEnum)]
enum FormatArg {
    /// Auto-detect world format
    Auto,
    /// Pre-1.26 legacy layout (region/, DIM-1/, DIM1/)
    Legacy,
    /// 1.26+ modern layout (dimensions/minecraft/.../region/)
    Modern,
}

impl FormatArg {
    const fn into_world_format(self) -> Option<WorldFormat> {
        match self {
            Self::Auto => None,
            Self::Legacy => Some(WorldFormat::Legacy),
            Self::Modern => Some(WorldFormat::Modern),
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
enum DimensionArg {
    /// Process all dimensions (overworld, nether, end)
    All,
    /// Process only the overworld
    Overworld,
    /// Process only the nether (DIM-1)
    Nether,
    /// Process only the end (DIM1)
    End,
}

impl From<DimensionArg> for Vec<Dimension> {
    fn from(arg: DimensionArg) -> Self {
        match arg {
            DimensionArg::All => vec![Dimension::Overworld, Dimension::Nether, Dimension::End],
            DimensionArg::Overworld => vec![Dimension::Overworld],
            DimensionArg::Nether => vec![Dimension::Nether],
            DimensionArg::End => vec![Dimension::End],
        }
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to world directory (will auto-detect dimensions) or specific region directory
    path: PathBuf,

    /// Enable dry run mode, which only simulates processing without making changes
    #[arg(long)]
    dry_run: bool,

    /// Enable verbose output (-v, -vv for more verbosity)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Number of threads to use for parallel processing
    #[arg(short, long, default_value_t = std::thread::available_parallelism().map_or(1, std::num::NonZero::get))]
    threads: usize,

    /// The cumulative number of ticks players have been in a chunk.
    /// Chunks with `InhabitedTime` below or equal to this threshold will be deleted.
    #[arg(short, long, default_value_t = 100)]
    inhabited_time: u32,

    /// If true, delete region instead of chunks
    #[arg(long)]
    delete_regions: bool,

    /// No progress bar
    #[arg(long)]
    no_progress: bool,

    /// Which dimension(s) to process. Only applies if path is a world directory.
    #[arg(short, long, value_enum, default_value = "all")]
    dimension: DimensionArg,

    /// World storage format. 'auto' detects based on directory layout.
    #[arg(long, value_enum, default_value = "auto")]
    format: FormatArg,
}

fn main() {
    let args = Args::parse();

    // Validate path early
    if !args.path.exists() {
        eprintln!(
            "Error: The specified path '{}' does not exist.",
            args.path.display()
        );
        process::exit(1);
    }

    if !args.path.is_dir() {
        eprintln!(
            "Error: The specified path '{}' is not a directory.",
            args.path.display()
        );
        process::exit(1);
    }

    // Initialize logging
    if let Err(e) = init_logging(args.verbose) {
        eprintln!("Failed to initialize logging: {e}");
        process::exit(1);
    }

    // Set thread pool size
    rayon::ThreadPoolBuilder::new()
        .num_threads(args.threads)
        .build_global()
        .unwrap_or_else(|e| {
            warn!("Failed to set thread pool size: {e}, using default");
        });

    debug!(
        "Using {} threads w/SIMD {}",
        args.threads,
        is_x86_feature_detected!("sse")
    );

    // Start timing
    let start = std::time::Instant::now();

    let config = Conf {
        dry_run: args.dry_run,
        inhabited_time_threshold: args.inhabited_time,
        delete_regions: args.delete_regions,
        no_progress: args.no_progress,
    };

    let dimensions: Vec<Dimension> = args.dimension.into();

    let world_format = args.format.into_world_format();
    let result = match process_world_with_format(&args.path, &config, &dimensions, world_format) {
        Ok(result) => result,
        Err(e) => {
            error!("Processing failed: {e}");
            process::exit(1);
        }
    };

    let duration = start.elapsed();

    // Print per-dimension statistics
    for dim_result in &result.dimension_results {
        info!(
            "{}: {} regions, {} chunks ({} inhabited, {:.1}%)",
            dim_result.dimension.name(),
            dim_result.total_regions,
            dim_result.total_chunk_stats.total,
            dim_result.total_chunk_stats.inhabited,
            if dim_result.total_chunk_stats.total > 0 {
                (f64::from(dim_result.total_chunk_stats.inhabited))
                    / f64::from(dim_result.total_chunk_stats.total)
                    * 100.0
            } else {
                0.0
            }
        );

        if dim_result.deleted_regions > 0 {
            info!("  Deleted regions: {}", dim_result.deleted_regions);
        }
    }

    // Print totals
    info!("===== TOTAL =====");
    info!(
        "Total processed: {} regions, {} chunks",
        result.total_regions, result.total_chunk_stats.total
    );

    let inhabited_percentage = if result.total_chunk_stats.total > 0 {
        (f64::from(result.total_chunk_stats.inhabited)) / f64::from(result.total_chunk_stats.total)
            * 100.0
    } else {
        0.0
    };

    info!(
        "Inhabited chunks: {} ({:.1}%)",
        result.total_chunk_stats.inhabited, inhabited_percentage
    );

    if result.deleted_regions > 0 {
        info!("Deleted regions: {}", result.deleted_regions);
    }

    let chunks_per_second = f64::from(result.total_chunk_stats.total) / duration.as_secs_f64();

    info!("Completed in {duration:.2?}, {chunks_per_second:.0} chunks per second");
}

fn init_logging(verbose: u8) -> Result<(), Box<dyn std::error::Error>> {
    let log_level = match verbose {
        0 => log::LevelFilter::Info,
        1 => log::LevelFilter::Debug,
        2 => log::LevelFilter::Trace,
        _ => {
            eprintln!("Error: Maximum verbosity level is 2 (-vv)");
            process::exit(1);
        }
    };

    env_logger::Builder::new()
        .filter_level(log_level)
        .format_timestamp(None)
        .try_init()?;

    Ok(())
}
