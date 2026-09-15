
# rawgrep

**Grep at the speed of raw disk** - search text by reading data directly from raw block devices.

## Benchmarks

benchmark script: [`bench.sh`](https://github.com/rakivo/rawgrep/blob/master/bench.sh)

The following benchmarks compare `rawgrep` `0.2.0` (rev `d05ccd7`, with `Hyperscan`)  against:

- [ripgrep](https://github.com/BurntSushi/ripgrep) `15.2.0` (rev `3fce3b5bb0`)
- [hypergrep](https://github.com/p-ranav/hypergrep) `0.1.1` (rev `ee85b71`)

All benchmarks were run with [hyperfine](https://github.com/sharkdp/hyperfine) `1.20.0`, with the CPU governor pinned to `performance` and NVMe autonomous power-state transitions disabled for the duration of the run.

### System Details

| Type            | Value                                                      |
| --------------- | ---------------------------------------------------------- |
| Processor       | 13th Gen Intel(R) Core(TM) i5-13400F (16 threads)          |
| IS Extensions   | Intel® SSE4.1, Intel® SSE4.2, Intel® AVX2                  |
| CPU Max Clock   | 2501 MHz (4.6 GHz on P-cores in turbo-boost)               |
| Installed RAM   | 15 GiB DDR4 3200MHZ                                        |
| Disk            | NVMe SSD (Crucial P2 250GB, ~994MB/s read / 736MB/s write) |
| OS / Kernel     | Debian, Linux `6.12.57+deb13-amd64`                        |

### Cache Scenarios

Each search is run under four conditions to separate raw match-finding speed from the effect of `rawgrep`'s on-disk fragment cache:

| Scenario                       | Page cache | Fragment cache |
| ------------------------------- | :--------: | :-------------: |
| `warm, with fragment cache`     | warm       | enabled          |
| `warm, no fragment cache`       | warm       | disabled         |
| `cold, no fragment cache`       | cold       | disabled         |
| `cold, with fragment cache`     | cold       | enabled          |


### Codebase Search: `Chromium` (~500K files)

The following searches are performed against a full clone of the [Chromium source tree](https://github.com/chromium/chromium).

**Pattern: `TODO`**

| Scenario                     | rawgrep      | ripgrep           | hypergrep         |
| ---------------------------- | ------------ | ------------------ | ------------------- |
| warm, with fragment cache    | **130.8 ms** | 364.7 ms (2.79x)   | 353.2 ms (2.71x)    |
| warm, no fragment cache      | **289.1 ms** | 362.9 ms (1.26x)  | 355.8 ms (1.23x)  |
| cold, no fragment cache      | **8.450 s**  | 11.911 s (1.41x)  | 10.187 s (1.21x)  |
| cold, with fragment cache    | **2.676 s**  | 11.875 s (4.44x)  | 10.140 s (3.79x)  |


**Pattern: `(?i)\bTODO\((?:crbug\.com/\d+|[a-zA-Z][\w.-]*)\)`**

| Scenario                  | rawgrep      | ripgrep          | hypergrep         |
| -------------------------- | ------------- | ----------------- | ------------------ |
| warm, with fragment cache  | **111.9 ms** | 384.7 ms (3.44x)  | 364.6 ms (3.26x)  |
| warm, no fragment cache    | **293.6 ms** | 386.4 ms (1.32x)  | 363.6 ms (1.24x)  |
| cold, no fragment cache    | **8.488 s**  | 11.871 s (1.40x)  | 10.195 s (1.20x)  |
| cold, with fragment cache  | **2.449 s**  | 11.844 s (4.84x)  | 10.206 s (4.17x)  |


### Codebase Search: `Linux 7.3.0-rc1` (~95K files)

The following searches are performed against a full clone of the [Linux kernel source tree](https://github.com/torvalds/linux).

**Pattern: `TODO`**

| Scenario                  | rawgrep          | ripgrep          | hypergrep         |
| -------------------------- | ----------------- | ----------------- | ------------------ |
| warm, with fragment cache  | **54.6 ms**       | 109.0 ms (2.00x)  | 129.7 ms (2.37x)  |
| warm, no fragment cache    | 174.5 ms (1.60x)  | **108.8 ms**      | 131.9 ms (1.21x)  |
| cold, no fragment cache    | **3.301 s**       | 3.529 s (1.07x)   | 3.375 s (1.02x)   |
| cold, with fragment cache  | **494.8 ms**      | 3.538 s (7.15x)   | 3.346 s (6.76x)   |

**Pattern: `[A-Z]+_SUSPEND`**

| Scenario                  | rawgrep          | ripgrep          | hypergrep         |
| -------------------------- | ----------------- | ----------------- | ------------------ |
| warm, with fragment cache  | **62.2 ms**       | 113.8 ms (1.83x)  | 139.0 ms (2.23x)  |
| warm, no fragment cache    | 179.6 ms (1.58x)  | **113.9 ms**      | 138.3 ms (1.21x)  |
| cold, no fragment cache    | **3.328 s**       | 3.501 s (1.05x)   | 3.341 s (1.00x)   |
| cold, with fragment cache  | **517.4 ms**      | 3.578 s (6.92x)   | 3.370 s (6.51x)   |


### Peak Memory Usage

Peak RSS as measured by hyperfine, taken from the `chromium_todo` runs (representative of the other cases):

| Scenario                  | rawgrep | ripgrep | hypergrep |
| -------------------------- | ------- | ------- | --------- |
| warm, with fragment cache  | 210 MiB | 210 MiB | 210 MiB   |
| warm, no fragment cache    | 295 MiB | 306 MiB | 306 MiB   |

**These RSS numbers aren't reliable yet** -- all three land within a few MiB of each other, which is, I'm pretty sure, just the leftover page cache from whatever ran before it. In actuality, `ripgrep` plateaus around ~70 MiB on the aforementioned benchmark, I'd imagine `hypergrep` using roughly the same amount of memory... `rawgrep`'s RAM usage hasn't been the main focus yet; the work so far has gone almost entirely into wall-time, though, there are for sure some known ideas to bring RSS down without affecting the performance much, or even at all.

### Thoughts

- `rawgrep`'s advantage is largest when its fragment cache is warm, particularly on cold page cache (4-7x over ripgrep/hypergrep). Without the fragment cache, the margin narrows substantially, and on the smaller Linux tree specifically, plain ripgrep is faster than `rawgrep (no cache)` in the warm/no-fragment-cache case (1.4-1.5x). This number, of course, isn't final, and `rawgrep` is gonna get much faster in the future.

- The *Fragment Cache* `rawgrep` uses doesn't require a separate, precomputed index the way approaches like n-gram/bigram indexing do -- rawgrep builds and updates it automatically as needed, therefore there isn't even a subcommand like `index`. It's also very lean: searching the full Chromium tree only keeps a 18.66 MB cache, and the Linux tree just 3.63 MB -- versus the hundreds of megabytes that n-gram/bigram-indexing tools like fff or tgrep require for the same corpora.

- Correctness was cross-checked against `rg` on every corpus/pattern pair: the set of *files* matched was identical in all cases.

- As always, a single benchmark run on one machine is never the whole story -- treat these as directional, not a guarantee of performance on your own workload.

## How is `rawgrep` so fast?

- `rawgrep` reads files DIRECTLY from your partition, completely bypassing the virtual filesystem layer.
- `rawgrep` uses work-stealing parallel traversal to keep all CPU cores busy during directory scanning.
- `rawgrep` uses the aforementioned fragment-based caching system (inspired by [nowgrep](https://github.com/asbott/nowgrep)).

## Installation

### Prerequisites

- Linux (contribute to make rawgrep support Windows) system with ext4/ntfs filesystem
- Rust toolchain (for building from source)
- Root access or be able to set capabilities

### Option 1: One-Time Setup with Capabilities (Recommended)

```bash
git clone https://github.com/rakivo/rawgrep
cd rawgrep

cargo build --profile=release-fast

# If you want maximum speed possible (requires nightly):
# cargo +nightly build --profile=release-fast --target=<your_target> --features=use_nightly

# Run the one-time setup command. Why? Read "Why Elevated Permissions?" section
sudo setcap cap_dac_read_search=eip ./target/release-fast/rawgrep
```

Now you can run it without `sudo`:
```bash
rawgrep "search pattern"
```

### Option 2: Use `sudo` Every Time

If you prefer not to use capabilities, just build and run with `sudo`:

```bash
cargo build --profile=release-fast

# Again, if you want maximum speed possible (requires nightly):
# cargo +nightly build --profile=release-fast --target=<your_target> --features=use_nightly

# Run with sudo each time
sudo ./target/release-fast/rawgrep "search pattern"
```

### Optional: `cap_ipc_lock` for Faster Cache Loads

rawgrep keeps its on-disk *fragment cache* `mlock`ed in memory once mmap'ed, so it
can't be evicted under memory pressure from a large scan. Locking requires
either root or the `cap_ipc_lock` capability; without it, rawgrep still
works fine, it just falls back to a best-effort population strategy
that's usually just as fast, but can occasionally be slower on a cold cache
under heavy concurrent memory pressure.

If you're already setting `cap_dac_read_search` per Option 1 above, add
`cap_ipc_lock` to the same command instead of running it twice:

```bash
sudo setcap 'cap_dac_read_search,cap_ipc_lock=eip' ./target/release-fast/rawgrep
```

## Usage

### Basic Search
```bash
# Search current directory
rawgrep "error"

# Search specific directory
rawgrep "TODO" /var/log

# Regex patterns
rawgrep "error|warning|critical" .
```

### Advanced Options
```bash
# Specify device manually (auto-detected by default)
rawgrep "pattern" /home --device=/dev/sda1

# Print statistics at the end of the search
rawgrep "pattern" . --stats

# Disable filtering (search everything)
rawgrep "pattern" . -uuu
# or
rawgrep "pattern" . --all

# Disable specific filters
rawgrep "pattern" . --no-ignore # Don't use .gitignore
rawgrep "pattern" . --binary    # Search binary files
```

### Filtering Levels
```bash
# Default: respects .gitignore, skips binaries and large files (> 30 MB)
rawgrep "pattern"

# -u: ignore .gitignore
rawgrep "pattern" -u

# -uu: also search binary files
rawgrep "pattern" -uu

# -uuu: search everything, including large files
rawgrep "pattern" -uuu
```

## Why Elevated Permissions?

`rawgrep` reads raw block devices (e.g., `/dev/sda1`), which are protected by the OS. Instead of requiring full root access via `sudo` every time, we use Linux capabilities to grant **only** the specific permission needed.

### What is `CAP_DAC_READ_SEARCH`?

This capability grants exactly **one** permission: bypass file read permission checks.

**`rawgrep` only reads data, it never writes anything to disk.**

### Verifying Capabilities

You can verify what capabilities the binary has:

```bash
getcap ./target/release-fast/rawgrep
# Output: ./target/release-fast/rawgrep = cap_dac_read_search+eip
```

### Removing Capabilities

If you want to revoke the capability and go back to using `sudo`:

```bash
sudo setcap -r ./target/release-fast/rawgrep
```

## Limitations (IMPORTANT)

- **ext4/ntfs only:** Currently only supports ext4/ntfs filesystems.

## Development

**Note:** Capabilities are tied to the binary file itself, so you'll need to re-run `setcap` after each rebuild.

> **Why no automation script?** I intentionally decide not to provide a script that runs `sudo` commands. If you want automation, write your own script, it's just a few lines of bash code and you'll understand exactly what it does.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## Roadmap

- [ ] Support for Windows. (Some physical partition reading/parsing stuff needs to get redesigned for Windows, besides that the NTFS parser is already working, although it's nearly not as optimized as the ext4 parser)
- [ ] Support for OSX+APFS.
- [ ] Symlink support

## Emacs Integration

To use rawgrep from Emacs with jumpable locations:

1. Download `emacs/rawgrep.el` and place it in your load path
2. Add to your `.emacs` or `init.el`:
```elisp
(require 'rawgrep)
(global-set-key (kbd "M-e") 'rawgrep)
```

Or if you use `use-package`:
```elisp
(use-package rawgrep
  :load-path "path/to/rawgrep.el"
  :bind ("M-e" . rawgrep))
```

Works exactly like `'grep-find` but better.

## FAQ

**Q: Is this safe to use?**
A: Yes. The tool only reads data and never writes. The `CAP_DAC_READ_SEARCH` capability is narrowly scoped.

**Q: Why am I missing some matches?**
A: By default, rawgrep respects `.gitignore` and skips binary/large/usually-reserved (> 30MB) files. Use `-u` to ignore `.gitignore`, `-uu` to also search binaries, or `-uuu` to search everything. This matches ripgrep's behavior.

**Q: Can I use this on other filesystems?**
A: Currently only ext4/ntfs is supported. Support for other filesystems may be added in the future. (Motivate me with stars)

**Q: Will this damage my filesystem?**
A: No. `rawgrep` only ever performs read operations when bypassing the VFS. It cannot modify your filesystem (except for the *Fragment Cache*, of course, but rawgrep doesn't bypass the VFS in order to read it).

**Q: What if partition auto-detection fails?**
A: Specify the device manually with `--device=/dev/sdXY`. Use `df -Th` to find your partition.

## Acknowledgments

Inspired by [ripgrep](https://github.com/BurntSushi/ripgrep) and [nowgrep](https://github.com/asbott/nowgrep), and the need for high-quality software in the big 25.
