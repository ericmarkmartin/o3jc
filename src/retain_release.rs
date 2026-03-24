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
//! to the zeroing that happens on deallocation.  Each weak-pointer slot is
//! treated as a `ShardedMutex<Id>` — the slot's *address* selects one of 127
//! stripe locks from a global pool, and the guard provides `&mut Id` directly,
//! so the mutex genuinely protects the data it guards.
//!
//! Lock ordering (never hold both simultaneously; always acquire in this order):
//!   weak slot lock  →  DashMap shard lock
//!
//! # Deallocation sequence
//!
//! 1. Decrement retain count to zero under the DashMap shard lock.
//! 2. Set `deallocating = true`, extract `weak_locations`, release shard lock.
//! 3. For each location: acquire its slot lock → write `None` → release.
//! 4. Call `-dealloc` (may safely retain/release other objects).
//! 5. Remove the entry from the table.

use dashmap::DashMap;
use sharded_mutex::{LockCount, ShardedMutex};
use smallvec::SmallVec;
use std::ptr::NonNull;
use std::sync::LazyLock;

use crate::autorelease::objc_autorelease;
use crate::msg_send::objc_msg_lookup;
use crate::sel::sel_register_name_str;
use crate::types::Id;

// ---------------------------------------------------------------------------
// Weak slot locking via ShardedMutex
//
// `ShardedMutex<Id, WeakSlotTag>` is `#[repr(transparent)]` around `Id`, so a
// `*mut Id` (the user's weak variable) can be reinterpreted as a
// `*mut ShardedMutex<Id, WeakSlotTag>`.  The slot's address selects one of 127
// stripe locks from a global pool.
// ---------------------------------------------------------------------------

/// Tag type for the weak-slot locking domain, keeping its mutex pool separate
/// from any other `ShardedMutex` users.
struct WeakSlotTag;

sharded_mutex::sharded_mutex!(WeakSlotTag: Id);

// ---------------------------------------------------------------------------
// WeakSlot — a reference to a user's weak-pointer slot, viewed as a
// ShardedMutex so that locking and data access are unified.
// ---------------------------------------------------------------------------

