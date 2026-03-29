[![Build](https://github.com/SrTobi/once-arc/actions/workflows/rust.yml/badge.svg)](https://github.com/SrTobi/once-arc)
[![Creates.io](https://img.shields.io/crates/v/once-arc?style)](https://crates.io/crates/once-arc)
[![Docs](https://docs.rs/once-arc/badge.svg)](https://docs.rs/once-arc/)

# once-arc

A lock-free, thread-safe container that can be atomically initialized once with an `Arc<T>`.

Think of it as `Atomic<Option<Arc<T>>>` — but with a critical restriction:
**the value can only be set once**. In return, reads are _extremely fast_:
a single atomic read, no reference count manipulation, no locking.

## Usage
## Quick start

### `OnceArc` — low-level, lock-free

```rust
use std::sync::Arc;
use std::sync::atomic::Ordering;
use once_arc::OnceArc;

let slot: OnceArc<i32> = OnceArc::new();

// Set it once
slot.store(Arc::new(42), Ordering::Release).unwrap();

// get() returns &T — a single atomic load, no refcount overhead
assert_eq!(slot.get(Ordering::Acquire), Some(&42));

// load() clones the Arc when you need ownership
let arc = slot.load(Ordering::Acquire).unwrap();
assert_eq!(*arc, 42);

// Second store fails, returning the value back
let err = slot.store(Arc::new(99), Ordering::Release).unwrap_err();
assert_eq!(*err, 99);
```

### `InitOnceArc` — Mutex protected initialization

```rust
use std::sync::Arc;
use std::sync::atomic::Ordering;
use once_arc::InitOnceArc;

let cell: InitOnceArc<String> = InitOnceArc::new();

// First call runs the closure
cell.init(|| {
    // While the closure is running, we are holding a mutex
    // so no other thread can set the cell.
    // Load accesses will not block and see that the cell is still empty.
    Arc::new("hello".to_string())
}).unwrap();

// The value is already set, so the closure is not run
cell.init(|| unreachable!()).unwrap();

assert_eq!(cell.get(Ordering::Acquire).unwrap(), "hello");
```

Multiple threads can race to call `store`/`init`/`try_init`; exactly one will run the
closure, and the rest will block briefly on the mutex.

## API overview

### `OnceArc<T>`

| Method                    | Returns              | Cost                          |
| ------------------------- | -------------------- | ----------------------------- |
| `get(Ordering)`           | `Option<&T>`         | Single atomic load            |
| `load(Ordering)`          | `Option<Arc<T>>`     | Atomic load + `Arc::clone`    |
| `store(Arc<T>, Ordering)` | `Result<(), Arc<T>>` | One CAS                       |
| `is_set(Ordering)`        | `bool`               | Single atomic load            |
| `into_inner()`            | `Option<Arc<T>>`     | No atomic ops (consumes self) |
| `get_mut()`               | `Option<&mut T>`     | No atomic ops (exclusive ref) |

### `InitOnceArc<T>`

| Method             | Returns                                   | Cost                                 |
| ------------------ | ----------------------------------------- | ------------------------------------ |
| `get(Ordering)`    | `Option<&T>`                              | Single atomic load                   |
| `load(Ordering)`   | `Option<Arc<T>>`                          | Atomic load + `Arc::clone`           |
| `store(Arc<T>)`    | `Result<(), Result<Arc<T>, PoisonError>>` | Mutex lock + CAS                     |
| `init(f)`          | `Result<bool, PoisonError>`               | Mutex lock + closure (once)          |
| `try_init(f)`      | `Result<bool, Result<E, PoisonError>>`    | Mutex lock + fallible closure (once) |
| `is_set(Ordering)` | `bool`                                    | Single atomic load                   |
| `into_inner()`     | `Option<Arc<T>>`                          | No atomic ops (consumes self)        |
| `get_mut()`        | `Option<&mut T>`                          | No atomic ops (exclusive ref)        |

## Why not just use `OnceLock<Arc<T>>`?

`OnceLock` stores the value inline. `get()` returns `&Arc<T>`, so
callers must go through two pointer indirections to reach `T`, and
cloning requires an `Arc::clone`. With `OnceArc`, the atomic
_is_ the `Arc`'s pointer — `get()` returns `&T` directly with a single
atomic load and zero indirection beyond the pointer itself.

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

### How initialize-once sidesteps all of this

`OnceArc` avoids every one of these problems with a single
rule: **the pointer, once written, never changes**.

- **No use-after-free**: the `Arc` is alive from the moment of `store()`
  until the `OnceArc` is dropped. No thread can remove it in
  between.
- **No ABA problem**: the value transitions from null to a pointer
  exactly once. No pointer is ever reused.
- **No deferred reclamation**: since the pointer is never swapped out,
  there's nothing to reclaim while readers exist.
- **No refcount on `get()`**: `get()` returns `&T` tied to the lifetime of
  `&self`. Since the data can't be freed while a shared reference to the
  container exists, this is sound without touching the reference count.

The result: `get()` compiles down to a single atomic load instruction
(which on x86 is just a plain `mov`), making it effectively zero-cost.
