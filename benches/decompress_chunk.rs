use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use std::fs;

use rori::{decompress_chunk, decompress_full, CompressionType};

/// Load compressed chunk data from a region file
fn load_compressed_chunks() -> Vec<(String, Vec<u8>, u8)> {
    let region_path = "benches/test_data/region_small/r.0.0.mca";
    let data = fs::read(region_path).expect("Failed to read region file");

    let mut chunks = Vec::new();

    // Parse region header and extract compressed chunks
    for idx in 0..1024 {
        let header_offset = idx * 4;
        if header_offset + 4 > data.len() {
            continue;
        }

        let raw = u32::from_be_bytes([
            data[header_offset],
            data[header_offset + 1],
            data[header_offset + 2],
            data[header_offset + 3],
        ]);
        let sector = raw >> 8;
        if sector == 0 {
            continue;
        }

        let byte_offset = (sector as usize) * 4096;
        if byte_offset + 4 > data.len() {
            continue;
        }

        let length = u32::from_be_bytes([
            data[byte_offset],
            data[byte_offset + 1],
            data[byte_offset + 2],
            data[byte_offset + 3],
        ]) as usize;

        if length == 0 || byte_offset + 4 + length > data.len() {
            continue;
        }

        let compression_type = data[byte_offset + 4];
        let compressed_data = data[byte_offset + 5..byte_offset + 4 + length].to_vec();

        let name = match compression_type {
            1 => format!(
                "gzip_{}kb_chunk{}",
                compressed_data.len() / 1024,
                chunks.len()
            ),
            2 => format!(
                "zlib_{}kb_chunk{}",
                compressed_data.len() / 1024,
                chunks.len()
            ),
            _ => continue,
        };

        chunks.push((name, compressed_data, compression_type));

        // Collect a few chunks of different types
        if chunks.len() >= 10 {
            break;
        }
    }

    chunks
}

fn bench_decompress_full(c: &mut Criterion) {
    let chunks = load_compressed_chunks();
    
    let mut group = c.benchmark_group("decompress_full");
    
    for (name, compressed_data, compression_type) in &chunks {
        let compression = CompressionType::from_byte(*compression_type).unwrap();
        
        group.bench_with_input(
            BenchmarkId::from_parameter(name),
            compressed_data,
            |b, data| {
                b.iter(|| {
                    black_box(decompress_full(compression, black_box(data)).unwrap())
                });
            },
        );
    }
    
    group.finish();
}fn bench_decompress_partial(c: &mut Criterion) {
    let chunks = load_compressed_chunks();
    
    // Test different partial decompression sizes
    let partial_sizes = [512, 1024, 2048, 4096];
    
    for partial_size in partial_sizes {
        let mut group = c.benchmark_group(format!("decompress_partial_{}b", partial_size));
        
        for (name, compressed_data, compression_type) in &chunks {
            let compression = CompressionType::from_byte(*compression_type).unwrap();
            
            group.bench_with_input(
                BenchmarkId::from_parameter(name),
                compressed_data,
                |b, data| {
                    b.iter(|| {
                        black_box(decompress_chunk(compression, black_box(data), partial_size).unwrap())
                    });
                },
            );
        }
        
        group.finish();
    }
}

fn bench_partial_vs_full_comparison(c: &mut Criterion) {
    let chunks = load_compressed_chunks();
    
    let mut group = c.benchmark_group("partial_vs_full_comparison");
    
    for (name, compressed_data, compression_type) in &chunks {
        let compression = CompressionType::from_byte(*compression_type).unwrap();
        
        // Full decompression
        group.bench_with_input(
            BenchmarkId::new("full", name),
            compressed_data,
            |b, data| {
                b.iter(|| {
                    black_box(decompress_full(compression, black_box(data)).unwrap())
                });
            },
        );
        
        // Partial decompression (1KB - your current default)
        group.bench_with_input(
            BenchmarkId::new("partial_1kb", name),
            compressed_data,
            |b, data| {
                b.iter(|| {
                    black_box(decompress_chunk(compression, black_box(data), 1024).unwrap())
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