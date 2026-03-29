//! # once-arc
//!
//! Initialize-once [`Arc<T>`](std::sync::Arc) containers with zero-cost reads.
//!
//! This crate provides two types for sharing data across threads where the
//! value is written once and read many times:
//!
//! - **[`OnceArc`]** — a lock-free, CAS-based initialize-once slot.
//!   You construct the [`Arc<T>`](std::sync::Arc) yourself and store it with an explicit
//!   [`Ordering`](std::sync::atomic::Ordering).
//!
//! - **[`InitOnceArc`]** — synchronized one-time initialization via a closure
//!   (similar to [`OnceLock`](std::sync::OnceLock)). The closure runs exactly
//!   once; its return value becomes the stored `Arc<T>`.
//!
//! Both types share the same fast path: once the value is set,
//! [`get()`](OnceArc::get) is a **single atomic load** — no
//! locking, no CAS, no reference-count manipulation. On x86 this compiles
//! to a plain `mov`.
//!
//! # Why not `OnceLock<Arc<T>>`?
//!
//! [`OnceLock`](std::sync::OnceLock) stores its value inline, so
//! `get()` returns `&Arc<T>` — two pointer indirections to reach `T`.
//! Here, the atomic *is* the `Arc`'s pointer, so `get()` returns `&T`
//! directly.
//!
//! # Examples
//!
//! ```
//! use std::sync::Arc;
//! use std::sync::atomic::Ordering;
//! use once_arc::OnceArc;
//!
//! let slot = OnceArc::new();
//! slot.store(Arc::new(42), Ordering::Release).unwrap();
//! assert_eq!(slot.get(Ordering::Acquire), Some(&42));
//! ```
//!
//! ```
//! use std::sync::Arc;
//! use std::sync::atomic::Ordering;
//! use once_arc::InitOnceArc;
//!
//! let cell = InitOnceArc::new();
//! cell.init(|| Arc::new("hello")).unwrap();
//! assert_eq!(cell.get(Ordering::Acquire), Some(&"hello"));
//! ```

mod init_once_arc;
mod once_arc;

pub use init_once_arc::InitOnceArc;
pub use once_arc::OnceArc;