/// A weak-pointer slot, viewed as a `&ShardedMutex<Id>`.
///
/// Created by reinterpreting the user's `*mut Id` through
/// `#[repr(transparent)]`.  Stored in the side table so that `do_dealloc` can
/// lock and zero each slot without a separate lookup.
///
/// The `'static` lifetime is an upper bound — the ABI guarantees
/// `objc_destroyWeak` is called (removing the entry) before the location is
/// invalidated.
struct WeakSlot(&'static ShardedMutex<Id, WeakSlotTag>);

impl WeakSlot {
    /// Cast a location pointer to a `WeakSlot`.
    ///
    /// # Safety
    /// `location` must be non-null, properly aligned, and point to a valid
    /// `Id` that remains live until `objc_destroyWeak` removes this entry.
    unsafe fn from_ptr(location: NonNull<Id>) -> Self {
        // SAFETY: caller guarantees the pointer is valid.
        // `ShardedMutex<Id>` is `#[repr(transparent)]` around `Id`.
        let p_sharded_lock = location.as_ptr().cast::<ShardedMutex<Id, WeakSlotTag>>();
        WeakSlot(unsafe { &*p_sharded_lock })
    }

    /// Return the raw location address (for equality comparisons).
    fn addr(&self) -> *const Id {
        std::ptr::from_ref(self.0).cast::<Id>()
    }
}

// SAFETY: The `&'static ShardedMutex<Id>` inside points to a user-owned weak
// slot whose access is serialized through the ShardedMutex stripe lock.
// `ShardedMutex<Id>` itself is `!Sync` (because `Id` contains `NonNull` which
// is `!Send`), but our `ObjcObject` has manual `Send + Sync` impls and all
// access goes through the lock, so sending/sharing the reference is safe.

// ---------------------------------------------------------------------------
// Side table
// ---------------------------------------------------------------------------

struct SideTableEntry {
    /// Actual retain count. Absent from the map ↔ implicit count of 1.
    retain_count: usize,
    /// Set before weak refs are zeroed and `-dealloc` is called.
    /// Prevents concurrent `objc_retain` from reviving a dying object.
    deallocating: bool,
    /// Weak-pointer slots to zero when this object deallocates.
    /// Inline size 0: most objects have no weak references.
    weak_locations: SmallVec<[WeakSlot; 0]>,
}

// Keyed by object address cast to `usize` — this makes the pointer inert
// (no `Send`/`Sync` issues) and avoids a custom wrapper type.  The address
// is only used for identity; the pointer is never dereferenced through the key.
static TABLE: LazyLock<DashMap<usize, SideTableEntry>> = LazyLock::new(DashMap::new);

// ---------------------------------------------------------------------------
// Retain / release
// ---------------------------------------------------------------------------

/// Increment the retain count of `obj` and return it, or return `None` if the
/// object has begun deallocation.
///
/// # Safety
/// `obj` must be `None` or point to a live `ObjcObject`.
pub unsafe fn objc_retain(obj: Id) -> Id {
    let obj = obj?;
    let mut entry = TABLE.entry(obj.as_ptr() as usize).or_insert(SideTableEntry {
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
    let key = obj.as_ptr() as usize;

    let weak_locations = match TABLE.entry(key) {
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
                // lock, then release it before touching any slot lock.
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
        TABLE.remove(&key);
    }
}

/// Return the current retain count of `obj` (primarily for debugging).
pub fn objc_retain_count(obj: Id) -> usize {
    let Some(obj) = obj else { return 0 };
    TABLE.get(&(obj.as_ptr() as usize)).map_or(1, |e| e.retain_count)
}

// ---------------------------------------------------------------------------
// Deallocation
// ---------------------------------------------------------------------------

/// Zero each weak location (under its slot lock), then call `-dealloc`.
///
/// # Safety
/// `obj` must be `Some`. The side table entry must have `deallocating = true`
/// and `weak_locations` must have been extracted from it.
unsafe fn do_dealloc(obj: Id, weak_locations: SmallVec<[WeakSlot; 0]>) {
    for ws in &weak_locations {
        let mut guard = ws.0.lock();
        // The slot lock is held, so this write is race-free with any
        // concurrent `objc_load_weak_retained` on this location.
        *guard = None;
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
// ---------------------------------------------------------------------------

/// Initialise the weak-pointer location `*location` to point to `obj`.
///
/// # Safety
/// `location` must be a valid, writable pointer. `obj` must be `None` or live.
pub unsafe fn objc_init_weak(location: NonNull<Id>, obj: Id) -> Id {
    // SAFETY: `location` is non-null, properly aligned, and valid for writes
    // of `Id` (caller's contract).
    unsafe { *location.as_ptr() = None };
    // SAFETY: `location` was just written to `None` above, so it is initialised;
    // `obj` is `Some` and points to a live `ObjcObject` (caller's contract).
    unsafe { objc_store_weak(location, Some(obj?)) }
}

/// Update the weak-pointer location `*location` to point to `new_obj`.
///
/// Stores `None` if `new_obj` has begun deallocation.
///
/// # Safety
/// `location` must have been initialised by `objc_init_weak`. `new_obj` must
/// be `None` or point to a live `ObjcObject`.
pub unsafe fn objc_store_weak(location: NonNull<Id>, new_obj: Id) -> Id {
    // SAFETY: `location` is non-null, aligned, and points to a valid `Id`
    // (caller's contract).
    let ws = unsafe { WeakSlot::from_ptr(location) };
    let mut guard = ws.0.lock();

    let old_obj = *guard;
    if let Some(old_obj) = old_obj
        && let Some(mut entry) = TABLE.get_mut(&(old_obj.as_ptr() as usize))
    {
        let loc = location.as_ptr() as *const Id;
        entry.weak_locations.retain(|w| w.addr() != loc);
    }

    if let Some(new_obj) = new_obj {
        let mut entry = TABLE.entry(new_obj.as_ptr() as usize).or_insert(SideTableEntry {
            retain_count: 1,
            deallocating: false,
            weak_locations: SmallVec::new(),
        });
        if entry.deallocating {
            *guard = None;
            return None;
        }
        entry.weak_locations.push(ws);
        *guard = Some(new_obj);
        return Some(new_obj);
    }

    *guard = None;
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
    // SAFETY: `location` is non-null, aligned, and points to a valid `Id`
    // (caller's contract).
    let ws = unsafe { WeakSlot::from_ptr(location) };
    let guard = ws.0.lock();

    // The slot lock is held, preventing concurrent zeroing by `do_dealloc`.
    let obj = *guard;
    obj?;
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
    // SAFETY: `location` is non-null, aligned, and points to a valid `Id`
    // (caller's contract).
    let ws = unsafe { WeakSlot::from_ptr(location) };
    let mut guard = ws.0.lock();

    let obj = *guard;
    if let Some(obj) = obj
        && let Some(mut entry) = TABLE.get_mut(&(obj.as_ptr() as usize))
    {
        let loc = location.as_ptr() as *const Id;
        entry.weak_locations.retain(|w| w.addr() != loc);
    }
    *guard = None;
}
