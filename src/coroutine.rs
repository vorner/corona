//! The [`Coroutine`](struct.Coroutine.html) and related things.

use std::any::Any;
use std::cell::RefCell;
use std::mem;
use std::panic::{self, AssertUnwindSafe, UnwindSafe};

use context::Context;
use context::stack::{Stack, ProtectedFixedSizeStack};
use futures::{Async, Future, Poll};
use futures::unsync::oneshot::{self, Receiver};
use tokio_core::reactor::Handle;

use errors::{Dropped, StackError, TaskFailed};
use switch::{Switch, WaitTask};

enum TaskResult<R> {
    Panicked(Box<Any + Send + 'static>),
    PanicPropagated,
    Lost,
    Finished(R),
}

/// A `Future` representing a completion of a coroutine.
///
/// Returns the result of the task the coroutine runs or the reason why it failed (it got lost
/// during shutdown or panicked).
pub struct CoroutineResult<R> {
    receiver: Receiver<TaskResult<R>>,
}

impl<R> Future for CoroutineResult<R> {
    type Item = R;
    type Error = TaskFailed;
    fn poll(&mut self) -> Poll<R, TaskFailed> {
        match self.receiver.poll() {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(TaskResult::Finished(result))) => Ok(Async::Ready(result)),
            Ok(Async::Ready(TaskResult::Panicked(reason))) => Err(TaskFailed::Panicked(reason)),
            Ok(Async::Ready(TaskResult::PanicPropagated)) => Err(TaskFailed::PanicPropagated),
            Ok(Async::Ready(TaskResult::Lost)) | Err(_) => Err(TaskFailed::Lost),
        }
    }
}

struct CoroutineContext {
    /// Use this to spawn waiting coroutines
    handle: Handle,
    /// The context that called us and we'll switch back to it when we wait for something.
    parent_context: Context,
    /// Our own stack. We keep ourselvel alive.
    stack: ProtectedFixedSizeStack,
    /// Do we want to leak this coroutine if the core is dropped due to a panic?
    leak_on_panic: bool,
}

thread_local! {
    static CONTEXTS: RefCell<Vec<CoroutineContext>> = RefCell::new(Vec::new());
}

/// A builder of coroutines.
///
/// This struct is the main entry point and a way to start coroutines of various kinds. It allows
/// both starting them with default parameters and configuring them with the builder pattern.
#[derive(Clone)]
pub struct Coroutine {
    handle: Handle,
    stack_size: usize,
    leak_on_panic: bool,
}

impl Coroutine {
    /// Starts building a coroutine.
    ///
    /// This constructor produces a new builder for coroutines. The builder can then be used to
    /// specify configuration of the coroutines.
    ///
    /// It is possible to spawn multiple coroutines from the same builder.
    ///
    /// # Parameters
    ///
    /// * `handle`: The coroutines need a reactor core to run on and schedule their control
    ///   switches. This is the handle to the reactor core to be used.
    ///
    /// # Examples
    ///
    /// ```
    /// # extern crate corona;
    /// # extern crate tokio_core;
    /// use corona::Coroutine;
    /// use tokio_core::reactor::Core;
    ///
    /// # fn main() {
    /// let core = Core::new().unwrap();
    /// let builder = Coroutine::new(core.handle());
    ///
    /// let coroutine = builder.spawn(|| { });
    /// # }
    ///
    /// ```
    pub fn new(handle: Handle) -> Self {
        Coroutine {
            handle,
            stack_size: Stack::default_size(),
            leak_on_panic: false,
        }
    }

    /// Configures the stack size used for coroutines.
    ///
    /// Coroutines spawned from this builder will get stack of this size. The default is something
    /// small, so if you use recursion, you might want to use this.
    ///
    /// Note that the size must be a valid stack size. This is platform dependente, but usually
    /// must be multiple of a page size. That usually means a multiple of 4096.
    ///
    /// If an invalid size is set, attemts to spawn coroutines will fail with an error.
    ///
    /// # Parameters
    ///
    /// * `size`: The stack size to use for newly spawned coroutines.
    pub fn stack_size(&mut self, size: usize) -> &mut Self {
        self.stack_size = size;
        self
    }

