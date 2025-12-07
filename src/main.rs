use std::{path::PathBuf, process};

use clap::Parser;
use log::{debug, error, info, warn};
use rori::{Conf, process_directory};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to directory containing .mca files
    path: PathBuf,

    /// Enable dry run mode, which only simulates processing without making changes
    #[arg(long)]
    dry_run: bool,

    /// Enable verbose output (-v, -vv for more verbosity)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Number of threads to use for parallel processing
    #[arg(short, long, default_value_t = num_cpus::get())]
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

    let result = match process_directory(&args.path, &config) {
        Ok(result) => result,
        Err(e) => {
            error!("Processing failed: {e}");
            process::exit(1);
        }
    };

    let duration = start.elapsed();
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

    let chunks_per_second =
        result.total_chunk_stats.total as f64 / duration.as_millis() as f64 * 1000.0;

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
