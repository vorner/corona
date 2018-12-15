#![doc(html_root_url = "https://docs.rs/corona/0.4.1/corona/")]
#![warn(missing_docs)]

//! A library combining futures and coroutines.
//!
//! This library brings stack-full coroutines. Each coroutine can asynchronously wait on futures
//! and provides a future of its result.
//!
//! # Motivation
//!
//! The current aim of Rust in regards to asynchronous programming is on
//! [`futures`](https://crates.io/crates/futures). They have some good properties, but some tasks
//! are more conveniently done in a more imperative way.
//!
//! There's the work in progress of [async-await](https://github.com/alexcrichton/futures-await).
//! But it requires nightly (for now), provides stack-less coroutines (which means the asynchronous
//! waiting can be done in a top-level function only) and there are too many `'static` bounds.
//! Something might improve over time.
//!
//! This library brings a more convenient interface. However, it comes with a run-time cost, so you
//! might want to consider if you prefer ease of development or memory efficiency. Often, the
//! asynchronous communication isn't the bottleneck and you won't be handling millions of
//! concurrent connections, only tens of thousands, so this might be OK.
//!
//! # Pros
//!
//! * Easier to use than futures.
//! * Can integrate with futures.
//! * Allows working with borrowed futures.
//! * Provides safe interface.
//!
//! # Cons
//!
//! * Each coroutine needs its own stack, which is at least few memory pages large. This makes the
//!   library unsuitable when there are many concurrent coroutines.
//! * The coroutines can't move between threads once created safely. This library is tied into
//!   [`tokio-current-threaded`](https://crates.io/crates/tokio-current-thread) executor (it might
//!   be possible to remove this tie-in, but not the single-threadiness).
//!
//! # How to use
//!
//! By bringing the `corona::prelude::*` into scope, all `Future`s, `Stream`s and `Sink`s get new
//! methods for asynchronous waiting on their completion or progress.
//!
//! The [`Coroutine`](coroutine/struct.Coroutine.html) is used to start a new coroutine. It acts a
//! bit like `std::thread::spawn`. However, all the coroutines run on the current thread and switch
//! to other coroutines whenever they wait on something. This must be done from the context of
//! current-thread executor, which is easiest by wrapping the whole application into
//! `tokio::runtime::current_thread::block_on_all` and `future::lazy`.
//!
//! There's also the [`Coroutine::run`](coroutine.struct.Coroutine.html#method.run) that does the
//! same (but is available only in case the `convenient-run` feature is not turned off).
//!
//! Each new coroutine returns a future. It resolves whenever the coroutine terminates. However,
//! the coroutine is *eager* ‒ it doesn't wait with the execution for the future to be polled. The
//! future can be dropped and the coroutine will still execute.
//!
//! ```rust
//! extern crate corona;
//! extern crate tokio;
//!
//! use std::time::Duration;
//!
//! use corona::prelude::*;
//! use tokio::clock;
//! use tokio::prelude::*;
//! use tokio::runtime::current_thread;
//! use tokio::timer::Delay;
//!
//! fn main() {
//!     let result = current_thread::block_on_all(future::lazy(|| {
//!         Coroutine::with_defaults(|| {
//!             let timeout = Delay::new(clock::now() + Duration::from_millis(50));
//!             timeout.coro_wait().unwrap(); // Timeouts don't error
//!             42
//!         })
//!     })).unwrap();
//!     assert_eq!(42, result);
//! }
//! ```
//!
//! ```rust
//! extern crate corona;
//! extern crate tokio;
//!
//! use std::time::Duration;
//!
//! use corona::prelude::*;
//! use tokio::clock;
//! use tokio::prelude::*;
//! use tokio::runtime::current_thread;
//! use tokio::timer::Delay;
//!
//! fn main() {
//!     let result = Coroutine::new()
//!         .run(|| {
//!             let timeout = Delay::new(clock::now() + Duration::from_millis(50));
//!             timeout.coro_wait().unwrap(); // Timeouts don't error
//!             42
//!         }).unwrap();
//!     assert_eq!(42, result);
//! }
//! ```
//!
//! Few things of note:
//!
//! * All the coroutine-aware methods panic outside of a coroutine.
//! * Many of them panic outside of a current-thread executor.
//! * You can freely mix future and coroutine approach. Therefore, you can use combinators to build
//!   a future and then `coro_wait` on it.
//! * A coroutine spawned by [`spawn`](coroutine/struct.Coroutine.html#method.spawn) or
//!   [`with_defaults`](coroutine/struct.Coroutine.html#method.with_defaults) will propagate panics
//!   outside. One spawned with
//!   [`spawn_catch_panic`](coroutine/struct.Coroutine.html#method.spawn_catch_panic) captures the
//!   panic and passes it on through its result.
//! * Panicking outside of the coroutine where the executor runs may lead to ugly things, like
//!   aborting the program (this'd usually lead to a double panic).
//! * Any of the waiting methods may switch to a different coroutine. Therefore it is not a good
//!   idea to hold a `RefCell` borrowed or a `Mutex` locked around that if another coroutine could
//!   also borrow it.
//!
//! The new methods are here:
//!
//! * [`Future`s](prelude/trait.CoroutineFuture.html)
//! * [`Stream`s](prelude/trait.CoroutineStream.html)
//! * [`Sink`s](prelude/trait.CoroutineSink.html)
//!
//! ## Coroutine-blocking IO
//!
//! Furthermore, if the `blocking-wrappers` feature is enabled (it is by default), all `AsyncRead`
//! and `AsyncWrite` objects can be wrapped in
//! [`corona::io::BlockingWrapper`](io/struct.BlockingWrapper.html). This implements
//! `Read` and `Write` in a way that mimics blocking, but it blocks only the coroutine, not the
//! whole thread. This allows it to be used with usual blocking routines, like
//! `serde_json::from_reader`.
//!
//! The API is still a bit rough (it exposes just the `Read` and `Write` traits, all other methods
//! need to be accessed through `.inner()` or `.inner_mut`), this will be improved in future
//! versions.
//!
//! ```
//! # extern crate corona;
//! # extern crate tokio;
//! use std::io::{Read, Result as IoResult};
//! use corona::io::BlockingWrapper;
//! use tokio::net::TcpStream;
//!
//! fn blocking_read(connection: &mut TcpStream) -> IoResult<()> {
//!     let mut connection = BlockingWrapper::new(connection);
//!     let mut buf = [0u8; 64];
//!     // This will block the coroutine, but not the thread
//!     connection.read_exact(&mut buf)
//! }
//!
//! # fn main() {}
//! ```
//!
//! # Cleaning up
//!
//! If the executor is dropped while a coroutine waits on something, the waiting method will panic.
//! That way the coroutine's stack is unwinded, releasing resources on its stack (there doesn't
//! seem to be a better way to drop the whole stack).
//!
//! However, if the executor is dropped because of a panic, Rust abort the whole program because of
//! a double-panic. Ideas how to overcome this (since the second panic is on a different stack, but
//! Rust doesn't know that) are welcome.
//!
//! There are waiting methods that return an error instead of panicking, but they are less
//! convenient to use.
//!
//! It also can be configured to leak the stack in such case instead of double-panicking.
//!
//! # Pitfalls
//!
//! If the coroutine is created with default configuration, it gets really small stack. If you
//! overflow it, you get a segfault (it happens more often on debug builds than release ones) with
//! really useless backtrace. Try making the stack bigger in that case.
//!
//! # API Stability
//!
//! The API is likely to get stabilized soon (I hope it won't change much any more). But I still
//! want to do more experimentation before making it official.
//!
//! There are two areas where I expect some changes will still be needed:
//!
//! * I want to support scoped coroutines (similar to some libraries that provide scoped threads).
//!
//! Other experiments from consumers are also welcome.
//!
//! # Known problems
//!
//! These are the problems I'm aware of and which I want to find a solution some day.
//!
//! * Many handy abstractions are still missing, like waiting for a future with a timeout, or
//!   conveniently waiting for a first of a set of futures or streams.
//! * The coroutines can't move between threads. This is likely impossible, since Rust's type
//!   system doesn't expect whole stacks with all local data to move.
//! * It relies on Tokio. This might change in the future.
//! * The API doesn't prevent some footguns ‒ leaving a `RefCell` borrowed across coroutine switch,
//!   deadlocking, calling the waiting methods outside of a coroutine or using `.wait()` by a
//!   mistake and blocking the whole thread. These manifest during runtime.
//! * The cleaning up of coroutines when the executor is dropped is done through panics.
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
//! # extern crate tokio;
//! use std::time::Duration;
//! use futures::unsync::mpsc;
//! use corona::prelude::*;
//! use tokio::clock;
//! use tokio::prelude::*;
//! use tokio::runtime::current_thread;
//! use tokio::timer::Delay;
//!
//! # fn main() {
//! let result = Coroutine::new().run(|| {
//!     let (sender, receiver) = mpsc::channel(1);
//!     corona::spawn(|| {
//!         let mut sender = sender;
//!         sender = sender.send(1).coro_wait().unwrap();
//!         sender = sender.send(2).coro_wait().unwrap();
//!     });
//!
//!     for item in receiver.iter_ok() {
//!         println!("{}", item);
//!     }
//!
//!     let timeout = Delay::new(clock::now() + Duration::from_millis(100));
//!     timeout.coro_wait().unwrap();
//!
//!     42
//! }).unwrap();
//! assert_eq!(42, result);
//! # }
//! ```
//!
//! Further examples can be found in the
//! [repository](https://github.com/vorner/corona/tree/master/examples).
//!
//! # Behind the scenes
//!
//! There are few things that might help understanding how the library works inside.
//!
//! First, there's some thread-local state. This state is used for caching currently unused stacks
//! as well as the state that is used when waiting for something and switching coroutines.
//!
//! Whenever one of the waiting methods is used, a wrapper future is created. After the original
//! future resolves, it resumes the execution to the current stack. This future is spawned onto the
//! executor and a switch is made to the parent coroutine (it's the coroutine that started or
//! resumed the current one). This way, the „outside“ coroutine is reached eventually. It is
//! expected this outside coroutine will run the executor, waking up the ready to proceed
//! coroutines and then switching to them.
//!
//! That's about it, the rest of the library are just implementation details about what is stored
//! where and how to pass the information around without breaking any lifetime bounds.

extern crate context;
extern crate futures;
#[cfg(any(test, feature = "convenient-run"))]
extern crate tokio;
extern crate tokio_current_thread;
#[cfg(feature = "blocking-wrappers")]
extern crate tokio_io;

#[cfg(feature = "blocking-wrappers")]
pub mod io;
pub mod coroutine;
pub mod errors;
pub mod prelude;
pub mod wrappers;

mod stack_cache;
mod switch;

pub use errors::{Dropped, TaskFailed};
pub use coroutine::{spawn, Coroutine, CoroutineResult};
