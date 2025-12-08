# Install cargo-flamegraph

cargo install flamegraph

if ! command -v perf &> /dev/null; then
    echo "perf is not installed"
    exit 1
fi

mkdir -p benches/test_write
cp -r benches/test_data/* benches/test_write

cargo flamegraph --bin rori --dev -i --flamechart -- "benches/test_write"
xdg-open flamegraph.svg
