# rori

A highly multithreaded tool for deleting inactive chunks or regions in Minecraft worlds, focusing on performance and efficiency, designed to be used for large-scale world management.

## Benchmarks

These figures should be taken as a rough estimate, as performance can vary based on system configuration. I used a Ryzen 5 5500 (6 cores, 12 threads) with 32GB of RAM and an NVMe SSD capable of 7000 MB/s read and 5000 MB/s write speeds.

### 1600 regions (1620529 chunks)
bold = current version

| Tool               | Time (s) | Dry Run Time (s) | Threads | Chunks per Second |
| ------------------ | -------- | ---------------- | ------- | ----------------- |
| **rori 0.3 (regions)** | 3.98     | 4.13         | 12      | 407168            |
| **rori 0.3**           | 4.06     | 4.35         | 12      | 399145            |
| rori 0.2 (regions) | 6.7      | 6.01             | 12      | 241870            |
| rori 0.2           | 6.78     | 5.82             | 12      | 239016            |
| **rori 0.3 (4 threads)**| 8.86     | 8.94        | **4**   | 182903             |
| thanos             | 16.3     | N/A              | 4       | 99418             |
| **rori 0.3 (1 thread)**| 36     | 33             | **1**   | 49000             |
| PotatoPeeler       | 126.7    | 137.5            | 12      | 12790             |
| ChunkCleaner       | N/A      | 610              | N/A     | 2656              |

## Chunk vs Region Mode

<img src="https://raw.githubusercontent.com/qtchaos/rori/refs/heads/master/.github/assets/chunks.png" alt="Chunk Mode" width="200">
<img src="https://raw.githubusercontent.com/qtchaos/rori/refs/heads/master/.github/assets/regions.png" alt="Region Mode" width="200">

## Usage

### Basic Usage

Process an entire world (all dimensions):
```bash
rori /path/to/world --dry-run
```

Process a specific region directory:
```bash
rori /path/to/world/region --dry-run
```

### Dimension Selection

Process only specific dimensions:
```bash
# Process only the Overworld
rori /path/to/world --dimension overworld

# Process only the Nether
rori /path/to/world --dimension nether

# Process only the End
rori /path/to/world --dimension end

# Process all dimensions (default)
rori /path/to/world --dimension all
```

### Advanced Examples

```bash
# Process all dimensions with custom inhabited time threshold
rori /path/to/world --inhabited-time 200 --dimension all

# Delete regions instead of individual chunks in the Nether
rori /path/to/world --dimension nether --delete-regions

# Process only Overworld with 8 threads
rori /path/to/world --dimension overworld --threads 8
```

## Arguments

| Argument                | Default           | Description                                                                       |
| ----------------------- | ----------------- | --------------------------------------------------------------------------------- |
| `-h`                    |                   | See the arguments and their usage                                                 |
| `-v` `--verbose`        |                   | Increased verbosity, `-vv` for traces                                             |
| `--dry-run`             | `false`           | Enable dry run mode, which only simulates processing without making changes       |
| `-t` `--threads`        | `num_cpus::get()` | Number of threads to use for processing regions in parallel                       |
| `-i` `--inhabited-time` | `100`             | Chunks with InhabitedTime below or equal to this threshold will be deleted        |
| `--delete-regions`      | `false`           | Delete entire regions instead of individual chunks when no inhabited chunks exist |
| `-d` `--dimension`      | `all`             | Which dimension(s) to process: `all`, `overworld`, `nether`, or `end`            |
| `--no-progress`         | `false`           | Disable progress bar                                                              |

## Building

Install Rust (nightly) and Cargo, then run:
`cargo build --release`

To build the PGO + BOLT optimized version, run:
`./pgo.sh`

## Disclaimer

Although I have tested this tool on various Minecraft versions (1.21+), I cannot guarantee that it will work flawlessly on all versions. Do your own testing and ensure you have backups of your worlds before running this tool, I am not responsible for any data loss or corruption that may occur.
