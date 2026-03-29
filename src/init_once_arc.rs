use std::fmt;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};

use crate::OnceArc;

/// A thread-safe container that provides synchronized one-time initialization
/// of an `Arc<T>` via a closure.
///
/// This builds on [`OnceArc`] by adding `init` and `try_init` methods.
/// When multiple threads call `init` concurrently on an empty cell, exactly
/// one thread will execute the closure — the others block on an internal
/// mutex and then observe the already-initialized value. If the closure
/// completes without panicking, its return value becomes the stored `Arc<T>`.
/// If it panics, the mutex is poisoned and subsequent calls return an error.
///
/// Once set, all reads go through the fast path: a single atomic load,
/// identical to [`OnceArc::get`] — no mutex, no CAS.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use std::sync::atomic::Ordering;
/// use once_arc::InitOnceArc;
///
/// let cell: InitOnceArc<String> = InitOnceArc::new();
///
/// // First call runs the initializer
/// assert_eq!(cell.init(|| "hello".to_string().into()).unwrap(), true);
///
/// // Subsequent calls return Ok(false) without running the closure
/// assert_eq!(cell.init(|| "world".to_string().into()).unwrap(), false);
/// assert_eq!(cell.get(Ordering::Acquire).unwrap(), "hello");
/// ```
pub struct InitOnceArc<T> {
  inner: OnceArc<T>,
  init_mutex: Mutex<()>,
}

// SAFETY: Same reasoning as OnceArc — T is behind an Arc
// and only accessible via shared reference. The Mutex is Send + Sync.
unsafe impl<T: Send + Sync> Send for InitOnceArc<T> {}
unsafe impl<T: Send + Sync> Sync for InitOnceArc<T> {}

impl<T> InitOnceArc<T> {
  /// Creates a new empty `InitOnceArc`.
  ///
  /// # Examples
  ///
  /// ```
  /// use once_arc::InitOnceArc;
  ///
  /// let cell: InitOnceArc<i32> = InitOnceArc::new();
  /// ```
  pub const fn new() -> Self {
    Self {
      inner: OnceArc::new(),
      init_mutex: Mutex::new(()),
    }
  }

  /// Attempts to store the value. Returns `Ok(())` on success,
  /// `Err(Ok(value))` if a value was already stored, or `Err(Err(_))` if the
  /// mutex is poisoned.
  ///
  /// If an init attempt is already ongoing, this `store` will wait for it,
  /// before trying to store.
  ///
  /// # Examples
  ///
  /// ```
  /// use std::sync::Arc;
  /// use once_arc::InitOnceArc;
  ///
  /// let cell: InitOnceArc<i32> = InitOnceArc::new();
  /// assert!(cell.store(Arc::new(42)).is_ok());
  ///
  /// let err = cell.store(Arc::new(99)).unwrap_err().unwrap();
  /// assert_eq!(*err, 99);
  /// ```
  pub fn store(&self, value: Arc<T>) -> Result<(), Result<Arc<T>, PoisonError<()>>> {
    let _guard = self.init_mutex.lock().map_err(|_| Err(PoisonError::new(())))?;
    self.inner.store(value, Ordering::SeqCst).map_err(Ok)
  }

  /// Initializes the cell with the value produced by `f`, if not yet set.
  /// Returns `Ok(true)` if the value was initialized, `Ok(false)` if it was
  /// already set, or `Err` if the mutex is poisoned.
  ///
  /// Only one initializer will be run per [`InitOnceArc`] and if the initializer doesn't panic,
  /// its return value is guaranteed to become the stored `Arc<T>`.
  /// If it panics, the mutex is poisoned and subsequent calls will return an error.
  ///
  /// While the initializer is running all readers will see the cell as empty.
  ///
  /// # Examples
  ///
  /// ```
  /// use std::sync::Arc;
  /// use once_arc::InitOnceArc;
  ///
  /// let cell: InitOnceArc<i32> = InitOnceArc::new();
  /// assert_eq!(cell.init(|| Arc::new(42)).unwrap(), true);
  /// assert_eq!(cell.init(|| Arc::new(99)).unwrap(), false); // already initialized
  /// ```
  pub fn init(&self, f: impl FnOnce() -> Arc<T>) -> Result<bool, PoisonError<()>> {
    // Slow path: acquire mutex, double-check, initialize
    let _guard = self.init_mutex.lock().map_err(|_| PoisonError::new(()))?;
    if self.inner.is_set(Ordering::SeqCst) {
      return Ok(false);
    }

    let arc = f();
    self
      .inner
      .store(arc, Ordering::SeqCst)
      .unwrap_or_else(|_| unreachable!("store failed while holding init mutex"));
    Ok(true)
  }

