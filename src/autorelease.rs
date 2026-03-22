//! Thread-local autorelease pools.
//!
//! Each thread maintains a stack of pools. `objc_autoreleasePoolPush` pushes a
//! new pool and returns an opaque token (the pre-push stack depth cast to a
//! pointer). `objc_autoreleasePoolPop(token)` drains every pool added since
//! `token` was issued, releasing the objects in LIFO order, then restores the
//! stack to its pre-push depth.
//!
//! If no pool is active when `objc_autorelease` is called the object is
//! silently leaked — this matches Apple and GNUstep behaviour.

use std::cell::RefCell;

use crate::retain_release::objc_release;
use crate::types::Id;

thread_local! {
    /// Stack of pools. Each inner `Vec` holds the objects autoreleased into
    /// that pool in the order they were added.
    static POOL_STACK: RefCell<Vec<Vec<Id>>> = const { RefCell::new(Vec::new()) };
}

/// Push a new autorelease pool onto the current thread's pool stack.
///
/// Returns an opaque token that must be passed to `objc_autoreleasePoolPop`
/// to drain this pool (and any pools pushed since).
pub fn objc_autorelease_pool_push() -> *mut () {
    POOL_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let token = stack.len() as *mut ();
        stack.push(Vec::new());
        token
    })
}

/// Pop autorelease pools back to `token`, releasing all objects added since.
///
/// Objects within each pool are released in reverse insertion order.
///
/// # Safety
/// `token` must have been returned by a prior call to `objc_autoreleasePoolPush`
/// on the same thread, and not yet consumed by a matching pop.
pub unsafe fn objc_autorelease_pool_pop(token: *mut ()) {
    let target_depth = token as usize;
    POOL_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        while stack.len() > target_depth {
            if let Some(pool) = stack.pop() {
                for obj in pool.into_iter().rev() {
                    // SAFETY: objects were live when added to the pool and
                    // `objc_autorelease` guarantees they are valid Id values.
                    unsafe { objc_release(obj) };
                }
            }
        }
    });
}

/// Add `obj` to the current autorelease pool and return it.
///
/// If no pool is active, `obj` is returned unchanged (and leaked).
///
/// # Safety
/// `obj` must be null or point to a live `ObjcObject`.
pub unsafe fn objc_autorelease(obj: Id) -> Id {
    if obj.is_none() {
        return obj;
    }
    POOL_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        if let Some(pool) = stack.last_mut() {
            pool.push(obj);
        }
    });
    obj
}
