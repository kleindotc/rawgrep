use std::sync::OnceLock;
use std::num::NonZeroUsize;

/// One logical-processor id per physical core, ordered P-cores first, then
/// E-cores. Search workers should be pinned in this order so they fill every
/// P-core before spilling onto E-cores.
pub struct CoreTopology {
    pub ordered_lps:  Vec<usize>,
    pub p_core_count: usize,
}

impl CoreTopology {
    /// Returns exactly `worker_count` core ids to pin workers to, plus one
    /// leftover core for the worker thread if one exists.
    ///
    /// worker_count <= ordered_lps.len(): workers take a prefix, one
    /// physical core is left over for the output thread.
    ///
    /// worker_count > ordered_lps.len(): oversubscription was asked for
    /// explicitly. Wrap around. Because ordered_lps lists P-cores before
    /// E-cores, wrapping visits P0..P5 a second time before it ever
    /// touches an E-core twice, which is exactly HT pairing: P-cores have
    /// a real second thread to share, E-cores I assume most of the time don't...?
    pub fn claim(&self, worker_count: usize) -> (Vec<usize>, Option<usize>) {
        if worker_count <= self.ordered_lps.len() {
            let (workers, rest) = self.ordered_lps.split_at(worker_count);
            return (workers.to_vec(), rest.first().copied());
        }

        let workers = (0..worker_count)
            .map(|i| self.ordered_lps[i % self.ordered_lps.len()])
            .collect();

        (workers, None)
    }
}

pub fn default_worker_count() -> NonZeroUsize {
    NonZeroUsize::new(detect().ordered_lps.len())
        .unwrap_or(unsafe { NonZeroUsize::new_unchecked(1) })
}

pub fn detect() -> &'static CoreTopology {
    static TOPOLOGY: OnceLock<CoreTopology> = OnceLock::new();
    TOPOLOGY.get_or_init(detect_new)
}

pub fn detect_new() -> CoreTopology {
    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(info) = gdt_cpus::CpuInfo::detect() {
            if info.is_hybrid() {
                let primary = info.primary_thread_mask(); // one LP per physical core
                let perf    = info.performance_core_mask();
                let eff     = info.efficiency_core_mask();
                let all     = info.logical_processor_ids();

                let p_lps: Vec<usize> = all.iter().copied()
                    .filter(|id| primary.contains(*id) && perf.contains(*id))
                    .collect();
                let e_lps: Vec<usize> = all.iter().copied()
                    .filter(|id| primary.contains(*id) && eff.contains(*id))
                    .collect();

                if !p_lps.is_empty() {
                    let p_core_count = p_lps.len();
                    let mut ordered_lps = p_lps;
                    ordered_lps.extend(e_lps);
                    return CoreTopology { ordered_lps, p_core_count };
                }
            }
        }
    }

    // Non-hybrid, detection failed, or macOS: no P/E distinction available.
    let n = crate::util::num_physical_cores_or(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
    CoreTopology { ordered_lps: (0..n).collect(), p_core_count: n }
}

/// Best-effort: pin to `core_id`, but if the caller says this thread is
/// lower priority than the search workers (i.e. it's sharing a P-core it
/// couldn't get exclusive use of), also ask the OS to deprioritize it so it
/// doesn't compete evenly with a worker on the same physical core.
#[inline]
pub fn pin_thread_to_core_deprioritized(core_id: usize) {
    crate::util::pin_thread_to_core(core_id);

    #[cfg(not(target_os = "macos"))]
    {
        _ = gdt_cpus::set_thread_priority(gdt_cpus::ThreadPriority::BelowNormal);
    }

    #[cfg(target_os = "macos")]
    {
        // No affinity on Apple Silicon anyway...
    }
}