    /// Leak coroutines instead of double-panicking.
    ///
    /// A coroutine whose core has been dropped is usually cleaned up by panicking inside the
    /// coroutine. However, if the core has been dropped because the main coroutine panicked,
    /// this'd lead to a double-panic, terminating the application.
    ///
    /// This allows *not* cleaning the coroutines in such case. In such situation, the stack itself
    /// is released, but the destructors of objects on the stack are not, possibly leaking
    /// resources.
    ///
    /// Other options to handling this is to use the `cleanup` methods for waiting (these return
    /// result and it is up to the code to handle the cleanup manually), or using the
    /// [`Coroutine::main`](#method.main) to start all the coroutines.
    ///
    /// Note that if a coroutine is leaked, its result future currently never resolves. This may
    /// change in the future (then it would return `TaskFailed::Lost`).
    pub fn leak_on_panic(&mut self, leak: bool) -> &mut Self {
        self.leak_on_panic = leak;
        self
    }

    /// Spawns a coroutine directly.
    ///
    /// This constructor spawns a coroutine with default parameters without the inconvenience of
    /// handling a builder. It is equivalent to spawning it with an unconfigured builder.
    ///
    /// Unlike the [`spawn`](#method.spawn.html), this one can't fail, since the default parameters
    /// of the builder are expected to always work (if they don't, file a bug).
    ///
    /// # Parameters
    ///
    /// * `handle`: Handle to the reactor the coroutine will use to suspend its execution and wait
    ///   for events.
    /// * `task`: The closure to run inside the coroutine.
    ///
    /// # Returns
    ///
    /// A future that'll resolve once the coroutine completes, with the result of the `task`, or
    /// with an error explaining why the coroutine failed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate corona;
    /// # extern crate tokio_core;
    /// use corona::Coroutine;
    /// use tokio_core::reactor::Core;
    ///
    /// # fn main() {
    /// let mut core = Core::new().unwrap();
    ///
    /// let coroutine = Coroutine::with_defaults(core.handle(), || { });
    ///
    /// core.run(coroutine).unwrap();
    /// # }
    ///
    /// ```
    pub fn with_defaults<R, Task>(handle: Handle, task: Task) -> CoroutineResult<R>
    where
        R: 'static,
        Task: FnOnce() -> R + 'static,
    {
        Coroutine::new(handle).spawn(task).unwrap()
    }

    /// The inner workings of `spawn` and `spawn_catch_panic`.
    fn spawn_inner<R, Task>(&self, task: Task, propagate_panic: bool)
        -> Result<CoroutineResult<R>, StackError>
    where
        R: 'static,
        Task: FnOnce() -> R + UnwindSafe + 'static,
    {
        let (sender, receiver) = oneshot::channel();

        let handle = self.handle.clone();
        let leak_on_panic = self.leak_on_panic;

        let perform = move |context, stack| {
            let my_context = CoroutineContext {
                handle,
                parent_context: context,
                stack,
                leak_on_panic,
            };
            CONTEXTS.with(|c| c.borrow_mut().push(my_context));
            let mut panic_result = None;
            let result = match panic::catch_unwind(AssertUnwindSafe(task)) {
                Ok(res) => TaskResult::Finished(res),
                Err(panic) => {
                    if panic.is::<Dropped>() {
                        TaskResult::Lost
                    } else if propagate_panic {
                        panic_result = Some(panic);
                        TaskResult::PanicPropagated
                    } else {
                        TaskResult::Panicked(panic)
                    }
                },
            };
            // We are not interested in errors. They just mean the receiver is no longer
            // interested, which is fine by us.
            drop(sender.send(result));
            let my_context = CONTEXTS.with(|c| c.borrow_mut().pop().unwrap());
            (my_context.parent_context, my_context.stack, panic_result)
        };
        Switch::run_new_coroutine(self.stack_size, Box::new(Some(perform)))?;

        Ok(CoroutineResult { receiver })
    }