  /// Initializes the cell with the value produced by `f`, if not yet set.
  /// If `f` returns `Err`, the cell remains empty and the error is propagated.
  /// Returns `Ok(true)` if initialized, `Ok(false)` if already set,
  /// `Err(Ok(e))` if `f` failed, or `Err(Err(_))` if the mutex is poisoned.
  ///
  /// Only one initializer will be run per [`InitOnceArc`] at a time and if the initializer doesn't panic or return an error,
  /// its return value is guaranteed to become the stored `Arc<T>`.
  /// If it panics, the mutex is poisoned and subsequent calls will return an error.
  ///
  /// While the initializer is running all readers will see the cell as empty.
  ///
  /// # Examples
  ///
  /// ```
  /// use std::sync::Arc;
  /// use once_arc::InitOnceArc;
  ///
  /// let cell: InitOnceArc<i32> = InitOnceArc::new();
  ///
  /// let err = cell.try_init(|| Err("oops")).unwrap_err().unwrap();
  /// assert_eq!(err, "oops");
  ///
  /// assert_eq!(cell.try_init(|| Ok::<_, &str>(Arc::new(42))).unwrap(), true);
  /// assert_eq!(cell.try_init(|| Ok::<_, &str>(Arc::new(99))).unwrap(), false);
  /// ```
  pub fn try_init<E>(&self, f: impl FnOnce() -> Result<Arc<T>, E>) -> Result<bool, Result<E, PoisonError<()>>> {
    // Slow path
    let _guard = self.init_mutex.lock().map_err(|_| Err(PoisonError::new(())))?;
    if self.inner.is_set(Ordering::SeqCst) {
      return Ok(false);
    }

    let value = match f() {
      Ok(v) => v,
      Err(e) => return Err(Ok(e)),
    };
    self
      .inner
      .store(value, Ordering::SeqCst)
      .unwrap_or_else(|_| unreachable!("store failed while holding init mutex"));
    Ok(true)
  }

  /// Returns a reference to the stored value, or `None` if not yet initialized.
  ///
  /// This is the fast path — a single atomic load, no mutex.
  ///
  /// # Examples
  ///
  /// ```
  /// use std::sync::Arc;
  /// use std::sync::atomic::Ordering;
  /// use once_arc::InitOnceArc;
  ///
  /// let cell: InitOnceArc<i32> = InitOnceArc::new();
  /// assert!(cell.get(Ordering::Acquire).is_none());
  ///
  /// cell.init(|| Arc::new(42)).unwrap();
  /// assert_eq!(cell.get(Ordering::Acquire), Some(&42));
  /// ```
  pub fn get(&self, ordering: Ordering) -> Option<&T> {
    self.inner.get(ordering)
  }

  /// Loads the stored value as a cloned `Arc<T>`, or `None` if not yet
  /// initialized.
  ///
  /// # Examples
  ///
  /// ```
  /// use std::sync::Arc;
  /// use std::sync::atomic::Ordering;
  /// use once_arc::InitOnceArc;
  ///
  /// let cell: InitOnceArc<i32> = InitOnceArc::new();
  /// cell.init(|| Arc::new(42)).unwrap();
  /// let arc = cell.load(Ordering::Acquire).unwrap();
  /// assert_eq!(*arc, 42);
  /// ```
  pub fn load(&self, ordering: Ordering) -> Option<Arc<T>> {
    self.inner.load(ordering)
  }

  /// Returns `true` if the value has been initialized.
  ///
  /// # Examples
  ///
  /// ```
  /// use std::sync::Arc;
  /// use std::sync::atomic::Ordering;
  /// use once_arc::InitOnceArc;
  ///
  /// let cell: InitOnceArc<i32> = InitOnceArc::new();
  /// assert!(!cell.is_set(Ordering::Relaxed));
  /// cell.init(|| Arc::new(1)).unwrap();
  /// assert!(cell.is_set(Ordering::Relaxed));
  /// ```
  pub fn is_set(&self, ordering: Ordering) -> bool {
    self.inner.is_set(ordering)
  }

  /// Consumes `self` and returns the stored `Arc<T>`, if any.
  ///
  /// # Examples
  ///
  /// ```
  /// use std::sync::Arc;
  /// use once_arc::InitOnceArc;
  ///
  /// let cell: InitOnceArc<i32> = InitOnceArc::new();
  /// cell.init(|| Arc::new(42)).unwrap();
  /// let arc = cell.into_inner().unwrap();
  /// assert_eq!(*arc, 42);
  /// ```
  pub fn into_inner(self) -> Option<Arc<T>> {
    self.inner.into_inner()
  }

  /// Returns a mutable reference to the stored value, or `None` if not yet set.
  ///
  /// Since this requires `&mut self`, no atomic operations or mutex are needed.
  ///
  /// # Examples
  ///
  /// ```
  /// use std::sync::Arc;
  /// use std::sync::atomic::Ordering;
  /// use once_arc::InitOnceArc;
  ///
  /// let mut cell: InitOnceArc<i32> = InitOnceArc::new();
  /// cell.init(|| Arc::new(10)).unwrap();
  /// *cell.get_mut().unwrap() = 20;
  /// assert_eq!(cell.get(Ordering::Acquire), Some(&20));
  /// ```
  pub fn get_mut(&mut self) -> Option<&mut T> {
    self.inner.get_mut()
  }
}

