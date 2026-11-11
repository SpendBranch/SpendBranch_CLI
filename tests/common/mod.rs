//! A global allocator that counts memory, shared by the memory tests. Each test binary that
//! uses it declares `#[global_allocator]` itself and must hold a single test, because the
//! counter is process-wide.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

pub struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator; only a counter is added.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            PEAK.fetch_max(CURRENT.fetch_add(l.size(), SeqCst) + l.size(), SeqCst);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        CURRENT.fetch_sub(l.size(), SeqCst);
        unsafe { System.dealloc(p, l) }
    }
}

/// Extra memory used while `f` runs, beyond what was allocated before it.
pub fn extra_peak<T>(f: impl FnOnce() -> T) -> (usize, T) {
    let base = CURRENT.load(SeqCst);
    PEAK.store(base, SeqCst);
    let out = f();
    (PEAK.load(SeqCst) - base, out)
}
