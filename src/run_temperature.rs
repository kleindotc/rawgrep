use std::cell::Cell;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};

const UNKNOWN: u8 = 0;
const COLD:    u8 = 1;
const WARM:    u8 = 2;

//
// Decisions are made per window of samples, so the state can change its mind mid-run.
//
// Going warm needs an almost perfect window, staying warm only a mostly good one (hysteresis).
// Being wrongly COLD costs a few cheap fadvise calls, being wrongly WARM costs I/O overlap, so
// the state is deliberately quicker to go cold than to go warm.
//
const WINDOW:     u32 = 32;
const ENTER_WARM: u32 = 31;  // Hits out of WINDOW needed to become WARM
const LEAVE_WARM: u32 = 26;  // Hits out of WINDOW needed to stay WARM

//
// The first samples are taken at every opportunity so the state settles quickly. After that,
// one opportunity in SAMPLE_EVERY (per thread) is sampled, so a warm run keeps checking that
// it is still warm for the price of a handful of syscalls.
//
const EAGER_SAMPLES: u32 = 64;
const SAMPLE_EVERY:  u32 = 4;

// How much of a file's head is looked at when sampling the data temperature.
pub const HEAD_SAMPLE_BYTES: u64 = 64 * 1024;

// A sampled range is a hit when at least 3/4 of its pages are in the page cache.
#[inline(always)]
pub const fn is_hit(cached: u32, total: u32) -> bool {
    (cached as u64) * 4 >= (total as u64) * 3
}

pub static INODES: Temperature = Temperature::new(0);
pub static DATA:   Temperature = Temperature::new(1);

#[repr(align(64))]
pub struct Temperature {
    state: AtomicU8,
    hits:  AtomicU32,
    seen:  AtomicU32,
    id:    usize,     // Index into the per-thread tick counters
}

thread_local! {
    static TICKS: [Cell<u32>; 2] = const { [const { Cell::new(0) }; 2] };
}

impl Temperature {
    #[inline(always)]
    const fn new(id: usize) -> Self {
        Self {
            state: AtomicU8::new(UNKNOWN),
            hits:  AtomicU32::new(0),
            seen:  AtomicU32::new(0),
            id,
        }
    }

    /// Still deciding: callers treat this like COLD, but may want to retry a failed sample.
    #[inline(always)] pub fn is_unknown(&self) -> bool { self.state.load(Relaxed) == UNKNOWN }
    #[inline(always)] pub fn is_warm(&self)    -> bool { self.state.load(Relaxed) == WARM }

    /// Should the caller spend a sample on this opportunity?
    #[inline(always)]
    pub fn want_sample(&self) -> bool {
        if self.seen.load(Relaxed) < EAGER_SAMPLES { return true; }

        TICKS.with(|t| {
            let c = &t[self.id];
            let n = c.get().wrapping_add(1);
            c.set(n);
            n % SAMPLE_EVERY == 0
        })
    }

    /// Feed one sample (true = the sampled range was cached).
    #[inline]
    pub fn observe(&self, hit: bool) {
        self.hits.fetch_add(hit as u32, Relaxed);

        //
        // Exactly one thread sees the last slot of a window (u32 wraps on a multiple of WINDOW)
        //
        if self.seen.fetch_add(1, Relaxed) % WINDOW == WINDOW - 1 {
            let hits = self.hits.swap(0, Relaxed);
            let need = if self.is_warm() { LEAVE_WARM } else { ENTER_WARM };

            self.state.store(if hits >= need { WARM } else { COLD }, Relaxed);
        }
    }
}

//
// Inode-block hints we issued recently.
//
// Pages we asked the kernel to read count as cached to cachestat/mincore even while the read is
// still in flight, so sampling one of them would measure ourselves. Inode blocks are shared
// between neighbouring directories. File data doesn't need this, extents of different files are disjoint,
// and a file's own head is sampled before it is hinted.
//
// A small ring of the last RING ranges, packed as (first page << 24 | page count) in 4KB units.
// A range that has been overwritten is simply no longer filtered, which is the harmless direction.
//

const RING:       usize = 64;
const RANGE_PAGE: u32   = 12;
const LEN_BITS:   u32   = 24;
const LEN_MASK:   u64   = (1 << LEN_BITS) - 1;

static HINTS: [AtomicU64; RING] = [const { AtomicU64::new(0) }; RING];
static NEXT:   AtomicUsize      = AtomicUsize::new(0);

#[inline(always)]
pub fn note_inode_hint(offset: u64, len: u64) {
    if len == 0 { return; }

    let first = offset >> RANGE_PAGE;
    let pages = (((offset + len - 1) >> RANGE_PAGE) - first + 1).min(LEN_MASK);
    let slot  = NEXT.fetch_add(1, Relaxed) % RING;

    HINTS[slot].store((first << LEN_BITS) | pages, Relaxed);  // pages >= 1, so never 0
}

#[inline(always)]
pub fn recently_hinted(offset: u64, len: u64) -> bool {
    if len == 0 { return false; }

    let first = offset >> RANGE_PAGE;
    let last  = (offset + len - 1) >> RANGE_PAGE;

    HINTS.iter().any(|h| {
        let e = h.load(Relaxed);
        if e == 0 { return false; }

        let (f, pages) = (e >> LEN_BITS, e & LEN_MASK);
        f <= last && first < f + pages
    })
}

/// cachestat(2), Linux 6.5+: how many pages of a byte range are in the page cache
pub mod cachestat {
    use std::sync::atomic::{AtomicBool, Ordering::Relaxed};

    #[repr(C)]
    struct Range { off: u64, len: u64 }

    #[repr(C)]
    #[derive(Default)]
    struct Stat {
        nr_cache:            u64,
        nr_dirty:            u64,
        nr_writeback:        u64,
        nr_evicted:          u64,
        nr_recently_evicted: u64,
    }

    // Set when the syscall is missing or refuses this fd (old kernel, seccomp, fd type), so it
    // is tried once, not once per sample.
    static UNSUPPORTED: AtomicBool = AtomicBool::new(false);

    #[inline(always)]
    fn page_shift() -> u32 {
        static SHIFT: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

        *SHIFT.get_or_init(|| {
            let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            if ps > 0 { (ps as u64).trailing_zeros() } else { 12 }
        })
    }

    /// (cached pages, total pages) for [off, off + len), or None if cachestat isn't usable.
    #[allow(clippy::single_match)]
    pub fn residency(fd: libc::c_int, off: u64, len: u64) -> Option<(u32, u32)> {
        if len == 0 || UNSUPPORTED.load(Relaxed) { return None; }

        let range    = Range { off, len };
        let mut stat = Stat::default();

        let rc = unsafe {
            const SYS_CACHESTAT: libc::c_long = 451;  // Same number on every arch

            libc::syscall(
                SYS_CACHESTAT,
                fd as libc::c_long,
                &range as *const Range,
                &mut stat as *mut Stat,
                0 as libc::c_long,  // flags
            )
        };

        if rc != 0 {
            match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::ENOSYS | libc::EOPNOTSUPP | libc::EBADF | libc::EPERM) => {
                    UNSUPPORTED.store(true, Relaxed);
                }

                _ => {}
            }

            return None;
        }

        let shift = page_shift();
        let total = ((off + len - 1) >> shift) - (off >> shift) + 1;

        Some((stat.nr_cache.min(total) as u32, total as u32))
    }
}
