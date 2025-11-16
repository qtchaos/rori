use std::{path::PathBuf, process};

use clap::Parser;
use log::{debug, error, info, warn};
use rori::{ProcessingOptions, process_directory};

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
    /// Chunks with InhabitedTime below or equal to this threshold will be deleted.
    #[arg(short, long, default_value_t = 100)]
    inhabited_time: u32,

    /// Delete entire regions instead of individual chunks when no inhabited chunks exist
    #[arg(long)]
    delete_regions: bool,

    /// Max bytes to decompress per chunk (0 = full, auto-fallback on parse failure)
    #[arg(long, default_value_t = 512)]
    decomp_size: usize,
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
        eprintln!("Failed to initialize logging: {}", e);
        process::exit(1);
    }

    // Set thread pool size
    rayon::ThreadPoolBuilder::new()
        .num_threads(args.threads)
        .build_global()
        .unwrap_or_else(|e| {
            warn!("Failed to set thread pool size: {}, using default", e);
        });

    debug!(
        "Using {} threads w/SIMD {}",
        args.threads,
        is_x86_feature_detected!("sse")
    );

    // Start timing
    let start = std::time::Instant::now();

    let options = ProcessingOptions {
        dry_run: args.dry_run,
        inhabited_time_threshold: args.inhabited_time,
        delete_entire_regions: args.delete_regions,
        max_decompression_bytes: args.decomp_size,
    };

    let result = match process_directory(&args.path, &options) {
        Ok(result) => result,
        Err(e) => {
            error!("Processing failed: {}", e);
            process::exit(1);
        }
    };

    let duration = start.elapsed();

    info!(
        "Total processed: {} regions, {} chunks",
        result.total_regions, result.total_chunks
    );

    let inhabited_percentage = if result.total_chunks > 0 {
        (result.inhabited_chunks as f64) / result.total_chunks as f64 * 100.0
    } else {
        0.0
    };

    info!(
        "Inhabited chunks: {} ({:.1}%)",
        result.inhabited_chunks, inhabited_percentage
    );

    if result.deleted_regions > 0 {
        info!("Deleted regions: {}", result.deleted_regions);
    }

    // Report InhabitedTime position statistics
    if result.position_count > 0 {
        debug!(
            "InhabitedTime position stats: min={}B, max={}B, avg={}B (n={})",
            result.min_position, result.max_position, result.avg_position, result.position_count
        );
    }

    info!("Processing completed in {:.2?}", duration);
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
