#![allow(clippy::significant_drop_tightening)]
use std::path::PathBuf;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

use rori::{
    Region,
    decompress::{decompress, decompress_partial, mmap_region},
};

/// Load compressed chunk data from a region file
fn load_region() -> Option<Region> {
    mmap_region(
        &PathBuf::from("benches/test_data/region_small/r.0.0.mca"),
        1,
    )
    .ok()
}

fn bench_decompress_full(c: &mut Criterion) {
    let Some(region) = load_region() else {
        eprintln!("Skipping bench_decompress_full: test data not found");
        return;
    };

    let Some(Some(chunk)) = region.chunks.first().and_then(|row| row.first()) else {
        eprintln!("Skipping bench_decompress_full: no chunks in test data");
        return;
    };

    let mut group = c.benchmark_group("decompress_full");

    if let Some(ref compressed_data) = chunk.data {
        let name = format!("chunk_{}_{}", 0, 0);
        let compression = region.compression;

        group.bench_with_input(
            BenchmarkId::from_parameter(&name),
            compressed_data,
            |b, data| {
                b.iter(|| {
                    black_box(
                        decompress(compression, black_box(data)).expect("bench decompress_full"),
                    )
                });
            },
        );
    }

    group.finish();
}
fn bench_decompress_partial(c: &mut Criterion) {
    let Some(region) = load_region() else {
        eprintln!("Skipping bench_decompress_partial: test data not found");
        return;
    };

    let Some(Some(chunk)) = region.chunks.first().and_then(|row| row.first()) else {
        eprintln!("Skipping bench_decompress_partial: no chunks in test data");
        return;
    };

    // Test different partial decompression sizes
    let partial_sizes = [512, 1024, 2048, 4096, 8192];

    for partial_size in partial_sizes {
        let mut group = c.benchmark_group(format!("decompress_partial_{partial_size}b"));

        if let Some(ref compressed_data) = chunk.data {
            let name = format!("chunk_{}_{}", 0, 0);
            let compression = region.compression;

            group.bench_with_input(
                BenchmarkId::from_parameter(&name),
                compressed_data,
                |b, data| {
                    b.iter(|| {
                        black_box(
                            decompress_partial(compression, black_box(data), partial_size)
                                .expect("bench decompress_partial"),
                        )
                    });
                },
            );
        }

        group.finish();
    }
}

fn bench_partial_vs_full_comparison(c: &mut Criterion) {
    let Some(region) = load_region() else {
        eprintln!("Skipping bench_partial_vs_full_comparison: test data not found");
        return;
    };

    let Some(Some(chunk)) = region.chunks.first().and_then(|row| row.first()) else {
        eprintln!("Skipping bench_decompress_partial: no chunks in test data");
        return;
    };

    let mut group = c.benchmark_group("partial_vs_full_comparison");

    if let Some(ref compressed_data) = chunk.data {
        let name = format!("chunk_{}_{}", 0, 0);
        let compression = region.compression;

        // Full decompression
        group.bench_with_input(
            BenchmarkId::new("full", &name),
            compressed_data,
            |b, data| {
                b.iter(|| {
                    black_box(
                        decompress(compression, black_box(data))
                            .expect("bench partial_vs_full: full"),
                    )
                });
            },
        );

        // Partial decompression
        group.bench_with_input(
            BenchmarkId::new("partial_512b", &name),
            compressed_data,
            |b, data| {
                b.iter(|| {
                    black_box(
                        decompress_partial(compression, black_box(data), 512)
                            .expect("bench partial_vs_full: partial"),
                    )
                });
            },
        );
    }

    group.finish();
}

criterion_group! {
    name = decompress_benches;
    // Increase sample size for more stable results
    // Increase warm-up time to stabilize CPU frequency
    config = Criterion::default()
        .sample_size(200)           // More samples = better statistics (default: 100)
        .measurement_time(std::time::Duration::from_secs(10))  // Longer measurement (default: 5s)
        .warm_up_time(std::time::Duration::from_secs(3));      // Longer warm-up (default: 3s)
    targets =
        bench_decompress_full,
        bench_decompress_partial,
        bench_partial_vs_full_comparison
}
criterion_main!(decompress_benches);
