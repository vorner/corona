#![doc(html_root_url = "https://docs.rs/corona/0.2.1/corona/")]

//! A library combining futures and coroutines.
//!
//! The current aim of Rust in regards to asynchronous programming is on
//! [`futures`](https://crates.io/crates/futures). They have many good properties. However, they
//! tend to result in very functional-looking code (eg. monadic chaining of closures through their
//! modifiers). Some tasks are more conveniently done throuch imperative approaches.
//!
//! The aim of this library is to integrate coroutines with futures. It is possible to start a
//! coroutine. The coroutine can wait for a future to complete (which'll suspend its execution, but
//! will not block the thread ‒ the execution will switch to other futures or coroutines on the
//! same thread). A spawned coroutine is represented through a handle that acts as a future,
//! representing its completion. Therefore, other coroutines can wait for it, or it can be used in
//! the usual functional way and compose it with other futures.
//!
//! Unlike the coroutines planned for core Rust, these coroutines are stack-full (eg. you can
//! suspend them from within deep stack frame) and they are available now. This has some
//! advantages, but also costs.
//!
//! # The cost
//!
//! The coroutines are *not* zero cost, at least not now. There are these costs:
//!
//! * Each coroutine needs a stack. The stack takes some space and is allocated dynamically.
//!   Furthermore, a stack can't be allocated through the usual allocator, but is mapped directly
//!   by the OS and manipulating the memory mapping of pages is relatively expensive operation (a
//!   syscall, TLB needs to be flushed, ...). The stacks are cached and reused, but still, creating
//!   them has a cost.
//! * Each wait for a future inside a coroutine allocates dynamically (because it spawns a task
//!   inside [Tokio's](https://crates.io/crates/tokio-core) reactor core.
//!
//! Some of these costs might be mitigated or lowered in future, but for now, expect to get
//! somewhat lower performance with coroutines compared to using only futures.
//!
//! # API Stability
//!
//! Currently, the crate is in an experimental state. It exists mostly as a research if it is
//! possible to provide something like async/await in Rust and integrate it with the current
//! asynchronous stack, without adding explicit support to the language.
//!
//! It is obviously possible, but the ergonomics is something that needs some work. Therefore,
//! expect the API to change, possibly in large ways.
//!
//! Still, if you want to test it, it should probably work and might be useful. It is experimental
//! in a sense the API is not stabilized, but it is not expected to eat data or crash.
//!
//! # Known problems
//!
//! These are the problems I'm aware of and which I want to find a solution some day.
//!
//! * The current API is probably inconvinient.
//! * Many abstractions are missing. Things like waiting for a future with a timeout, or waiting
//!   for the first of many futures or streams would come handy.
//! * Many places have `'static` bounds on the types, even though these shouldn't be needed in
//!   theory.
//! * The relation with unwind safety is unclear.
//! * No support for threads (probably not even possible ‒ Rust's type system doesn't expect a
//!   stack to move from one thread to another).
//! * It relies on the tokio. It would be great if it worked with other future executors as well.
//!   This might change after the tokio refactoring.
//! * Cleaning up of stacks (and things on them) when the coroutines didn't finish yet is done
//!   through panicking. This has some ugly side effects.
//! * It is possible to create a deadlock when moving the driving tokio core inside a coroutine,
//!   like this (eg. this is an example what *not* to do):
//!
//! ```rust,no_run
//! # extern crate corona;
//! # extern crate futures;
//! # extern crate tokio_core;
//! # use std::time::Duration;
//! # use corona::Coroutine;
//! # use corona::prelude::*;
//! # use futures::Future;
//! # use futures::unsync::oneshot;
//! # use tokio_core::reactor::{Core, Timeout};
//! #
//! # fn main() {
//! let mut core = Core::new().unwrap();
//! let (sender, receiver) = oneshot::channel();
//! let handle = core.handle();
//! let c = Coroutine::with_defaults(handle.clone(), move || {
//!     core.run(receiver).unwrap();
//! });
//! Coroutine::with_defaults(handle.clone(), move || {
//!     let timeout = Timeout::new(Duration::from_millis(50), &handle).unwrap();
//!     timeout.coro_wait().unwrap();
//!     drop(sender.send(42));
//! });
//! c.wait().unwrap();
//! # }
//! ```
//!
//! # Contribution
//!
//! All kinds of contributions are welcome, including reporting bugs, improving the documentation,
//! submitting code, etc. However, the most valuable contribution for now would be trying it out
//! and providing some feedback ‒ if the thing works, where the API needs improvements, etc.
//!
//! # Examples
//!
//! One that shows the API.
//!
//! ```
//! # extern crate corona;
//! # extern crate futures;
//! # extern crate tokio_core;
//! use std::time::Duration;
//! use futures::Sink;
//! use futures::unsync::mpsc;
//! use corona::prelude::*;
//!
//! use tokio_core::reactor::{Core, Timeout};
//!
//! # fn main() {
//! let mut core = Core::new().unwrap();
//! let handle = core.handle();
//! let builder = Coroutine::new(core.handle());
//! let (sender, receiver) = mpsc::channel(1);
//! builder.spawn(|| {
//!     let mut sender = sender;
//!     sender = sender.send(1).coro_wait().unwrap();
//!     sender = sender.send(2).coro_wait().unwrap();
//! });
//! let coroutine = builder.spawn(move || {
//!     for item in receiver.iter_ok() {
//!         println!("{}", item);
//!     }
//!
//!     let timeout = Timeout::new(Duration::from_millis(100), &handle).unwrap();
//!     timeout.coro_wait().unwrap();
//!
//!     42
//! });
//! assert_eq!(42, core.run(coroutine).unwrap());
//! # }
//! ```
//!
//! Further examples can be found in the
//! [repository](https://github.com/vorner/corona/tree/master/examples).

// XXX Add docs
// XXX More examples and explanation how to use
// XXX Switch submodules to pub(crate) where possible/appropriate

extern crate context;
extern crate futures;
extern crate tokio_core;

pub mod errors;
pub mod prelude;
pub mod wrappers;

mod coroutine;
mod stack_cache;
mod switch;

pub use errors::{Dropped, TaskFailed};
pub use coroutine::{Coroutine, CoroutineResult, TaskResult};
