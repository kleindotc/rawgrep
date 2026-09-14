#!/usr/bin/env bash

set -uo pipefail

# --- config ---

THREADS=16
RUNS=10
WARM_RUNS=50
WARMUP=5
RESULTS_DIR="./benchmark_results"
DO_CORRECTNESS_CHECK=true

DEVICE="/dev/nvme0n1p2"
NVME_CTRL="nvme0"

HYPERGREP_BIN="hgrep"

# search trees + patterns we benchmark against.
CHROMIUM_DIR="../chromium"
LINUX_DIR="../linux"
LINUX_VERSION="7.3.0-rc1"   # just for the system-info header, no functional effect

PATTERN_TODO="TODO"
PATTERN_TODO_REGEX='(?i)\bTODO\((?:crbug\.com/\d+|[a-zA-Z][\w.-]*)\)'
PATTERN_REGEX='[A-Z]+_SUSPEND'

if [[ "${1:-}" == "--no-correctness-check" ]]; then
    DO_CORRECTNESS_CHECK=false
fi

mkdir -p "$RESULTS_DIR"

# fff isn't in this list -- my life is very short and fff is very slow.
# Its benchmark is disabled below (see run_fff_benchmark),
# so we don't need the binary installed to run this script right now...
for cmd in rg rawgrep "$HYPERGREP_BIN" hyperfine jq; do
    if ! command -v "$cmd" &>/dev/null; then
        echo "error: $cmd not found"
        exit 1
    fi
done

# --- pin CPU governor and NVMe power state for the duration of the run ---
# schedutil ramps clocks lazily under sudden multi-thread load, and NVMe
# APST (auto) lets the drive drop into low power states between bursts,
# both of which inject noise into short benchmark runs. Force both to
# max-performance mode here, and restore original state on exit no matter
# how the script terminates.

ORIG_GOVERNORS=$(cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor 2>/dev/null | sort -u)
ORIG_APST=$(cat "/sys/class/nvme/${NVME_CTRL}/power/control" 2>/dev/null || echo "auto")

restore_power_settings() {
    echo ""
    echo "=== restoring original power settings ==="
    if [ -n "${ORIG_GOVERNORS:-}" ]; then
        # If governors were mixed originally just fall back to schedutil,
        # otherwise restore whatever the single common value was
        governor_count=$(echo "$ORIG_GOVERNORS" | wc -l)
        if [ "$governor_count" -eq 1 ]; then
            echo "$ORIG_GOVERNORS" | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor > /dev/null
            echo "cpu governor restored to: $ORIG_GOVERNORS"
        else
            echo "schedutil" | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor > /dev/null
            echo "cpu governor was mixed originally, defaulted restore to: schedutil"
        fi
    fi
    echo "$ORIG_APST" | sudo tee "/sys/class/nvme/${NVME_CTRL}/power/control" > /dev/null
    echo "nvme power control restored to: $ORIG_APST"
}
trap restore_power_settings EXIT

echo "=== pinning cpu governor to performance ==="
echo performance | sudo tee /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor > /dev/null
cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor | sort -u

echo ""
echo "=== disabling nvme autonomous power state transitions ==="
echo on | sudo tee "/sys/class/nvme/${NVME_CTRL}/power/control" > /dev/null
cat "/sys/class/nvme/${NVME_CTRL}/power/control"

# used as hyperfine's --prepare for every cold-cache run below. Exported so
# the subshell hyperfine spawns for --prepare can actually see it -- this
# used to be duplicated inline at every --prepare call site, no reason for
# that now that hyperfine gets it as a real function.
drop_caches() {
    sync
    echo 3 | sudo tee /proc/sys/vm/drop_caches > /dev/null
    sleep 1
}
export -f drop_caches

# --- system info ---