    /// Spawns a coroutine with configuration from the builder.
    ///
    /// This spawns a new coroutine with the values previously set in the builder.
    ///
    /// # Parameters
    ///
    /// * `task`: The closure to run inside the coroutine.
    ///
    /// # Returns
    ///
    /// A future that'll resolve once the coroutine terminates and will yield the result of
    /// `task`, or an error explaining why the coroutine failed.
    ///
    /// This returns a `StackError` if the configured stack size is invalid.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate corona;
    /// # extern crate tokio_core;
    /// use corona::Coroutine;
    /// use tokio_core::reactor::Core;
    ///
    /// # fn main() {
    /// let mut core = Core::new().unwrap();
    ///
    /// let coroutine = Coroutine::new(core.handle())
    ///     .stack_size(40_960)
    ///     .spawn(|| { }).unwrap();
    ///
    /// core.run(coroutine).unwrap();
    /// # }
    /// ```
    ///
    /// # Panic handling
    ///
    /// If the coroutine panics, the panic is propagated. This usually means the `core.run`, unless
    /// the panic happens before the first suspension point, in which case it is the `spawn` itself
    /// which panics.
    pub fn spawn<R, Task>(&self, task: Task) -> Result<CoroutineResult<R>, StackError>
    where
        R: 'static,
        Task: FnOnce() -> R + 'static,
    {
        // That AssertUnwindSafe is OK. We just pause the panic, teleport it to the callers thread
        // and then let it continue.
        self.spawn_inner(AssertUnwindSafe(task), true)
    }

    /// Spawns a coroutine, preventing the panics in it from killing the parent task.
    ///
    /// This is just like [spawn](#method.spawn), but any panic in the coroutine is captured and
    /// returned through the result instead of propagating.
    ///
    /// Note that you need to ensure the `task` is [unwind
    /// safe](https://doc.rust-lang.org/std/panic/trait.UnwindSafe.html) for that reason.
    pub fn spawn_catch_panic<R, Task>(&self, task: Task) -> Result<CoroutineResult<R>, StackError>
    where
        R: 'static,
        Task: FnOnce() -> R + UnwindSafe + 'static,
    {
        self.spawn_inner(task, false)
    }

