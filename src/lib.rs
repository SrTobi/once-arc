use std::fmt;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, Ordering};

/// A thread-safe container that can be atomically initialized once with an `Arc<T>`.
///
/// This is conceptually similar to `Atomic<Option<Arc<T>>>`, but with a critical restriction:
/// the value can only be set once. This restriction is what makes the implementation both
/// sound and extremely fast — `load` is a single atomic read with no reference count
/// manipulation.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::Ordering;
/// use atomic_once_arc::AtomicOnceArcOption;
///
/// let slot: AtomicOnceArcOption<i32> = AtomicOnceArcOption::new();
/// assert!(slot.get(Ordering::Acquire).is_none());
///
/// slot.set(Arc::new(42), Ordering::Release).unwrap();
///
/// // get() returns &T — just a single atomic load
/// assert_eq!(slot.get(Ordering::Acquire), Some(&42));
///
/// // load() returns a cloned Arc
/// let arc = slot.load(Ordering::Acquire).unwrap();
/// assert_eq!(*arc, 42);
///
/// // Second set fails and returns the value back.
/// let err = slot.set(Arc::new(99), Ordering::Release).unwrap_err();
/// assert_eq!(*err, 99);
/// ```
pub struct AtomicOnceArcOption<T> {
  ptr: AtomicPtr<T>,
}

// SAFETY: The inner `T` is behind an `Arc` and only accessible via shared reference.
// `Send` and `Sync` require `T: Send + Sync` to match `Arc<T>`'s bounds.
unsafe impl<T: Send + Sync> Send for AtomicOnceArcOption<T> {}
unsafe impl<T: Send + Sync> Sync for AtomicOnceArcOption<T> {}

impl<T> AtomicOnceArcOption<T> {
  /// Creates a new empty `AtomicOnceArcOption`.
  pub const fn new() -> Self {
    Self {
      ptr: AtomicPtr::new(ptr::null_mut()),
    }
  }

  /// Attempts to set the value. Returns `Ok(())` if this is the first call,
  /// or `Err(value)` if a value was already stored.
  ///
  /// `ordering` describes the required ordering for the
  /// read-modify-write operation that takes place if the None-check succeeds.
  /// Using [`Acquire`] as ordering makes the store part
  /// of this operation [`Relaxed`], and using [`Release`] makes the load [`Relaxed`].
  pub fn set(&self, value: Arc<T>, ordering: Ordering) -> Result<(), Arc<T>> {
    let raw = Arc::into_raw(value) as *mut T;
    match self
      .ptr
      .compare_exchange(ptr::null_mut(), raw, ordering, Ordering::Relaxed)
    {
      Ok(_) => Ok(()),
      Err(_) => {
        // SAFETY: We just created this raw pointer from Arc::into_raw above,
        // and the CAS failed so nobody else owns it.
        let value = unsafe { Arc::from_raw(raw) };
        Err(value)
      }
    }
  }

  /// Returns a reference to the stored value, or `None` if not yet set.
  ///
  /// This is extremely fast: a single atomic load with no reference count
  /// manipulation. The returned reference is valid for as long as `&self` is
  /// valid, because the stored `Arc` is never removed until `self` is dropped.
  pub fn get(&self, ordering: Ordering) -> Option<&T> {
    let ptr = self.ptr.load(ordering);
    if ptr.is_null() {
      None
    } else {
      // SAFETY: Once set, the pointer is never changed or freed until drop.
      // The &T lifetime is tied to &self, and drop requires &mut self,
      // so the data is guaranteed alive for the duration of the borrow.
      Some(unsafe { &*ptr })
    }
  }

  /// Loads the stored value as a cloned `Arc<T>`. This increments the reference
  /// count, so the caller gets an independent handle to the underlying data.
  ///
  /// Returns `None` if the value has not been set yet.
  pub fn load(&self, ordering: Ordering) -> Option<Arc<T>> {
    let ptr = self.ptr.load(ordering);
    if ptr.is_null() {
      None
    } else {
      // SAFETY: The pointer is valid and the Arc is alive (same reasoning as get).
      // increment_strong_count keeps the existing Arc alive while we create a new one.
      unsafe { Arc::increment_strong_count(ptr) };
      Some(unsafe { Arc::from_raw(ptr) })
    }
  }

