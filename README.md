# atomic-once-arc

A lock-free, thread-safe container that can be atomically initialized once with an `Arc<T>`.

Think of it as `Atomic<Option<Arc<T>>>` — but with a critical restriction:
**the value can only be set once**. In return, reads are _extremely fast_:
a single atomic read, no reference count manipulation, no locking.

## Usage

```rust
use std::sync::Arc;
use std::sync::atomic::Ordering;
use atomic_once_arc::AtomicOnceArcOption;

// Start empty
let slot: AtomicOnceArcOption<i32> = AtomicOnceArcOption::new();
assert!(slot.get(Ordering::Acquire).is_none());

// Set it once
slot.store(Arc::new(42), Ordering::Release).unwrap();

// get() returns &T — just a single atomic load
assert_eq!(*slot.get(Ordering::Acquire).unwrap(), 42);

// load() returns a cloned Arc
let arc = slot.load(Ordering::Acquire).unwrap();
assert_eq!(*arc, 42);

// Storing again fails, returning the value
let err = slot.store(Arc::new(99), Ordering::Release).unwrap_err();
assert_eq!(*err, 99);
```

## API

| Method                    | Returns              | Cost                                      |
| ------------------------- | -------------------- | ----------------------------------------- |
| `get(Ordering)`           | `Option<&T>`         | Single atomic load (no refcount overhead) |
| `load(Ordering)`          | `Option<Arc<T>>`     | Atomic load + refcount increment          |
| `store(Arc<T>, Ordering)` | `Result<(), Arc<T>>` | One CAS operation                         |
| `is_set(Ordering)`        | `bool`               | Single atomic load                        |
| `into_inner()`            | `Option<Arc<T>>`     | No atomic ops (consumes self)             |

## Why is a general `Atomic<Arc<T>>` so hard?

A general-purpose `Atomic<Arc<T>>` that supports multiple stores is
deceptively difficult to implement correctly. Here's why:

### The fundamental race condition

Consider a naive `load` implementation for `Atomic<Arc<T>>`:

1. **Thread A** atomically reads the raw pointer from the atomic slot.
2. **Thread B** atomically swaps in a new `Arc`, then drops the old one.
3. The old `Arc`'s reference count hits zero — the data is freed.
4. **Thread A** tries to increment the reference count of the now-freed pointer. **Use-after-free.**

The core problem: between reading the pointer and incrementing the
reference count, another thread can remove and destroy the pointed-to
data. There is no way to make "read pointer" and "increment refcount"
into a single atomic operation on standard hardware.

### Solutions exist, but they're expensive

Real implementations of atomic shared pointers (like C++20's
`std::atomic<std::shared_ptr<T>>` or Rust's `arc-swap` crate) use
techniques like:

- **Hazard pointers** — readers publish which pointers they're looking
  at, and writers defer freeing memory until no reader holds a hazard on
  it. Adds per-thread bookkeeping and slows down both loads and stores.

- **Epoch-based reclamation** — threads enter/exit "epochs" and memory
  is only freed once all threads have advanced past the epoch where the
  pointer was removed. Requires cooperative epoch advancement.

- **Split reference counts** — maintain both a "global" and "local"
  reference count, using the global count to defer destruction. Loads
  must still atomically modify the global count.

All of these approaches add overhead to loads — exactly the operation
that's typically on the hot path.

### How set-once sidesteps all of this

`AtomicOnceArcOption` avoids every one of these problems with a single
rule: **the pointer, once written, never changes**.

- **No use-after-free**: the `Arc` is alive from the moment of `set()`
  until the `AtomicOnceArcOption` is dropped. No thread can remove it in
  between.
- **No ABA problem**: the value transitions from null to a pointer
  exactly once. No pointer is ever reused.
- **No deferred reclamation**: since the pointer is never swapped out,
  there's nothing to reclaim while readers exist.
- **No refcount on load**: `load()` returns `&T` tied to the lifetime of
  `&self`. Since the data can't be freed while a shared reference to the
  container exists, this is sound without touching the reference count.

The result: `load()` compiles down to a single atomic load instruction
(which on x86 is just a plain `mov`), making it effectively zero-cost.
