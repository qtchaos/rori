use criterion::{Criterion, black_box, criterion_group, criterion_main};
use flate2::read::ZlibDecoder;
use std::fs;
use std::io::Read;

use rori::nbt::{NbtReader, NbtTag};

/// Load and decompress a sample chunk from a region file for benchmarking
fn load_sample_chunk() -> Vec<u8> {
    // Read a region file
    let region_path = "benches/test_data/region_small/r.0.0.mca";
    let data = fs::read(region_path).expect("Failed to read region file");

    // Parse the first valid chunk we find
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
        if compression_type != 2 {
            continue; // Only use Zlib compressed chunks
        }

        let compressed_data = &data[byte_offset + 5..byte_offset + 4 + length];

        // Decompress the chunk
        let mut decoder = ZlibDecoder::new(compressed_data);
        let mut decompressed = Vec::new();
        decoder
            .read_to_end(&mut decompressed)
            .expect("Failed to decompress");

        return decompressed;
    }

    panic!("No valid chunks found in sample region file");
}

fn bench_nbt_reader_search(c: &mut Criterion) {
    let chunk_data = load_sample_chunk();

    c.bench_function("nbt_search_inhabited_time", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&chunk_data));

            // Read root compound tag
            let tag_type = NbtTag::from_u8(reader.read_u8().unwrap()).unwrap();
            assert_eq!(tag_type, NbtTag::Compound);

            // Skip root tag name
            reader.skip_string().unwrap();

            // Search for InhabitedTime
            const INHABITED_TIME: &[u8] = b"InhabitedTime";
            // pass an empty byte slice as the second argument (no starting field)
            reader.search_compound_for_field(INHABITED_TIME, b"").unwrap()
        });
    });
}

fn bench_nbt_reader_primitives(c: &mut Criterion) {
    // Create test data with various NBT primitives
    let mut test_data = Vec::new();

    // Add some big-endian integers
    test_data.extend_from_slice(&42i16.to_be_bytes());
    test_data.extend_from_slice(&12345i32.to_be_bytes());
    test_data.extend_from_slice(&9876543210i64.to_be_bytes());
    test_data.extend_from_slice(&3.14f32.to_be_bytes());
    test_data.extend_from_slice(&2.718281828f64.to_be_bytes());

    c.bench_function("nbt_read_i16", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&test_data));
            reader.read_i16_be().unwrap()
        });
    });

    c.bench_function("nbt_read_i32", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&test_data[2..]));
            reader.read_i32_be().unwrap()
        });
    });

    c.bench_function("nbt_read_i64", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&test_data[6..]));
            reader.read_i64_be().unwrap()
        });
    });

    c.bench_function("nbt_read_f32", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&test_data[14..]));
            reader.read_f32_be().unwrap()
        });
    });

    c.bench_function("nbt_read_f64", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&test_data[18..]));
            reader.read_f64_be().unwrap()
        });
    });
}

fn bench_nbt_string_matching(c: &mut Criterion) {
    // Test string matching with different lengths
    let short_string = b"InhabitedTime";
    let long_string = b"VeryLongFieldNameForTestingStringMatchingPerformance";

    // Create NBT string format: length (u16 BE) + data
    let mut short_nbt = Vec::new();
    short_nbt.extend_from_slice(&(short_string.len() as u16).to_be_bytes());
    short_nbt.extend_from_slice(short_string);

    let mut long_nbt = Vec::new();
    long_nbt.extend_from_slice(&(long_string.len() as u16).to_be_bytes());
    long_nbt.extend_from_slice(long_string);

    c.bench_function("nbt_string_match_short", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&short_nbt));
            reader.is_string_match(black_box(short_string)).unwrap()
        });
    });

    c.bench_function("nbt_string_match_long", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&long_nbt));
            reader.is_string_match(black_box(long_string)).unwrap()
        });
    });

    c.bench_function("nbt_string_match_mismatch", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&short_nbt));
            reader
                .is_string_match(black_box(b"DifferentField"))
                .unwrap()
        });
    });
}

fn bench_nbt_skip_operations(c: &mut Criterion) {
    let chunk_data = load_sample_chunk();

    c.bench_function("nbt_skip_compound", |b| {
        b.iter(|| {
            let mut reader = NbtReader::new(black_box(&chunk_data));

            // Read root tag
            let tag_type = NbtTag::from_u8(reader.read_u8().unwrap()).unwrap();
            assert_eq!(tag_type, NbtTag::Compound);
            reader.skip_string().unwrap();

            // Skip the entire compound
            reader.skip_compound().unwrap()
        });
    });
}

criterion_group! {
    name = nbt_benches;
    config = Criterion::default().sample_size(100);
    targets =
        bench_nbt_reader_search,
        bench_nbt_reader_primitives,
        bench_nbt_string_matching,
        bench_nbt_skip_operations
}
criterion_main!(nbt_benches);
