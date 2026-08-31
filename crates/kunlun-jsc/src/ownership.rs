//! Platform-independent ownership primitives for opaque, thread-affine handles.
//!
//! Keeping these mechanics free of JavaScriptCore calls lets Miri exercise the
//! wrapper's release, protection, clone, and failure paths on every platform.

use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

/// Catches even panic payloads whose destructor panics. Only the exceptional
/// payload is leaked; callback state is unwound/dropped normally on this thread.
pub(crate) fn catch_callback_panic<T>(operation: impl FnOnce() -> T) -> Result<T, ()> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(value) => Ok(value),
        Err(payload) => {
            std::mem::forget(payload);
            Err(())
        }
    }
}

/// Owns one opaque native handle and releases it exactly once.
pub(crate) struct OwnedHandle<T> {
    raw: NonNull<T>,
    release: unsafe fn(NonNull<T>),
    _thread_affinity: PhantomData<Rc<()>>,
}

impl<T> OwnedHandle<T> {
    /// Takes exclusive ownership of `raw`.
    ///
    /// # Safety
    ///
    /// `raw` must be a live owned handle accepted by `release`. No other owner
    /// may release it, and it must remain thread-affine for this value's life.
    pub(crate) unsafe fn from_raw(raw: *mut T, release: unsafe fn(NonNull<T>)) -> Option<Self> {
        Some(Self {
            raw: NonNull::new(raw)?,
            release,
            _thread_affinity: PhantomData,
        })
    }

    #[cfg(kunlun_jsc_native)]
    pub(crate) fn as_ptr(&self) -> *mut T {
        self.raw.as_ptr()
    }
}

impl<T> Drop for OwnedHandle<T> {
    fn drop(&mut self) {
        // SAFETY: construction transfers unique release responsibility to
        // this guard, and Drop runs exactly once.
        unsafe { (self.release)(self.raw) };
    }
}

/// Owns one protection count for a context-bound borrowed handle.
pub(crate) struct ProtectedHandle<T, C> {
    context: Rc<C>,
    raw: NonNull<T>,
    unprotect: unsafe fn(&C, NonNull<T>),
}

impl<T, C> ProtectedHandle<T, C> {
    /// Adds one protection count and assumes responsibility for removing it.
    pub(crate) fn try_new<E>(
        context: Rc<C>,
        raw: *const T,
        protect: unsafe fn(&C, NonNull<T>) -> Result<(), E>,
        unprotect: unsafe fn(&C, NonNull<T>),
        null_error: E,
    ) -> Result<Self, E> {
        let raw = NonNull::new(raw.cast_mut()).ok_or(null_error)?;
        // SAFETY: the caller supplied a live handle belonging to `context`.
        // No guard is constructed when protection fails.
        unsafe { protect(&context, raw)? };
        Ok(Self {
            context,
            raw,
            unprotect,
        })
    }

    /// Adds an independent protection count for a new owned guard.
    pub(crate) fn try_clone<E>(
        &self,
        protect: unsafe fn(&C, NonNull<T>) -> Result<(), E>,
    ) -> Result<Self, E> {
        // SAFETY: this guard retains the context and proves the raw handle is
        // still live. A failed protection creates no second guard.
        unsafe { protect(&self.context, self.raw)? };
        Ok(Self {
            context: Rc::clone(&self.context),
            raw: self.raw,
            unprotect: self.unprotect,
        })
    }

    #[cfg(kunlun_jsc_native)]
    pub(crate) fn as_ptr(&self) -> *const T {
        self.raw.as_ptr().cast_const()
    }

    #[cfg(kunlun_jsc_native)]
    pub(crate) fn context(&self) -> &Rc<C> {
        &self.context
    }
}