    /// Waits for completion of a future.
    ///
    /// This suspends the execution of the current coroutine until the provided future is
    /// completed, possibly switching to other coroutines in the meantime.
    ///
    /// This is the low-level implementation of the waiting. It is expected user code will use the
    /// interface in [`prelude`](../prelude/index.html) instead.
    ///
    /// # Parameters
    ///
    /// * `fut`: The future to wait on.
    ///
    /// # Returns
    ///
    /// * `Ok(result)` with the result the future resolved to.
    /// * `Err(Dropped)` when the reactor was dropped before the future had a chance to resolve.
    ///
    /// # Panics
    ///
    /// If called outside of a coroutine (there's nothing to suspend).
    ///
    /// Also, panics from withit the provided future are propagated into the calling coroutine.
    pub fn wait<I, E, Fut>(mut fut: Fut) -> Result<Result<I, E>, Dropped>
    where
        Fut: Future<Item = I, Error = E>,
    {
        // Grimoire marginalia (eg. a sidenote on the magic here).
        //
        // This is probably the hearth of the library both in the importance and complexity to
        // understand. We want to wait for the future to finish.
        //
        // To do that we do the following:
        // • Prepare a space for the result on our own stack.
        // • Prepare a wrapper future that'll do some bookkeeping around the user's future ‒ for
        //   example makes sure the wrapper future has the same signature and can be spawned onto
        //   the reactor.
        // • Switch to our parent context with the instruction to install the future for us into
        //   the reactor.
        //
        // Some time later, as the reactor runs, the future resolves. It'll do the following:
        // • Store the result into the prepared space on our stack.
        // • Switch the context back to us.
        // • This function resumes, picks ups the result from its stack and returns it.
        //
        // There are few unsafe blocks here, some of them looking a bit dangerous. So, some
        // rationale why this should be in fact safe.
        //
        // The handle.spawn() requires a 'static future. It is because the future will almost
        // certainly live longer than the stack frame that spawned it onto the reactor. Therefore,
        // the future must own anything it'll touch in some unknown later time.
        //
        // However, this is true in our case. The closure that runs in a coroutine is required to
        // be 'static. Therefore, anything non-'static must live on the coroutine's stack. And the
        // future has the only pointer to the stack of this coroutine, therefore effectively owns
        // the stack and everything on it.
        //
        // In other words, the stack is there for as long as the future waits idle in the reactor
        // and won't go away before the future either resolves or is dropped. There's a small trick
        // in the `drop` implementation and the future itself to ensure this is true even when
        // switching the contexts (it is true when we switch to this coroutine, but not after we
        // leave it, so the future's implementation must not touch the things afterwards.
        let my_context = CONTEXTS.with(|c| {
            c.borrow_mut().pop().expect("Can't wait outside of a coroutine")
        });
        let mut result: Option<Result<I, E>> = None;
        let (reply_instruction, context) = {
            let res_ref = &mut result as *mut _ as usize;
            let mut poll = move || {
                let res = match fut.poll() {
                    Ok(Async::NotReady) => return Ok(Async::NotReady),
                    Ok(Async::Ready(ok)) => Ok(ok),
                    Err(err) => Err(err),
                };
                let result = res_ref as *mut Option<Result<I, E>>;
                // Inject the result inside the place on the stack.
                unsafe { *result = Some(res) };
                Ok(Async::Ready(()))
            };
            let p: &mut FnMut() -> Poll<(), ()> = &mut poll;
            let handle = my_context.handle.clone();
            let mut task = WaitTask {
                poll: Some(unsafe { mem::transmute::<_, &'static mut _>(p) }),
                context: None,
                handle,
                leak_on_panic: my_context.leak_on_panic,
                stack: Some(my_context.stack),
            };
            let instruction = Switch::WaitFuture { task };
            instruction.exchange(my_context.parent_context)
        };
        let (result, stack) = match reply_instruction {
            Switch::Resume { stack } => (Ok(Ok(result.unwrap())), stack),
            Switch::Cleanup { stack } => (Ok(Err(Dropped)), stack),
            Switch::PropagateFuturePanic { stack, panic } => (Err(panic), stack),
            _ => unreachable!("Invalid instruction on wakeup"),
        };
        // Reconstruct our context anew after we switched back.
        let new_context = CoroutineContext {
            parent_context: context,
            stack: stack,
            handle: my_context.handle,
            leak_on_panic: my_context.leak_on_panic,
        };
        CONTEXTS.with(|c| c.borrow_mut().push(new_context));
        match result {
            Ok(result) => result,
            Err(panic) => panic::resume_unwind(panic),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::rc::Rc;
    use std::time::Duration;

    use futures::future;
    use tokio_core::reactor::{Core, Timeout};

    use super::*;

    /// Test spawning and execution of tasks.
    #[test]
    fn spawn_some() {
        let mut core = Core::new().unwrap();
        let s1 = Rc::new(AtomicBool::new(false));
        let s2 = Rc::new(AtomicBool::new(false));
        let s1c = s1.clone();
        let s2c = s2.clone();
        let handle = core.handle();

        let mut builder = Coroutine::new(handle);
        builder.stack_size(40960);
        let builder_inner = builder.clone();

        let result = builder.spawn(move || {
                let result = builder_inner.spawn(move || {
                        s2c.store(true, Ordering::Relaxed);
                        42
                    })
                    .unwrap();
                s1c.store(true, Ordering::Relaxed);
                result
            })
            .unwrap();

        // Both coroutines run to finish
        assert!(s1.load(Ordering::Relaxed), "The outer closure didn't run");
        assert!(s2.load(Ordering::Relaxed), "The inner closure didn't run");
        // The result gets propagated through.
        let extract = result.and_then(|r| r);
        assert_eq!(42, core.run(extract).unwrap());
    }

    /// Wait for a future to complete.
    #[test]
    fn future_wait() {
        let mut core = Core::new().unwrap();
        let handle = core.handle();
        let (sender, receiver) = oneshot::channel();
        let all_done = Coroutine::with_defaults(core.handle(), move || {
            let msg = Coroutine::wait(receiver).unwrap().unwrap();
            msg
        });
        Coroutine::with_defaults(core.handle(), move || {
            let timeout = Timeout::new(Duration::from_millis(50), &handle).unwrap();
            Coroutine::wait(timeout).unwrap().unwrap();
            sender.send(42).unwrap();
        });
        assert_eq!(42, core.run(all_done).unwrap());
    }

    /// The panic doesn't kill the main thread, but is reported.
    #[test]
    fn panics_catch() {
        let mut core = Core::new().unwrap();
        let handle = core.handle();
        match core.run(Coroutine::new(handle).spawn_catch_panic(|| panic!("Test")).unwrap()) {
            Err(TaskFailed::Panicked(_)) => (),
            _ => panic!("Panic not reported properly"),
        }
        let handle = core.handle();
        assert_eq!(42, core.run(Coroutine::with_defaults(handle, || 42)).unwrap());
    }

    /// However, normal coroutines do panic.
    #[test]
    #[should_panic]
    fn panics_spawn() {
        let core = Core::new().unwrap();
        let _ = Coroutine::new(core.handle()).spawn(|| panic!("Test"));
    }

    /// This one panics and the panic is propagated, but after suspension point it is out of run.
    #[test]
    fn panics_run() {
        let mut core = Core::new().unwrap();
        let coroutine = Coroutine::with_defaults(core.handle(), || {
                let _ = Coroutine::wait(future::ok::<(), ()>(()));
                panic!("Test");
            });
        panic::catch_unwind(AssertUnwindSafe(|| core.run(coroutine))).unwrap_err();
    }

    /// It's impossible to wait on a future outside of a coroutine
    #[test]
    #[should_panic]
    fn panic_without_coroutine() {
        drop(Coroutine::wait(future::ok::<_, ()>(42)));
    }

    /// Tests leaking instead of double-panicking. This is tested simply by the tests not crashing
    /// hard.
    #[test]
    fn panic_leak() {
        let core = Core::new().unwrap();
        let _coroutine = Coroutine::new(core.handle())
            .leak_on_panic(true)
            .spawn(|| {
                let _ = Coroutine::wait(future::empty::<(), ()>());
                panic!("Should never get here!");
            })
            .unwrap();
        panic::catch_unwind(AssertUnwindSafe(move || {
                let _core = core;
                panic!("Test");
            }))
            .unwrap_err();
        /*
         * FIXME: This doesn't work as intended. The sender gets leaked too, so it is never closed
         * and we don't get the Lost case. Any way to make sure we get it?
        if let Err(TaskFailed::Lost) = coroutine.wait() {
            // OK, correct error
        } else {
            panic!("Coroutine didn't get lost correctly");
        }
        */
    }

    /// A panic from inside the future doesn't kill the core, but falls out of the wait into the
    /// responsible coroutine.
    ///
    /// We test this separately because the future is „exported“ to the main coroutine to be
    /// polled. So we check it doesn't kill the core.
    #[test]
    fn panic_in_future() {
        let mut core = Core::new().unwrap();
        let coroutine = Coroutine::with_defaults(core.handle(), || {
            struct PanicFuture;
            impl Future for PanicFuture {
                type Item = ();
                type Error = ();
                fn poll(&mut self) -> Poll<(), ()> {
                    panic!("Test");
                }
            }

            if let Ok(_) = panic::catch_unwind(|| Coroutine::wait(PanicFuture)) {
                panic!("A panic should fall out of wait");
            }
        });
        core.run(coroutine).unwrap();
    }
}