  /// Returns `true` if a value has been stored.
  pub fn is_set(&self, ordering: Ordering) -> bool {
    !self.ptr.load(ordering).is_null()
  }

  /// Consumes `self` and returns the stored `Arc<T>`, if any.
  pub fn into_inner(mut self) -> Option<Arc<T>> {
    let ptr = *self.ptr.get_mut();
    std::mem::forget(self); // skip Drop since we're taking ownership of the Arc
    if ptr.is_null() {
      None
    } else {
      // SAFETY: We have exclusive ownership via `self` (by value).
      Some(unsafe { Arc::from_raw(ptr) })
    }
  }

  /// Returns a mutable reference to the stored value, or `None` if not yet set.
  ///
  /// Since this requires `&mut self`, no atomic operations are needed and this
  /// is guaranteed to be the only accessor.
  pub fn get_mut(&mut self) -> Option<&mut T> {
    let ptr = *self.ptr.get_mut();
    if ptr.is_null() {
      None
    } else {
      // SAFETY: We have exclusive access via &mut self. The pointer was created
      // by Arc::into_raw and the data is valid until drop. &mut self guarantees
      // no other references exist.
      Some(unsafe { &mut *ptr })
    }
  }
}

impl<T> Default for AtomicOnceArcOption<T> {
  fn default() -> Self {
    Self::new()
  }
}

impl<T: fmt::Debug> fmt::Debug for AtomicOnceArcOption<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("AtomicOnceArcOption")
      .field("value", &self.get(Ordering::SeqCst))
      .finish()
  }
}

impl<T> Drop for AtomicOnceArcOption<T> {
  fn drop(&mut self) {
    let ptr = *self.ptr.get_mut();
    if !ptr.is_null() {
      // SAFETY: We have &mut self, so no other references exist.
      // The pointer was created by Arc::into_raw in set().
      unsafe { drop(Arc::from_raw(ptr)) };
    }
  }
}

impl<T> From<Arc<T>> for AtomicOnceArcOption<T> {
  fn from(value: Arc<T>) -> Self {
    Self {
      ptr: AtomicPtr::new(Arc::into_raw(value) as *mut T),
    }
  }
}