impl<T> Default for InitOnceArc<T> {
  fn default() -> Self {
    Self::new()
  }
}

impl<T: fmt::Debug> fmt::Debug for InitOnceArc<T> {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    f.debug_struct("InitOnceArc")
      .field("value", &self.inner.get(Ordering::SeqCst))
      .finish()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::Arc;
  use std::sync::atomic::Ordering;

  #[test]
  fn once_arc_init() {
    let cell: InitOnceArc<i32> = InitOnceArc::new();
    assert_eq!(cell.init(|| Arc::new(42)).unwrap(), true);
    assert_eq!(cell.init(|| Arc::new(99)).unwrap(), false);
    assert_eq!(cell.get(Ordering::Acquire), Some(&42));
  }

  #[test]
  fn once_arc_get_empty() {
    let cell: InitOnceArc<i32> = InitOnceArc::new();
    assert!(cell.get(Ordering::Acquire).is_none());
  }

  #[test]
  fn once_arc_try_init_err_then_ok() {
    let cell: InitOnceArc<i32> = InitOnceArc::new();
    let err = cell.try_init(|| Err("fail")).unwrap_err().unwrap();
    assert_eq!(err, "fail");
    assert!(cell.get(Ordering::Acquire).is_none());

    assert_eq!(cell.try_init(|| Ok::<_, &str>(Arc::new(42))).unwrap(), true);
    assert_eq!(cell.get(Ordering::Acquire), Some(&42));
    assert_eq!(cell.try_init(|| Ok::<_, &str>(Arc::new(99))).unwrap(), false);
  }

  #[test]
  fn once_arc_load_and_is_set() {
    let cell: InitOnceArc<i32> = InitOnceArc::new();
    assert!(!cell.is_set(Ordering::Relaxed));
    assert!(cell.load(Ordering::Acquire).is_none());

    cell.init(|| Arc::new(7)).unwrap();
    assert!(cell.is_set(Ordering::Relaxed));
    assert_eq!(*cell.load(Ordering::Acquire).unwrap(), 7);
  }

  #[test]
  fn once_arc_into_inner() {
    let cell: InitOnceArc<i32> = InitOnceArc::new();
    cell.init(|| Arc::new(42)).unwrap();
    let arc = cell.into_inner().unwrap();
    assert_eq!(*arc, 42);
  }

  #[test]
  fn once_arc_into_inner_empty() {
    let cell: InitOnceArc<i32> = InitOnceArc::new();
    assert!(cell.into_inner().is_none());
  }

  #[test]
  fn once_arc_concurrent_init() {
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;
    use std::thread;

    let cell = Arc::new(InitOnceArc::<i32>::new());
    let init_count = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(10));
    let mut handles = Vec::new();

    for _ in 0..10 {
      let cell = cell.clone();
      let init_count = init_count.clone();
      let barrier = barrier.clone();
      handles.push(thread::spawn(move || {
        barrier.wait();
        cell
          .init(|| {
            init_count.fetch_add(1, Ordering::Relaxed);
            Arc::new(42)
          })
          .unwrap()
      }));
    }

    let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|&&b| b).count(), 1);
    assert_eq!(cell.get(Ordering::Acquire), Some(&42));
    // Exactly one thread ran the initializer
    assert_eq!(init_count.load(Ordering::Relaxed), 1);
  }

  #[test]
  fn once_arc_debug_fmt() {
    let cell: InitOnceArc<i32> = InitOnceArc::new();
    cell.init(|| Arc::new(42)).unwrap();
    let dbg = format!("{:?}", cell);
    assert!(dbg.contains("42"));
  }

  /// Poisons the mutex by panicking inside `init` on another thread.
  fn poisoned_cell() -> Arc<InitOnceArc<i32>> {
    use std::thread;
    let cell = Arc::new(InitOnceArc::<i32>::new());
    let c = cell.clone();
    let _ = thread::spawn(move || {
      let _ = c.init(|| panic!("deliberate poison"));
    })
    .join();
    cell
  }

  #[test]
  fn store_returns_poison_error() {
    let cell = poisoned_cell();
    let err = cell.store(Arc::new(1)).unwrap_err();
    assert!(err.is_err()); // Err(PoisonError)
  }

  #[test]
  fn init_returns_poison_error() {
    let cell = poisoned_cell();
    assert!(cell.init(|| Arc::new(1)).is_err());
  }

  #[test]
  fn try_init_returns_poison_error() {
    let cell = poisoned_cell();
    let err = cell.try_init(|| Ok::<_, &str>(Arc::new(1))).unwrap_err();
    assert!(err.is_err()); // Err(PoisonError)
  }
}
