use criterion::{Criterion, criterion_group, criterion_main};

use log::warn;
use rori::process_directory;

fn bench_dry_process_directory(c: &mut Criterion) {
    let path = std::path::Path::new("benches/test_data/region_small");
    let dry_run = true;
    let inhabited_time = 100;

    // Benchmarks are subjective to the current system and its capabilities.
    rayon::ThreadPoolBuilder::new()
        .num_threads(num_cpus::get())
        .build_global()
        .unwrap_or_else(|e| {
            warn!("Failed to set thread pool size: {}, using default", e);
        });

    c.bench_function("process_directory", |b| {
        b.iter(|| {
            process_directory(path, dry_run, inhabited_time, false).unwrap();
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench_dry_process_directory
}
criterion_main!(benches);
