use crate::ChunkStats;
use std::time::Duration;

#[derive(Debug, Default, Clone, Copy)]
pub struct StageTimings {
    pub mmap: Duration,
    pub parse_header: Duration,
    pub scan_chunks: Duration,
    pub partial_decompress: Duration,
    pub partial_nbt: Duration,
    pub stream_decompress: Duration,
    pub stream_nbt: Duration,
    pub full_decompress: Duration,
    pub full_nbt: Duration,
    pub payload_copy: Duration,
    pub assemble_region: Duration,
    pub filesystem: Duration,
    pub anvil_fallback: Duration,
    pub partial_hits: u32,
    pub stream_hits: u32,
    pub stream_misses: u32,
    pub partial_skips: u32,
    pub full_fallbacks: u32,
    pub full_hits: u32,
    pub scan_failures: u32,
    pub stream_output_bytes: u64,
}

impl StageTimings {
    pub(crate) fn merge(&mut self, other: Self) {
        self.mmap += other.mmap;
        self.parse_header += other.parse_header;
        self.scan_chunks += other.scan_chunks;
        self.partial_decompress += other.partial_decompress;
        self.partial_nbt += other.partial_nbt;
        self.stream_decompress += other.stream_decompress;
        self.stream_nbt += other.stream_nbt;
        self.full_decompress += other.full_decompress;
        self.full_nbt += other.full_nbt;
        self.payload_copy += other.payload_copy;
        self.assemble_region += other.assemble_region;
        self.filesystem += other.filesystem;
        self.anvil_fallback += other.anvil_fallback;
        self.partial_hits += other.partial_hits;
        self.stream_hits += other.stream_hits;
        self.stream_misses += other.stream_misses;
        self.partial_skips += other.partial_skips;
        self.full_fallbacks += other.full_fallbacks;
        self.full_hits += other.full_hits;
        self.scan_failures += other.scan_failures;
        self.stream_output_bytes = self
            .stream_output_bytes
            .saturating_add(other.stream_output_bytes);
    }

    pub(crate) fn wall_total(&self) -> Duration {
        self.mmap
            + self.parse_header
            + self.scan_chunks
            + self.assemble_region
            + self.filesystem
            + self.anvil_fallback
    }

    pub(crate) fn chunk_cpu_total(&self) -> Duration {
        self.partial_decompress
            + self.partial_nbt
            + self.stream_decompress
            + self.stream_nbt
            + self.full_decompress
            + self.full_nbt
            + self.payload_copy
    }

    pub(crate) fn decompress_total(&self) -> Duration {
        self.partial_decompress + self.stream_decompress + self.full_decompress
    }

    pub(crate) fn nbt_total(&self) -> Duration {
        self.partial_nbt + self.stream_nbt + self.full_nbt
    }
}

pub(crate) fn log_timing_summary(
    prefix: &str,
    extra: &str,
    chunks: ChunkStats,
    timings: StageTimings,
    level: log::Level,
) {
    if !log::log_enabled!(level) {
        return;
    }

    log::log!(
        level,
        "{prefix}: {extra} chunks={} inhabited={} {}",
        chunks.total,
        chunks.inhabited,
        timing_breakdown(timings, chunks.total)
    );
}

pub(crate) fn timing_breakdown(timings: StageTimings, chunk_count: u32) -> String {
    let wall = timings.wall_total();
    let chunk_cpu = timings.chunk_cpu_total();
    let decompress = timings.decompress_total();
    let nbt = timings.nbt_total();
    // ponytail: inlined pct/count_pct/avg_bytes, add helpers back if reused elsewhere
    let w = wall.as_secs_f64();
    let c = chunk_cpu.as_secs_f64();
    // ponytail: callers guarantee wall/chunk_cpu/chunk_count > 0
    let pct = |part: Duration| part.as_secs_f64() / w * 100.0;
    let cpct = |part: Duration| part.as_secs_f64() / c * 100.0;
    let cnt_pct = |part: u32| f64::from(part) / f64::from(chunk_count) * 100.0;
    let stream_out_avg = timings.stream_output_bytes
        / u64::from(timings.stream_hits.saturating_add(timings.stream_misses).max(1));

    format!(
        concat!(
            "wall(sum)={wall:.2?}: mmap={mmap:.2?} ({mmap_pct:.1}%), ",
            "header={header:.2?} ({header_pct:.1}%), scan_chunks={scan:.2?} ({scan_pct:.1}%), ",
            "assemble={assemble:.2?} ({assemble_pct:.1}%), fs={fs:.2?} ({fs_pct:.1}%), ",
            "anvil_fallback={anvil:.2?} ({anvil_pct:.1}%); ",
            "chunk_cpu(sum)={chunk_cpu:.2?}: decompress={decompress:.2?} ({decompress_pct:.1}%) ",
            "[partial={partial_decompress:.2?}, stream={stream_decompress:.2?}, full={full_decompress:.2?}], ",
            "nbt={nbt:.2?} ({nbt_pct:.1}%) [partial={partial_nbt:.2?}, stream={stream_nbt:.2?}, full={full_nbt:.2?}], ",
            "payload_copy={payload_copy:.2?} ({payload_copy_pct:.1}%); ",
            "scan_paths: partial={partial_hits} ({partial_hits_pct:.1}%), ",
            "stream={stream_hits} ({stream_hits_pct:.1}%), ",
            "stream_miss={stream_misses} ({stream_misses_pct:.1}%), ",
            "partial_skip={partial_skips} ({partial_skips_pct:.1}%), ",
            "stream_out_avg={stream_out_avg}B, ",
            "full={full_hits}/{full_fallbacks} hits/fallbacks ({full_fallbacks_pct:.1}%), ",
            "failures={scan_failures} ({scan_failures_pct:.1}%)"
        ),
        wall = wall,
        mmap = timings.mmap,
        mmap_pct = pct(timings.mmap),
        header = timings.parse_header,
        header_pct = pct(timings.parse_header),
        scan = timings.scan_chunks,
        scan_pct = pct(timings.scan_chunks),
        assemble = timings.assemble_region,
        assemble_pct = pct(timings.assemble_region),
        fs = timings.filesystem,
        fs_pct = pct(timings.filesystem),
        anvil = timings.anvil_fallback,
        anvil_pct = pct(timings.anvil_fallback),
        chunk_cpu = chunk_cpu,
        decompress = decompress,
        partial_decompress = timings.partial_decompress,
        stream_decompress = timings.stream_decompress,
        full_decompress = timings.full_decompress,
        decompress_pct = cpct(decompress),
        nbt = nbt,
        partial_nbt = timings.partial_nbt,
        stream_nbt = timings.stream_nbt,
        full_nbt = timings.full_nbt,
        nbt_pct = cpct(nbt),
        payload_copy = timings.payload_copy,
        payload_copy_pct = cpct(timings.payload_copy),
        partial_hits = timings.partial_hits,
        partial_hits_pct = cnt_pct(timings.partial_hits),
        stream_hits = timings.stream_hits,
        stream_hits_pct = cnt_pct(timings.stream_hits),
        stream_misses = timings.stream_misses,
        stream_misses_pct = cnt_pct(timings.stream_misses),
        partial_skips = timings.partial_skips,
        partial_skips_pct = cnt_pct(timings.partial_skips),
        stream_out_avg = stream_out_avg,
        full_hits = timings.full_hits,
        full_fallbacks = timings.full_fallbacks,
        full_fallbacks_pct = cnt_pct(timings.full_fallbacks),
        scan_failures = timings.scan_failures,
        scan_failures_pct = cnt_pct(timings.scan_failures),
    )
}
