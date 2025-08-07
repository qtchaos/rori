use std::{path::PathBuf, process};

use clap::Parser;
use log::{debug, error, info, warn};
use rori::process_directory;

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

    /// The cumulative number of ticks players have been a chunk.
    #[arg(short, long, default_value_t = 100)]
    inhabited_time: u32,

    /// Delete entire regions instead of individual chunks when no inhabited chunks exist
    #[arg(long)]
    delete_regions: bool,
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

    if let Err(e) = process_directory(
        &args.path,
        args.dry_run,
        args.inhabited_time,
        args.delete_regions,
    ) {
        error!("Processing failed: {}", e);
        process::exit(1);
    }

    let duration = start.elapsed();
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