echo "=== system info ===" | tee "$RESULTS_DIR/system.txt"
uname -a | tee -a "$RESULTS_DIR/system.txt"
lscpu | grep -E "Model name|CPU\(s\)|MHz" | tee -a "$RESULTS_DIR/system.txt"
free -h | tee -a "$RESULTS_DIR/system.txt"
lsblk -d -o NAME,ROTA,SCHED,SIZE | tee -a "$RESULTS_DIR/system.txt"
if command -v nvme &>/dev/null; then
    sudo nvme id-ctrl "$DEVICE" 2>/dev/null | grep -E "mn|fr" | tee -a "$RESULTS_DIR/system.txt"
fi
echo "kernel:      $(uname -r)" | tee -a "$RESULTS_DIR/system.txt"
echo "rawgrep:     $(rawgrep --version 2>/dev/null || echo unknown)" | tee -a "$RESULTS_DIR/system.txt"
echo "ripgrep:     $(rg --version | head -1)" | tee -a "$RESULTS_DIR/system.txt"
echo "hypergrep:   $($HYPERGREP_BIN --version 2>/dev/null || echo unknown)" | tee -a "$RESULTS_DIR/system.txt"
echo "hyperfine:   $(hyperfine --version)" | tee -a "$RESULTS_DIR/system.txt"
echo "linux tree:  $LINUX_VERSION" | tee -a "$RESULTS_DIR/system.txt"

# --- correctness check ---
# compares a candidate tool's output against rg's for a given
# search-dir/pattern pair, treating rg as ground truth. Called once per
# candidate (rawgrep, hypergrep) so each gets its own report and its own
# set of temp files -- the temp files are namespaced by $label so the two
# calls made per suite below don't clobber each other.

check_correctness() {
    local label="$1"                    # e.g. "rawgrep" or "hypergrep"
    local cmd_candidate_nocache="$2"
    local cmd_rg="$3"
    local search_dir="$4"
    local out_file="$5"

    local candidate_txt="/tmp/bench_${label}.txt"
    local candidate_files_txt="/tmp/bench_files_${label}.txt"
    local rg_txt="/tmp/bench_rg_${label}.txt"
    local rg_files_txt="/tmp/bench_files_rg_${label}.txt"

    echo ""
    echo "=== correctness check ($label vs rg): $out_file ===" | tee "$out_file"

    # rawgrep's --jump output has an extra space after the line number
    # ("file:123: text") that rg's -n output doesn't, so normalize that
    # away for rawgrep specifically. hypergrep is assumed to already match
    # rg's "file:line:text" shape -- if that turns out wrong, add an
    # equivalent branch here rather than guessing at a fix downstream.
    local candidate_normalizer="cat"
    if [[ "$label" == "rawgrep" ]]; then
        candidate_normalizer="sed 's/:\([0-9]*\): /:\1:/'"
    fi

    # Strip ANSI codes, carriage returns, trailing/leading spaces, and empty lines
    eval "$cmd_candidate_nocache" 2>/dev/null \
        | tr -d '\r' \
        | sed -E 's/\x1B\[[0-9;]*[a-zA-R]//g' \
        | eval "$candidate_normalizer" \
        | sed 's/[[:space:]]*$//' \
        | grep -v '^$' \
        | LC_ALL=C sort > "$candidate_txt"

    eval "$cmd_rg" 2>/dev/null \
        | tr -d '\r' \
        | sed -E 's/\x1B\[[0-9;]*[a-zA-R]//g' \
        | sed 's/[[:space:]]*$//' \
        | grep -v '^$' \
        | LC_ALL=C sort > "$rg_txt"

    # rg/candidate's paths carry a "$search_dir/" prefix since we run from
    # outside it; strip that prefix before diffing or every real match
    # looks like a mismatch.
    local search_dir_prefix
    search_dir_prefix=$(printf '%s\n' "${search_dir%/}/" | sed 's/[.[\*^$/]/\\&/g')
    sed -i "s|^${search_dir_prefix}||" "$candidate_txt"
    sed -i "s|^${search_dir_prefix}||" "$rg_txt"

    cut -d: -f1 "$candidate_txt" | grep -v '^$' | LC_ALL=C sort -u > "$candidate_files_txt"
    cut -d: -f1 "$rg_txt" | grep -v '^$' | LC_ALL=C sort -u > "$rg_files_txt"

    # File 1 = rg, File 2 = candidate across all comparisons
    # comm -23 file1 file2 -> items in file1 (rg) but NOT file2 (candidate)
    # comm -13 file1 file2 -> items in file2 (candidate) but NOT file1 (rg)
    local missed_lines extra_lines missed_files extra_files
    missed_lines=$(LC_ALL=C comm -23 "$rg_txt" "$candidate_txt" | wc -l)
    extra_lines=$(LC_ALL=C comm -13 "$rg_txt" "$candidate_txt" | wc -l)
    missed_files=$(LC_ALL=C comm -23 "$rg_files_txt" "$candidate_files_txt" | wc -l)
    extra_files=$(LC_ALL=C comm -13 "$rg_files_txt" "$candidate_files_txt" | wc -l)

    {
        echo "$label vs rg:"
        echo "  lines in rg but not $label:   $missed_lines"
        echo "  lines in $label but not rg:   $extra_lines"
        echo "  files matched by rg only:        $missed_files"
        echo "  files matched by $label only:    $extra_files"
        echo ""
        echo "files matched by rg only (sample):"
        LC_ALL=C comm -23 "$rg_files_txt" "$candidate_files_txt" | head -10
        echo ""
        echo "files matched by $label only (sample):"
        LC_ALL=C comm -13 "$rg_files_txt" "$candidate_files_txt" | head -10
    } | tee -a "$out_file"
}

