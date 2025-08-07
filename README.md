# rori

A highly multithreaded tool for deleting inactive chunks or regions in Minecraft worlds, focusing on performance and efficiency, designed to be used for large-scale world management.

## Benchmarks

These figures should be taken as a rough estimate, as performance can vary based on system configuration. I used a Ryzen 5 5500 (6 cores, 12 threads) with 32GB of RAM and an NVMe SSD capable of 7000 MB/s read and 5000 MB/s write speeds.

### 1600 regions (1620529 chunks)

| Tool           | Time (s) | Dry Run Time (s) | Threads | Chunks per Second |
| -------------- | -------- | ---------------- | ------- | ----------------- |
| rori (regions) | 10.15    | 12.15            | 12      | 159658            |
| rori           | 11.14    | 12.81            | 12      | 145469            |
| PotatoPeeler   | 126.7    | 137.5            | 12      | 12790             |
| ChunkCleaner   | N/A      | 610              | N/A     | 2656              |

## Chunk vs Region Mode

<img src="https://raw.githubusercontent.com/qtchaos/rori/refs/heads/master/.github/assets/chunks.png" alt="Chunk Mode" width="200">
<img src="https://raw.githubusercontent.com/qtchaos/rori/refs/heads/master/.github/assets/regions.png" alt="Region Mode" width="200">

## Arguments

| Argument                | Default           | Description                                                                       |
| ----------------------- | ----------------- | --------------------------------------------------------------------------------- |
| `-h`                    |                   | See the arguments and their usage                                                 |
| `-v` `--verbose`        |                   | Increased verbosity, `-vv` for traces                                             |
| `--dry-run`             | `false`           | Enable dry run mode, which only simulates processing without making changes       |
| `-t` `--threads`        | `num_cpus::get()` | Number of threads to use for processing regions in parallel                       |
| `-i` `--inhabited-time` | `100`             | The cumulative number of ticks players have been a chunk                          |
| `--delete-regions`      | `false`           | Delete entire regions instead of individual chunks when no inhabited chunks exist |

## Building

Install Rust (nightly) and Cargo, then run:
`cargo build --release`

To build the PGO + BOLT optimized version, run:
`./build_pgo.sh`

## Disclaimer

Although I have tested this tool on various Minecraft versions (1.21+), I cannot guarantee that it will work flawlessly on all versions. Do your own testing and ensure you have backups of your worlds before running this tool, I am not responsible for any data loss or corruption that may occur.