impl<T, C> Drop for ProtectedHandle<T, C> {
    fn drop(&mut self) {
        // SAFETY: every successful construction owns exactly one protection
        // count, and the retained Rc keeps the context alive through Drop.
        unsafe { (self.unprotect)(&self.context, self.raw) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[test]
    fn callback_panic_drops_local_state_but_not_a_hostile_payload() {
        struct Payload;
        impl Drop for Payload {
            fn drop(&mut self) {
                panic!("panic payload destructor must not run");
            }
        }
        let log = Rc::new(RefCell::new(Vec::new()));
        let result = catch_callback_panic(|| {
            let _local = owner(Rc::clone(&log));
            std::panic::panic_any(Payload);
        });
        assert!(result.is_err());
        assert_eq!(&*log.borrow(), &["owner"]);
        assert_eq!(catch_callback_panic(|| 42), Ok(42));
    }

    struct TrackedOwner {
        log: Rc<RefCell<Vec<&'static str>>>,
        protect_attempts: Cell<usize>,
        unprotects: Cell<usize>,
        reject_next_protect: Cell<bool>,
    }

    impl Drop for TrackedOwner {
        fn drop(&mut self) {
            self.log.borrow_mut().push("owner");
        }
    }

    struct TrackedResource {
        label: &'static str,
        log: Rc<RefCell<Vec<&'static str>>>,
    }

    unsafe fn release_resource(raw: NonNull<TrackedResource>) {
        // SAFETY: the test transfers one Box allocation to OwnedHandle.
        let resource = unsafe { Box::from_raw(raw.as_ptr()) };
        resource.log.borrow_mut().push(resource.label);
    }

    unsafe fn protect(owner: &TrackedOwner, _raw: NonNull<u8>) -> Result<(), &'static str> {
        owner.protect_attempts.set(owner.protect_attempts.get() + 1);
        if owner.reject_next_protect.replace(false) {
            Err("protect failed")
        } else {
            Ok(())
        }
    }

    unsafe fn unprotect(owner: &TrackedOwner, _raw: NonNull<u8>) {
        owner.unprotects.set(owner.unprotects.get() + 1);
        owner.log.borrow_mut().push("unprotect");
    }

    fn owner(log: Rc<RefCell<Vec<&'static str>>>) -> Rc<TrackedOwner> {
        Rc::new(TrackedOwner {
            log,
            protect_attempts: Cell::new(0),
            unprotects: Cell::new(0),
            reject_next_protect: Cell::new(false),
        })
    }

    #[test]
    fn owned_handle_releases_exactly_once() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let raw = Box::into_raw(Box::new(TrackedResource {
            label: "resource",
            log: Rc::clone(&log),
        }));
        // SAFETY: `raw` is a unique live Box allocation for the test releaser.
        let handle = unsafe { OwnedHandle::from_raw(raw, release_resource) }.unwrap();

        assert!(log.borrow().is_empty());
        drop(handle);
        assert_eq!(&*log.borrow(), &["resource"]);
    }

    #[test]
    fn temporary_string_releases_before_its_retained_context() {
        struct StringScope {
            _string: OwnedHandle<TrackedResource>,
            _context: Rc<TrackedOwner>,
        }

        let log = Rc::new(RefCell::new(Vec::new()));
        let context = owner(Rc::clone(&log));
        let raw = Box::into_raw(Box::new(TrackedResource {
            label: "string",
            log: Rc::clone(&log),
        }));
        let scope = StringScope {
            // SAFETY: `raw` is uniquely owned by this string guard.
            _string: unsafe { OwnedHandle::from_raw(raw, release_resource) }.unwrap(),
            _context: Rc::clone(&context),
        };

        drop(context);
        drop(scope);
        assert_eq!(&*log.borrow(), &["string", "owner"]);
    }

    #[test]
    fn protected_clone_balances_each_protection() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let owner = owner(Rc::clone(&log));
        let raw = NonNull::<u8>::dangling().as_ptr();
        let first =
            ProtectedHandle::try_new(Rc::clone(&owner), raw, protect, unprotect, "null").unwrap();
        let second = first.try_clone(protect).unwrap();

        assert_eq!(owner.protect_attempts.get(), 2);
        drop(first);
        assert_eq!(owner.unprotects.get(), 1);
        drop(second);
        assert_eq!(owner.unprotects.get(), 2);
    }

    #[test]
    fn failed_protection_does_not_create_an_unprotect_obligation() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let owner = owner(log);
        owner.reject_next_protect.set(true);
        let raw = NonNull::<u8>::dangling().as_ptr();

        let result = ProtectedHandle::try_new(Rc::clone(&owner), raw, protect, unprotect, "null");
        assert_eq!(result.err(), Some("protect failed"));
        assert_eq!(owner.protect_attempts.get(), 1);
        assert_eq!(owner.unprotects.get(), 0);
    }

    #[test]
    fn unprotect_runs_before_the_retained_owner_is_released() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let owner = owner(Rc::clone(&log));
        let raw = NonNull::<u8>::dangling().as_ptr();
        let protected =
            ProtectedHandle::try_new(Rc::clone(&owner), raw, protect, unprotect, "null").unwrap();

        drop(owner);
        assert!(log.borrow().is_empty());
        drop(protected);
        assert_eq!(&*log.borrow(), &["unprotect", "owner"]);
    }

    #[test]
    fn child_handle_releases_before_its_group_owner() {
        struct Child {
            _handle: OwnedHandle<TrackedResource>,
            _group: Rc<TrackedOwner>,
        }

        let log = Rc::new(RefCell::new(Vec::new()));
        let group = owner(Rc::clone(&log));
        let raw = Box::into_raw(Box::new(TrackedResource {
            label: "context",
            log: Rc::clone(&log),
        }));
        let child = Child {
            // SAFETY: `raw` is uniquely owned by this guard.
            _handle: unsafe { OwnedHandle::from_raw(raw, release_resource) }.unwrap(),
            _group: Rc::clone(&group),
        };

        drop(group);
        drop(child);
        assert_eq!(&*log.borrow(), &["context", "owner"]);
    }
}