# --- main benchmark suite ---
# runs the 4 cache-state permutations (warm+cache, warm+no-cache,
# cold+no-cache, cold+cache) for one search-dir/pattern pair, across all
# three tools. This is the function that gets called once per (tree,
# pattern) combination below.

run_search_benchmarks() {
    local suite_label="$1"   # also doubles as the output subdir name, e.g. "chromium_todo"
    local search_dir="$2"
    local pattern="$3"

    local out_dir="$RESULTS_DIR/$suite_label"
    mkdir -p "$out_dir"

    local cmd_rawgrep="rawgrep '$pattern' '$search_dir' --jump --no-color --reserved-tool-dirs --large --threads $THREADS"
    local cmd_rawgrep_nocache="rawgrep '$pattern' '$search_dir' --jump --no-color --threads $THREADS --reserved-tool-dirs --large --no-cache --no-cache-write"
    local cmd_rg="rg '$pattern' '$search_dir' --no-heading --color=never -n --threads $THREADS"
    # hypergrep has no on-disk fragment cache to toggle like rawgrep does,
    # so (like rg) one command covers all four cache-state phases below.
    # NOTE: flags MUST precede PATTERN/PATH -- hgrep's own SYNOPSIS is
    # `hgrep [OPTIONS] PATTERN [PATH ...]`, and PATH accepts multiple
    # values. Putting -n/--ignore-gitindex after the search dir risks them
    # being swallowed as extra (nonexistent) PATH arguments instead of
    # being parsed as options, which silently turns this into a no-op
    # search that errors out near-instantly (a hyperfine mean in the low
    # single-digit milliseconds against a multi-GB tree is that failure
    # mode, not a real result).
    local cmd_hypergrep="$HYPERGREP_BIN -n --ignore-gitindex '$pattern' '$search_dir'"

    echo ""
    echo "########################################"
    echo "# $suite_label  (pattern: $pattern)"
    echo "########################################"

    if $DO_CORRECTNESS_CHECK; then
        check_correctness "rawgrep" "$cmd_rawgrep_nocache" "$cmd_rg" "$search_dir" "$out_dir/correctness.txt"
        check_correctness "hypergrep" "$cmd_hypergrep" "$cmd_rg" "$search_dir" "$out_dir/correctness_hypergrep.txt"
    fi

    # warm cache, with rawgrep's fragment cache
    echo ""
    echo "=== [$suite_label] warm cache + fragment cache ==="
    eval "$cmd_rawgrep" > /dev/null 2>&1 || true
    eval "$cmd_rg" > /dev/null 2>&1 || true
    eval "$cmd_hypergrep" > /dev/null 2>&1 || true
    hyperfine \
        --warmup "$WARMUP" \
        --runs "$WARM_RUNS" \
        --export-json "$out_dir/warm_with_cache.json" \
        --export-markdown "$out_dir/warm_with_cache.md" \
        --command-name "rawgrep" "$cmd_rawgrep" \
        --command-name "ripgrep" "$cmd_rg" \
        --command-name "hypergrep" "$cmd_hypergrep"

    # warm cache, no fragment cache
    echo ""
    echo "=== [$suite_label] warm cache, no fragment cache ==="
    eval "$cmd_rawgrep_nocache" > /dev/null 2>&1 || true
    eval "$cmd_rg" > /dev/null 2>&1 || true
    eval "$cmd_hypergrep" > /dev/null 2>&1 || true
    hyperfine \
        --warmup "$WARMUP" \
        --runs "$WARM_RUNS" \
        --export-json "$out_dir/warm_no_cache.json" \
        --export-markdown "$out_dir/warm_no_cache.md" \
        --command-name "rawgrep (no cache)" "$cmd_rawgrep_nocache" \
        --command-name "ripgrep" "$cmd_rg" \
        --command-name "hypergrep" "$cmd_hypergrep"

    # cold cache, no fragment cache
    echo ""
    echo "=== [$suite_label] cold cache, no fragment cache ==="
    hyperfine \
        --runs "$RUNS" \
        --export-json "$out_dir/cold_no_cache.json" \
        --export-markdown "$out_dir/cold_no_cache.md" \
        --prepare "sync && echo 3 | sudo tee /proc/sys/vm/drop_caches > /dev/null && sleep 1" \
        --command-name "rawgrep (no cache)" "$cmd_rawgrep_nocache" \
        --command-name "ripgrep" "$cmd_rg" \
        --command-name "hypergrep" "$cmd_hypergrep"

    # cold cache, with fragment cache
    echo ""
    echo "=== [$suite_label] cold cache + fragment cache ==="
    eval "$cmd_rawgrep" > /dev/null 2>&1 || true
    hyperfine \
        --runs "$RUNS" \
        --export-json "$out_dir/cold_with_cache.json" \
        --export-markdown "$out_dir/cold_with_cache.md" \
        --prepare "sync && echo 3 | sudo tee /proc/sys/vm/drop_caches > /dev/null && sleep 1" \
        --command-name "rawgrep" "$cmd_rawgrep" \
        --command-name "ripgrep" "$cmd_rg" \
        --command-name "hypergrep" "$cmd_hypergrep"

    SUITES+=("$suite_label")
}

