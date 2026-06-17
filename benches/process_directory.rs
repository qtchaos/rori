#![allow(clippy::significant_drop_tightening)]
use criterion::{Criterion, criterion_group, criterion_main};

use log::warn;
use rori::{Conf, Dimension, process_world};

fn bench_dry_process_directory(c: &mut Criterion) {
    let path = std::path::Path::new("benches/test_data");
    let dry_run = true;
    let inhabited_time = 100;

    // Benchmarks are subjective to the current system and its capabilities.
    rayon::ThreadPoolBuilder::new()
        .num_threads(std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
        .build_global()
        .unwrap_or_else(|e| {
            warn!("Failed to set thread pool size: {e}, using default");
        });

    let options = Conf {
        dry_run,
        inhabited_time_threshold: inhabited_time,
        delete_regions: false,
        no_progress: true,
    };

    c.bench_function("process_directory_partial_1kb", |b| {
        b.iter(|| {
            process_world(
                path,
                &options,
                &[Dimension::Overworld, Dimension::Nether, Dimension::End],
            )
            .expect("bench process_directory_partial_1kb failed");
        });
    });

    c.bench_function("process_directory_full", |b| {
        b.iter(|| {
            process_world(
                path,
                &options,
                &[Dimension::Overworld, Dimension::Nether, Dimension::End],
            )
            .expect("bench process_directory_full failed");
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench_dry_process_directory
}
criterion_main!(benches);
