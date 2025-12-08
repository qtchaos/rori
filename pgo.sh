# https://kobzol.github.io/rust/cargo/2023/07/28/rust-cargo-pgo.html

# Initially build with PGO enabled
cargo pgo build -- --bin rori

# Copy regions so we can run write benchmarks
mkdir -p benches/test_write
cp -r benches/test_data/* benches/test_write

# Gather data for PGO
./target/x86_64-unknown-linux-gnu/release/rori -- benches/test_write

# Clean up the test data after running benchmarks
rm -rf benches/test_write/*

# Build instrumented binary with BOLT+PGO
cargo pgo bolt build --with-pgo -- --bin rori

mkdir -p benches/test_write

# Copy regions again for the next benchmark
cp -r benches/test_data/* benches/test_write

# Run benchmarks for BOLT
./target/x86_64-unknown-linux-gnu/release/rori-bolt-instrumented -- benches/test_write

# Clean up the test data after running benchmarks
rm -rf benches/test_write

# Optimize the binary with BOLT and PGO
cargo pgo bolt optimize --with-pgo -- --bin rori