# --- fff benchmarks ---
# disabled for now (nothing below calls these) but left as real functions
# instead of a wall of commented-out shell, so it's a one-line uncomment to
# bring back rather than a rewrite. fff doesn't take a positional search-dir
# arg -- it resolves the project root from cwd via git discovery -- so it
# can't just slot into run_search_benchmarks above like rg/rawgrep can.

FFF_MAX_RESULTS=999999999999

# Build fff's cache exactly once, up front. I just don't have enough time
# for it to be any slower -- do NOT call this inside the benchmark loop.
build_fff_cache() {
    local search_dir="$1"
    rm -rf "${search_dir}/.fff"
    (cd "$search_dir" && fff index --force) > /dev/null 2>&1 || true
}

run_fff_benchmark() {
    local suite_label="$1"
    local search_dir="$2"
    local pattern="$3"
    local cmd_rawgrep="$4"

    local out_dir="$RESULTS_DIR/$suite_label"
    mkdir -p "$out_dir"

    # fff has no positional search-dir arg on `grep`, it resolves the
    # project root from cwd via git discovery, so this runs in a subshell
    # that cd's into search_dir first.
    local cmd_fff_grep="(cd '$search_dir' && fff grep '$pattern' --max-results $FFF_MAX_RESULTS)"

    eval "$cmd_fff_grep" > /dev/null 2>&1 || true
    eval "$cmd_rawgrep" > /dev/null 2>&1 || true

    echo ""
    echo "=== [$suite_label] warm, fff (cache built once) vs rawgrep (fragment cache) ==="
    hyperfine \
        --warmup "$WARMUP" \
        --runs "$WARM_RUNS" \
        --export-json "$out_dir/warm_fff.json" \
        --export-markdown "$out_dir/warm_fff.md" \
        --command-name "fff (cache built once)" "$cmd_fff_grep" \
        --command-name "rawgrep" "$cmd_rawgrep"

    echo ""
    echo "=== [$suite_label] cold, fff (cache built once) vs rawgrep (fragment cache) ==="
    hyperfine \
        --runs "$RUNS" \
        --export-json "$out_dir/cold_fff.json" \
        --export-markdown "$out_dir/cold_fff.md" \
        --prepare "sync && echo 3 | sudo tee /proc/sys/vm/drop_caches > /dev/null && sleep 1" \
        --command-name "fff (cache built once)" "$cmd_fff_grep" \
        --command-name "rawgrep" "$cmd_rawgrep"
}

