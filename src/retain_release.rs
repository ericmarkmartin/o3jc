//! Reference counting, deallocation, and weak references.
//!
//! # Side table
//!
//! A `DashMap<usize, SideTableEntry>` keyed by object address holds the retain
//! count, a `deallocating` flag, and weak-reference locations per object.
//! An object absent from the table has an implicit retain count of 1 and is
//! not deallocating.
//!
//! # Weak reference safety
//!
//! Reading a weak pointer and retaining the object must be atomic with respect
//! to the zeroing that happens on deallocation. A second striped lock set
//! (`WEAK_LOCKS`, indexed by *location* address) protects the pointer value
//! stored at each weak-reference slot.
//!
//! Lock ordering (never hold both simultaneously; always acquire in this order):
//!   weak location lock  →  DashMap shard lock
//!
//! # Deallocation sequence
//!
//! 1. Decrement retain count to zero under the DashMap shard lock.
//! 2. Set `deallocating = true`, extract `weak_locations`, release shard lock.
//! 3. For each location: acquire its weak lock → write `None` → release.
//! 4. Call `-dealloc` (may safely retain/release other objects).
//! 5. Remove the entry from the table.

use dashmap::DashMap;
use parking_lot::Mutex;
use smallvec::SmallVec;
use std::ptr::NonNull;
use std::sync::LazyLock;

use crate::autorelease::objc_autorelease;
use crate::msg_send::objc_msg_lookup;
use crate::sel::sel_register_name_str;
use crate::types::Id;

// ---------------------------------------------------------------------------
// Weak location locks
//
// Striped by location address (not object address), so they can be found
// without first reading the (potentially racy) pointer value.

static WEAK_LOCKS: LazyLock<[Mutex<()>; 8]> =
    LazyLock::new(|| std::array::from_fn(|_| Mutex::new(())));

// ---------------------------------------------------------------------------
// WeakLocation

/// A non-null pointer to a weak-reference slot (`Id` variable).
///
/// Encapsulates the stripe-lock logic so the `unsafe impl Send/Sync` is
/// narrowed to this type rather than the whole `SideTableEntry`.
///
/// # Safety invariant
/// All reads and writes through the inner pointer must be performed while
/// holding the guard returned by `acquire()`.
struct WeakLocation(NonNull<Id>);

// SAFETY: `WeakLocation` is Send + Sync because the inner pointer is only
// accessed while holding the stripe lock locked by acquire, so it can be
// thought of morally like `Mutex<NonNull<Id>>` (but without the poisoning
// semantics).
unsafe impl Send for WeakLocation {}
unsafe impl Sync for WeakLocation {}

/// RAII guard that holds a stripe lock for a weak-pointer slot.
///
/// `Deref`s to `NonNull<Id>` — the pointer to the slot itself. Callers use
/// `NonNull::read` and `NonNull::write` (both `unsafe`) to access the slot
/// value; holding the guard is the precondition that makes those operations
/// race-free.
struct ProxyGuard<T> {
    _guard: parking_lot::MutexGuard<'static, ()>,
    value: T,
}

impl<T> std::ops::Deref for ProxyGuard<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<T> std::ops::DerefMut for ProxyGuard<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

impl WeakLocation {
    fn new(ptr: NonNull<Id>) -> Self {
        WeakLocation(ptr)
    }

    /// Acquire the stripe lock for this location's address and return a guard.
    /// The slot must not be read or written except through the guard.
    fn lock(&self) -> ProxyGuard<NonNull<Id>> {
        ProxyGuard {
            // Stripe index derived from the location address (not the object
            // address), so the correct lock can be found without first reading
            // the potentially-racy pointer stored at the location.
            _guard: WEAK_LOCKS[(self.0.as_ptr() as usize >> 3) % 8].lock(),
            value: self.0,
        }
    }
}

