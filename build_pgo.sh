# https://kobzol.github.io/rust/cargo/2023/07/28/rust-cargo-pgo.html

# Initially build with PGO enabled
cargo pgo build

# Copy region_large and region_small so we can run write benchmarks
mkdir -p benches/test_write
cp -r benches/test_data/region_small benches/test_write/region_small

# Gather data for PGO
./target/x86_64-unknown-linux-gnu/release/rori -- benches/test_write/region_small

# Clean up the test data after running benchmarks
rm -rf benches/test_write/region_small

# Build instrumented binary with BOLT+PGO
cargo pgo bolt build --with-pgo

# Copy region_small again for the next benchmark
cp -r benches/test_data/region_small benches/test_write/region_small

# Run benchmarks for BOLT
./target/x86_64-unknown-linux-gnu/release/rori-bolt-instrumented -- benches/test_write/region_small

# Clean up the test data after running benchmarks
rm -rf benches/test_write/region_small

# Optimize the binary with BOLT and PGO
cargo pgo bolt optimize --with-pgo