# --- run everything ---

SUITES=()

# chromium: literal TODO pattern, same as the original script
run_search_benchmarks "chromium_todo" "$CHROMIUM_DIR" "$PATTERN_TODO"

# chromium: the TODO(...) convention regex, to see how the two engines
# compare on something with a literal anchor plus real branching, not just
# a flat literal scan
run_search_benchmarks "chromium_regex" "$CHROMIUM_DIR" "$PATTERN_TODO_REGEX"

# same two patterns, now against the linux tree ($LINUX_VERSION)
run_search_benchmarks "linux_todo" "$LINUX_DIR" "$PATTERN_TODO"
run_search_benchmarks "linux_regex" "$LINUX_DIR" "$PATTERN_REGEX"

# fff comparison -- not run right now, uncomment to bring it back. Needs
# build_fff_cache called once up front for whichever tree you point it at.
# build_fff_cache "$CHROMIUM_DIR"
# run_fff_benchmark "chromium_fff" "$CHROMIUM_DIR" "$PATTERN_TODO" \
#     "rawgrep '$PATTERN_TODO' '$CHROMIUM_DIR' --jump --no-color --reserved-tool-dirs --large --threads $THREADS"

# --- ram usage ---
# hyperfine's --export-json already captured peak RSS per run in
# memory_usage_byte, just pull it back out and report mean/max per command
# across every JSON file we wrote above. Written straight to a file and
# only ever cat'd once, at the bottom, instead of being printed here too.

compute_ram_usage() {
    local out_file="$1"
    {
        echo "=== ram usage (peak RSS, from hyperfine's own measurements) ==="
        for f in "$RESULTS_DIR"/*/*.json; do
            jq -r '
                .results[]
                | select(.memory_usage_byte != null)
                | [.command,
                   (([.memory_usage_byte[]] | add / length) / 1048576 | floor),
                   (([.memory_usage_byte[]] | max) / 1048576 | floor)]
                | @tsv
            ' "$f" 2>/dev/null
        done | awk -F'\t' '{printf "%-40s mean %6s MiB   max %6s MiB\n", $1, $2, $3}'
    } > "$out_file"
}

compute_ram_usage "$RESULTS_DIR/ram.txt"

# --- summary ---

echo ""
echo "========================================"
echo "results"
echo "========================================"

for suite in "${SUITES[@]}"; do
    out_dir="$RESULTS_DIR/$suite"
    echo ""
    echo "--- $suite ---"
    for phase in warm_with_cache warm_no_cache cold_no_cache cold_with_cache; do
        if [ -f "$out_dir/$phase.md" ]; then
            echo ""
            echo "$phase:"
            cat "$out_dir/$phase.md"
        fi
    done
done

echo ""
echo "ram usage:"
cat "$RESULTS_DIR/ram.txt"

echo ""
echo "full results in $RESULTS_DIR/"