impl PartialEq for WeakLocation {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl PartialEq<NonNull<Id>> for WeakLocation {
    fn eq(&self, other: &NonNull<Id>) -> bool {
        self.0 == *other
    }
}

// ---------------------------------------------------------------------------
// Side table

struct SideTableEntry {
    /// Actual retain count. Absent from the map ↔ implicit count of 1.
    retain_count: usize,
    /// Set before weak refs are zeroed and `-dealloc` is called.
    /// Prevents concurrent `objc_retain` from reviving a dying object.
    deallocating: bool,
    /// Weak-pointer locations to zero when this object deallocates.
    /// Inline size 0: most objects have no weak references.
    weak_locations: SmallVec<[WeakLocation; 0]>,
}

// SAFETY: WeakLocation is Send + Sync (its own unsafe impl); the other fields
// are plain data. Access to weak slot memory is serialised by WEAK_LOCKS.
unsafe impl Send for SideTableEntry {}
unsafe impl Sync for SideTableEntry {}

static TABLE: LazyLock<DashMap<usize, SideTableEntry>> = LazyLock::new(DashMap::new);

// ---------------------------------------------------------------------------
// Retain / release

/// Increment the retain count of `obj` and return it, or return `None` if the
/// object has begun deallocation.
///
/// # Safety
/// `obj` must be `None` or point to a live `ObjcObject`.
pub unsafe fn objc_retain(obj: Id) -> Id {
    let obj = obj?;
    let mut entry = TABLE
        .entry(obj.as_ptr() as usize)
        .or_insert(SideTableEntry {
            retain_count: 1,
            deallocating: false,
            weak_locations: SmallVec::new(),
        });
    if entry.deallocating {
        return None;
    }
    entry.retain_count += 1;
    Some(obj)
}

/// Decrement the retain count of `obj`; deallocate when it reaches zero.
///
/// # Safety
/// `obj` must be `None` or point to a live `ObjcObject`.
pub unsafe fn objc_release(obj: Id) {
    let Some(obj) = obj else { return };
    let addr = obj.as_ptr() as usize;

    let weak_locations = match TABLE.entry(addr) {
        dashmap::mapref::entry::Entry::Vacant(_) => {
            // Absent → implicit count 1; releasing drops to 0.
            // No weak locations possible (they require a table entry).
            Some(SmallVec::new())
        }
        dashmap::mapref::entry::Entry::Occupied(mut e) => {
            if e.get().deallocating {
                // Already deallocating; ignore.
                return;
            }
            if e.get().retain_count <= 1 {
                // Mark deallocating and extract weak locations under the shard
                // lock, then release it before touching any weak location lock.
                let entry = e.get_mut();
                entry.deallocating = true;
                Some(std::mem::take(&mut entry.weak_locations))
            } else {
                e.get_mut().retain_count -= 1;
                None
            }
        }
    };

    if let Some(weak_locations) = weak_locations {
        // SAFETY: `obj` is non-null (destructured from `Some`) and points to a
        // live `ObjcObject` (caller's invariant). `deallocating` is set and
        // `weak_locations` has been extracted from the entry.
        unsafe { do_dealloc(Some(obj), weak_locations) };
        TABLE.remove(&addr);
    }
}

/// Return the current retain count of `obj` (primarily for debugging).
pub fn objc_retain_count(obj: Id) -> usize {
    let Some(obj) = obj else { return 0 };
    TABLE
        .get(&(obj.as_ptr() as usize))
        .map_or(1, |e| e.retain_count)
}

// ---------------------------------------------------------------------------
// Deallocation

/// Zero each weak location (under its lock), then call `-dealloc`.
///
/// # Safety
/// `obj` must be `Some`. The side table entry must have `deallocating = true`
/// and `weak_locations` must have been extracted from it.
unsafe fn do_dealloc(obj: Id, weak_locations: SmallVec<[WeakLocation; 0]>) {
    for loc in &weak_locations {
        let guard = loc.lock();
        // The stripe lock is held, so this write is race-free with any
        // concurrent `objc_load_weak_retained` on this location.
        // SAFETY: `loc` was registered via `objc_init_weak`/`objc_store_weak`
        // (caller's contract), so the pointer is non-null, properly aligned,
        // and valid for writes of `Id`.
        unsafe { guard.write(None) };
    }

    let dealloc_sel = sel_register_name_str("dealloc");
    // SAFETY: `obj` is `Some` per the function's safety contract, so it is a
    // non-null, aligned pointer to a live `ObjcObject`. `dealloc_sel` is a
    // non-null interned selector pointer.
    if let Some(imp) = unsafe { objc_msg_lookup(obj, dealloc_sel) } {
        // SAFETY: `obj` is a non-null, aligned pointer to a live `ObjcObject`
        // (caller's contract); `imp` is the resolved IMP for `dealloc_sel` on
        // this object's class.
        unsafe { imp(obj, dealloc_sel) };
    }
}

// ---------------------------------------------------------------------------
// Weak references

/// Initialise the weak-pointer location `*location` to point to `obj`.
///
/// # Safety
/// `location` must be a valid, writable pointer. `obj` must be `None` or live.
pub unsafe fn objc_init_weak(location: NonNull<Id>, obj: Id) -> Id {
    // SAFETY: `location` is non-null, properly aligned, and valid for writes
    // of `Id` (caller's contract).
    unsafe { *location.as_ptr() = None };
    if obj.is_none() {
        return None;
    }
    // SAFETY: `location` was just written to `None` above, so it is initialised;
    // `obj` is `Some` and points to a live `ObjcObject` (caller's contract).
    unsafe { objc_store_weak(location, obj) }
}

/// Update the weak-pointer location `*location` to point to `new_obj`.
///
/// Stores `None` if `new_obj` has begun deallocation.
///
/// # Safety
/// `location` must have been initialised by `objc_init_weak`. `new_obj` must
/// be `None` or point to a live `ObjcObject`.
pub unsafe fn objc_store_weak(location: NonNull<Id>, new_obj: Id) -> Id {
    let wl = WeakLocation::new(location);
    let guard = wl.lock();

    // The stripe lock is held, so this read is race-free with any concurrent
    // `do_dealloc` zeroing this location.
    // SAFETY: `location` was initialised by `objc_init_weak` (caller's
    // contract), so it is non-null, aligned, and contains a valid `Id`.
    let old_obj = unsafe { guard.read() };
    if let Some(old_obj) = old_obj {
        if let Some(mut entry) = TABLE.get_mut(&(old_obj.as_ptr() as usize)) {
            entry.weak_locations.retain(|loc| loc != &location);
        }
    }

    if let Some(new_obj) = new_obj {
        let mut entry = TABLE
            .entry(new_obj.as_ptr() as usize)
            .or_insert(SideTableEntry {
                retain_count: 1,
                deallocating: false,
                weak_locations: SmallVec::new(),
            });
        if entry.deallocating {
            // The stripe lock is held, preventing concurrent reads of this
            // location by `objc_load_weak_retained`.
            // SAFETY: `location` was initialised by `objc_init_weak`
            // (caller's contract), so it is non-null, aligned, and valid
            // for writes of `Id`.
            unsafe { guard.write(None) };
            return None;
        }
        entry.weak_locations.push(wl);
        // The stripe lock is held, preventing concurrent reads of this
        // location by `objc_load_weak_retained`.
        // SAFETY: `location` was initialised by `objc_init_weak`
        // (caller's contract), so it is non-null, aligned, and valid for
        // writes of `Id`. `new_obj` is a valid `NonNull<ObjcObject>`
        // (destructured from `Some`).
        unsafe { guard.write(Some(new_obj)) };
        return Some(new_obj);
    }

    // The stripe lock is held, preventing concurrent reads of this location.
    // SAFETY: `location` was initialised by `objc_init_weak` (caller's
    // contract), so it is non-null, aligned, and valid for writes of `Id`.
    unsafe { guard.write(None) };
    None
}

/// Promote the weak pointer at `location` to a +1 strong reference.
///
/// Returns a retained object, or `None` if the object has been deallocated.
/// The caller is responsible for releasing the returned reference.
///
/// # Safety
/// `location` must have been initialised by `objc_init_weak`.
pub unsafe fn objc_load_weak_retained(location: NonNull<Id>) -> Id {
    let guard = WeakLocation::new(location).lock();
    // The stripe lock is held, preventing concurrent zeroing by `do_dealloc`.
    // SAFETY: `location` was initialised by `objc_init_weak` (caller's
    // contract), so it is non-null, aligned, and contains a valid `Id`.
    let obj = unsafe { guard.read() };
    if obj.is_none() {
        return None;
    }
    // SAFETY: `obj` is `Some` (checked above), so it is a non-null, aligned
    // pointer to an `ObjcObject`. The object may be in the `deallocating`
    // state; `objc_retain` handles that case by returning `None`.
    unsafe { objc_retain(obj) }
}

/// Load a weak pointer, autoreleasing the result.
///
/// Returns the object (autoreleased) or `None` if it has been deallocated.
///
/// # Safety
/// `location` must have been initialised by `objc_init_weak`.
pub unsafe fn objc_load_weak(location: NonNull<Id>) -> Id {
    // SAFETY: `location` was initialised by `objc_init_weak` (caller's contract).
    let obj = unsafe { objc_load_weak_retained(location) };
    if obj.is_some() {
        // SAFETY: `obj` is `Some` (checked above), so it is a non-null,
        // aligned pointer to a live `ObjcObject` with a +1 retain.
        unsafe { objc_autorelease(obj) };
    }
    obj
}

/// Unregister and clear the weak-pointer location `*location`.
///
/// # Safety
/// `location` must have been initialised by `objc_init_weak`.
pub unsafe fn objc_destroy_weak(location: NonNull<Id>) {
    let wl = WeakLocation::new(location);
    let guard = wl.lock();
    // The stripe lock is held, so this read is race-free with any concurrent
    // `do_dealloc` zeroing this location.
    // SAFETY: `location` was initialised by `objc_init_weak` (caller's
    // contract), so it is non-null, aligned, and contains a valid `Id`.
    let obj = unsafe { guard.read() };
    if let Some(obj) = obj
        && let Some(mut entry) = TABLE.get_mut(&(obj.as_ptr() as usize))
    {
        entry.weak_locations.retain(|loc| loc != &location);
    }
    // The stripe lock is held, preventing concurrent reads of this location
    // by `objc_load_weak_retained`.
    // SAFETY: `location` was initialised by `objc_init_weak` (caller's
    // contract), so it is non-null, aligned, and valid for writes of `Id`.
    unsafe { guard.write(None) };
}
