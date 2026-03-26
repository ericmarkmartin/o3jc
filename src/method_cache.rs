//! Per-class method cache: a small dense list of `(SEL, IMP)` pairs.
//!
//! # Design
//! Each class owns a heap-allocated `MethodCache` stored as `Option<NonNull<MethodCache>>`
//! in `ObjcClass::cache`.
//!
//! Thread safety is provided by a `parking_lot::RwLock` around the inner list.
//! The fast path acquires a read lock; insertions and flushes acquire a write lock.
//!
//! The cache is a `Vec` pre-allocated to `CACHE_CAPACITY`. Lookup is a linear
//! scan — for the small hot working sets typical of a class, this is as fast as
//! or faster than a hash table due to dense sequential memory access. When the
//! list is full it is cleared (same as Apple's runtime) rather than grown:
//! the cache is invalidated wholesale on every swizzle or post-registration
//! method add anyway, so growth would only waste memory.

use parking_lot::RwLock;

use crate::sel::sel_eq;
use crate::types::{IMP, ObjcClass, SEL};

// ---------------------------------------------------------------------------
// Inner list

/// Maximum entries before the cache is flushed rather than grown.
const CACHE_CAPACITY: usize = 16;

struct CacheEntry {
    sel: SEL,
    imp: IMP,
}

struct CacheInner {
    entries: Vec<CacheEntry>,
}

impl CacheInner {
    fn new() -> Self {
        CacheInner {
            entries: Vec::with_capacity(CACHE_CAPACITY),
        }
    }

    fn lookup(&self, sel: SEL) -> Option<IMP> {
        self.entries
            .iter()
            .find(|e| sel_eq(e.sel, sel))
            .map(|e| e.imp)
    }

    fn insert(&mut self, sel: SEL, imp: IMP) {
        // Another thread may have inserted this sel while we waited for the
        // write lock; in that case the cached IMP is already correct.
        if self.entries.iter().any(|e| sel_eq(e.sel, sel)) {
            return;
        }
        if self.entries.len() == self.entries.capacity() {
            self.entries.clear();
        }
        self.entries.push(CacheEntry { sel, imp });
    }

    fn flush(&mut self) {
        self.entries.clear();
    }
}

// ---------------------------------------------------------------------------
// Public API

/// Per-class method cache.
pub struct MethodCache {
    inner: RwLock<CacheInner>,
}

impl MethodCache {
    /// Allocate a new, empty cache on the heap.
    pub fn new() -> Box<Self> {
        Box::new(MethodCache {
            inner: RwLock::new(CacheInner::new()),
        })
    }

    /// Look up `sel` in the cache. Returns `None` on a miss.
    pub fn lookup(&self, sel: SEL) -> Option<IMP> {
        self.inner.read().lookup(sel)
    }

    /// Insert `(sel, imp)` into the cache.
    pub fn insert(&self, sel: SEL, imp: IMP) {
        self.inner.write().insert(sel, imp);
    }

    /// Clear all entries. Called on method list mutation or swizzle.
    pub fn flush(&self) {
        self.inner.write().flush();
    }
}

// ---------------------------------------------------------------------------
// Cache-tree helpers

/// Flush the method cache of `cls` and recursively all of its subclasses.
///
/// # Safety
/// `cls` must be null or point to a live `ObjcClass`. All pointers reachable
/// via `first_subclass` / `next_sibling` must also be valid.
pub unsafe fn flush_class_cache_tree(cls: *mut ObjcClass) {
    if cls.is_null() {
        return;
    }
    // SAFETY: caller guarantees `cls` is non-null and valid.
    let cls_ref = unsafe { &*cls };
    if let Some(cache) = cls_ref.cache() {
        // SAFETY: cache was allocated by `MethodCache::new` in `objc_allocate_class_pair`.
        unsafe { cache.as_ref().flush() };
    }
    let mut sub = cls_ref.subclass_list;
    while let Some(s) = sub {
        // SAFETY: subclass pointers are set in `objc_allocate_class_pair` and are valid
        // for the process lifetime (classes are never freed).
        unsafe { flush_class_cache_tree(s.as_ptr()) };
        sub = unsafe { s.as_ref().sibling_class };
    }
}
