#![allow(clippy::significant_drop_tightening)]
use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::path::PathBuf;

use rori::{
    decompress::{decompress, mmap_region},
    nbt::{NbtReader, NbtTag},
};

fn load_sample_chunk() -> Vec<u8> {
    // Load a real chunk from test data if available, otherwise use empty vec
    mmap_region(
        &PathBuf::from("benches/test_data/region_small/r.0.0.mca"),
        1,
    )
    .ok()
    .and_then(|region| {
        let compression = region.compression;
        region
            .chunks
            .iter()
            .flat_map(|row| row.iter())
            .find_map(|chunk| {
                chunk.as_ref().and_then(|c| {
                    c.data
                        .as_ref()
                        .and_then(|compressed_data| decompress(compression, compressed_data).ok())
                })
            })
    })
    .unwrap_or_default()
}

fn bench_nbt_reader_search(c: &mut Criterion) {
    const INHABITED_TIME: &[u8] = b"InhabitedTime";

    let chunk_data = load_sample_chunk();

    if chunk_data.is_empty() {
        return;
    }

    c.bench_function("nbt_search_inhabited_time", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&chunk_data));

            // Read root compound tag
            let tag_type = NbtTag::from_u8(reader.read_u8().expect("read tag byte")).expect("parse tag");
            assert_eq!(tag_type, NbtTag::Compound);

            // Skip root tag name
            reader.skip_string().expect("skip root name");

            // Search for InhabitedTime
            reader
                .search_compound_for_field(INHABITED_TIME, black_box(&chunk_data))
                .expect("find InhabitedTime")
        });
    });
}

fn bench_nbt_reader_primitives(c: &mut Criterion) {
    // Create test data with various NBT primitives
    let mut test_data = Vec::new();

    // Add some big-endian integers
    test_data.extend_from_slice(&42i16.to_be_bytes());
    test_data.extend_from_slice(&12345i32.to_be_bytes());
    test_data.extend_from_slice(&9_876_543_210_i64.to_be_bytes());
    test_data.extend_from_slice(&std::f32::consts::PI.to_be_bytes());
    test_data.extend_from_slice(&std::f64::consts::E.to_be_bytes());

    c.bench_function("nbt_read_i16", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&test_data));
            reader.read_i16_be().expect("read i16")
        });
    });

    c.bench_function("nbt_read_i32", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&test_data[2..]));
            reader.read_i32_be().expect("read i32")
        });
    });

    c.bench_function("nbt_read_i64", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&test_data[6..]));
            reader.read_i64_be().expect("read i64")
        });
    });

    c.bench_function("nbt_read_f32", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&test_data[14..]));
            reader.read_f32_be().expect("read f32")
        });
    });

    c.bench_function("nbt_read_f64", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&test_data[18..]));
            reader.read_f64_be().expect("read f64")
        });
    });
}

fn bench_nbt_skip_operations(c: &mut Criterion) {
    let Ok(region_data) = mmap_region(&PathBuf::from("benches/test_data"), 1) else {
        return;
    };

    let compression = region_data.compression;

    // Find a chunk with data and decompress it
    let chunk_data = region_data
        .chunks
        .iter()
        .flat_map(|row| row.iter())
        .find_map(|chunk| {
            chunk.as_ref().and_then(|c| {
                c.data
                    .as_ref()
                    .and_then(|compressed_data| decompress(compression, compressed_data).ok())
            })
        });

    let Some(chunk_data) = chunk_data else {
        return;
    };

    c.bench_function("nbt_skip_compound", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&chunk_data));

            // Read root tag
            let tag_type = NbtTag::from_u8(reader.read_u8().expect("read tag byte")).expect("parse tag");
            assert_eq!(tag_type, NbtTag::Compound);
            reader.skip_string().expect("skip root name");

            // Skip the entire compound
            reader.skip_compound().expect("skip compound");
        });
    });
}

criterion_group! {
    name = nbt_benches;
    config = Criterion::default().sample_size(100);
    targets =
        bench_nbt_reader_search,
        bench_nbt_reader_primitives,
        bench_nbt_skip_operations
}
criterion_main!(nbt_benches);