impl<T> From<Option<Arc<T>>> for AtomicOnceArcOption<T> {
  fn from(value: Option<Arc<T>>) -> Self {
    match value {
      Some(arc) => Self::from(arc),
      None => Self::new(),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Arc;
  use std::sync::atomic::Ordering;

  #[test]
  fn empty_loads_none() {
    let slot: AtomicOnceArcOption<i32> = AtomicOnceArcOption::new();
    assert!(slot.get(Ordering::Acquire).is_none());
    assert!(slot.load(Ordering::Acquire).is_none());
    assert!(!slot.is_set(Ordering::Relaxed));
  }

  #[test]
  fn set_once_and_load() {
    let slot: AtomicOnceArcOption<i32> = AtomicOnceArcOption::new();
    slot.set(Arc::new(42), Ordering::Release).unwrap();
    assert_eq!(*slot.get(Ordering::Acquire).unwrap(), 42);
    assert!(slot.is_set(Ordering::Relaxed));
  }

  #[test]
  fn set_twice_fails() {
    let slot: AtomicOnceArcOption<i32> = AtomicOnceArcOption::new();
    slot.set(Arc::new(1), Ordering::Release).unwrap();
    let err = slot.set(Arc::new(2), Ordering::Release).unwrap_err();
    assert_eq!(*err, 2);
    assert_eq!(*slot.get(Ordering::Acquire).unwrap(), 1);
  }

  #[test]
  fn load_returns_arc() {
    let slot: AtomicOnceArcOption<&str> = AtomicOnceArcOption::new();
    let original = Arc::new("hello");
    slot.set(original.clone(), Ordering::Release).unwrap();

    let loaded = slot.load(Ordering::Acquire).unwrap();
    assert!(Arc::ptr_eq(&original, &loaded));
    assert_eq!(Arc::strong_count(&original), 3); // original + slot + loaded
  }

  #[test]
  fn into_inner_returns_arc() {
    let slot: AtomicOnceArcOption<i32> = AtomicOnceArcOption::new();
    let original = Arc::new(100);
    slot.set(original.clone(), Ordering::Release).unwrap();

    let inner = slot.into_inner().unwrap();
    assert!(Arc::ptr_eq(&original, &inner));
    assert_eq!(Arc::strong_count(&original), 2); // original + inner (slot consumed)
  }

  #[test]
  fn into_inner_empty() {
    let slot: AtomicOnceArcOption<i32> = AtomicOnceArcOption::new();
    assert!(slot.into_inner().is_none());
  }

  #[test]
  fn drop_decrements_refcount() {
    let arc = Arc::new(42);
    assert_eq!(Arc::strong_count(&arc), 1);
    {
      let slot: AtomicOnceArcOption<i32> = AtomicOnceArcOption::new();
      slot.set(arc.clone(), Ordering::Release).unwrap();
      assert_eq!(Arc::strong_count(&arc), 2);
    }
    assert_eq!(Arc::strong_count(&arc), 1);
  }

  #[test]
  fn from_arc() {
    let slot = AtomicOnceArcOption::from(Arc::new(7));
    assert_eq!(*slot.get(Ordering::Acquire).unwrap(), 7);
  }

  #[test]
  fn from_none() {
    let slot = AtomicOnceArcOption::<i32>::from(None);
    assert!(slot.get(Ordering::Acquire).is_none());
  }

  #[test]
  fn from_some() {
    let slot = AtomicOnceArcOption::from(Some(Arc::new(55)));
    assert_eq!(*slot.get(Ordering::Acquire).unwrap(), 55);
  }

  #[test]
  fn debug_fmt() {
    let slot: AtomicOnceArcOption<i32> = AtomicOnceArcOption::new();
    slot.set(Arc::new(42), Ordering::Release).unwrap();
    let dbg = format!("{:?}", slot);
    assert!(dbg.contains("42"));
  }

  #[test]
  fn concurrent_set_exactly_one_wins() {
    use std::sync::Barrier;
    use std::thread;

    let slot = Arc::new(AtomicOnceArcOption::<i32>::new());
    let barrier = Arc::new(Barrier::new(10));
    let mut handles = Vec::new();

    for i in 0..10 {
      let slot = slot.clone();
      let barrier = barrier.clone();
      handles.push(thread::spawn(move || {
        barrier.wait();
        slot.set(Arc::new(i), Ordering::Release).is_ok()
      }));
    }

    let successes: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(successes.iter().filter(|&&s| s).count(), 1);
    assert!(slot.is_set(Ordering::Relaxed));
  }

  #[test]
  fn concurrent_loads_after_set() {
    use std::sync::Barrier;
    use std::thread;

    let slot = Arc::new(AtomicOnceArcOption::from(Arc::new(99)));
    let barrier = Arc::new(Barrier::new(10));
    let mut handles = Vec::new();

    for _ in 0..10 {
      let slot = slot.clone();
      let barrier = barrier.clone();
      handles.push(thread::spawn(move || {
        barrier.wait();
        *slot.get(Ordering::Acquire).unwrap()
      }));
    }

    for h in handles {
      assert_eq!(h.join().unwrap(), 99);
    }
  }

  #[test]
  fn get_mut_empty() {
    let mut slot: AtomicOnceArcOption<i32> = AtomicOnceArcOption::new();
    assert!(slot.get_mut().is_none());
  }

  #[test]
  fn get_mut_modifies_value() {
    let mut slot: AtomicOnceArcOption<i32> = AtomicOnceArcOption::new();
    slot.set(Arc::new(10), Ordering::Release).unwrap();
    *slot.get_mut().unwrap() = 20;
    assert_eq!(*slot.get(Ordering::Acquire).unwrap(), 20);
  }
}
