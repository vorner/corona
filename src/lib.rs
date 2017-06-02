extern crate context;
extern crate futures;
extern crate tokio_core;
extern crate typed_arena;

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::rc::{Rc, Weak};

use context::{Context, Transfer};
use context::stack::ProtectedFixedSizeStack;
use futures::{Async, Poll};
use futures::future::Future;
use futures::unsync::oneshot::{self, Receiver};
use tokio_core::reactor::Handle as TokioHandle;
use typed_arena::Arena;

enum TaskResult<R> {
    /// The task has been aborted, possibly because the scheduler got destroyed.
    Aborted,
    /// The task panicked.
    Panicked(Box<Any + Send + 'static>),
    /// Succeeded.
    Ok(R),
}

// TODO: Implement Error for this thing.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TaskFailed {
    /// The task has been aborted, possibly because the scheduler got destroyed.
    Aborted,
    /// Lost.
    ///
    /// TODO: Can this actually happen?
    Lost,
}

pub struct Spawned<R>(Receiver<TaskResult<R>>);

impl<R> Future for Spawned<R> {
    type Item = R;
    type Error = TaskFailed;
    fn poll(&mut self) -> Poll<R, TaskFailed> {
        match self.0.poll() {
            Ok(Async::Ready(TaskResult::Aborted)) => Err(TaskFailed::Aborted),
            Ok(Async::Ready(TaskResult::Panicked(reason))) => panic::resume_unwind(reason),
            Ok(Async::Ready(TaskResult::Ok(result))) => Ok(Async::Ready(result)),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(_) => Err(TaskFailed::Lost),
        }
    }
}

// Make sure this is *not* Copy or Clone. That ensures the user is not able to await outside of the
// child context.
pub struct Await<'a> {
    transfer: &'a mut Transfer,
    handle: &'a Handle,
}

thread_local! {
    /// The `coroutine_function` must not be a closure. This is used to pass the scheduler into it.
    ///
    /// This should be None at all other times except when passing it through.
    static HANDLE: RefCell<Option<Handle>> = RefCell::new(None);
}

#[derive(Eq, PartialEq)]
enum CoroutineInstruction {
    /// The coroutine should pick up a new task and work on it.
    Work,
    /// The coroutine should terminate.
    Terminate,
}

#[derive(Eq, PartialEq)]
enum CoroutineStatus {
    /// Ready to do more work.
    Ready,
    /// Please, deallocate me.
    Terminated,
}

extern "C" fn coroutine_function(mut transfer: Transfer) -> ! {
    {
        let handle = HANDLE.with(|h| h.borrow_mut().take().unwrap());
        while transfer.data == CoroutineInstruction::Work as usize {
            {
                let mut task = handle.into_scheduler().get_task();
                transfer = task.perform(transfer);
            }
            // Ask for more work and go to sleep
            transfer = transfer.context.resume(CoroutineStatus::Ready as usize);
        }
    }
    assert_eq!(transfer.data, CoroutineInstruction::Terminate as usize);
    // Signal that we are ready to be destroyed
    transfer.context.resume(CoroutineStatus::Terminated as usize);
    unreachable!("A context woken up after termination");
}

/// A hack to work around the fact that Box<FnOnce()> doesn't really work.
trait BoxableTask {
    fn perform(&mut self, transfer: Transfer) -> Transfer;
}

impl<F: FnOnce(Transfer) -> Transfer> BoxableTask for Option<F> {
    fn perform(&mut self, transfer: Transfer) -> Transfer {
        self.take().unwrap()(transfer)
    }
}

type Task = Box<BoxableTask>;

struct Internal {
    /// The tokio used to run futures.
    handle: TokioHandle,
    /// Are we currently in the main context?
    main_context: Cell<bool>,
    /// List of unstarted tasks to perform in coroutines.
    unstarted: RefCell<VecDeque<Task>>,
    /// Just a space for allocation of the coroutines.
    stacks: Arena<ProtectedFixedSizeStack>,
    /// Contexts not doing anything right now.
    ///
    /// These contexts finished their assigned work and are ready to be reused.
    retired: RefCell<Vec<Context>>,
}

impl Drop for Internal {
    fn drop(&mut self) {
        // TODO clean up (unwind) active contexts?
    }
}

pub struct Scheduler(Rc<Internal>);

impl Scheduler {
    pub fn new(handle: TokioHandle) -> Self {
        let internal = Internal {
            handle,
            main_context: Cell::new(true),
            unstarted: RefCell::new(VecDeque::new()),
            stacks: Arena::new(),
            retired: RefCell::new(Vec::new()),
        };
        Scheduler(Rc::new(internal))
    }
    pub fn spawn<R: 'static, F: FnOnce(&Await) -> R + 'static>(&self, f: F) -> Spawned<R> {
        let (sender, receiver) = oneshot::channel();
        let handle = self.handle();
        let task = move |mut transfer| {
            {
                let await = Await {
                    transfer: &mut transfer,
                    handle: &handle,
                };
                // TODO: Think about that AssertUnwindSafe.
                let result = match panic::catch_unwind(AssertUnwindSafe(|| f(&await))) {
                    Ok(res) => TaskResult::Ok(res),
                    Err(panic) => TaskResult::Panicked(panic),
                };
                // We don't care if the receiver doesn't listen → ignore errors here
                drop(sender.send(result));
            }
            transfer
        };
        self.0.unstarted.borrow_mut().push_back(Box::new(Some(task)));
        self.try_running();
        Spawned(receiver)
    }
    pub fn handle(&self) -> Handle {
        Handle(Rc::downgrade(&self.0))
    }
    fn try_running(&self) {
        if !self.0.main_context.get() {
            // Run just one coroutine at a time. Start another once we return.
            return;
        }
        while !self.0.unstarted.borrow().is_empty() {
            // We have an unstarted task, start a new coroutine that'll eat it.
            let context = self.get_context();
            self.run_context(context, CoroutineInstruction::Work);
        }
    }
    /// Get a context.
    ///
    /// Either allocate it or get a retired one.
    fn get_context(&self) -> Context {
        self.0.retired
            .borrow_mut()
            .pop()
            .unwrap_or_else(|| {
                let stack = self.0.stacks.alloc(ProtectedFixedSizeStack::default());
                Context::new(stack, coroutine_function)
            })
    }
    fn run_context(&self, context: Context, instruction: CoroutineInstruction) {
        // As the context function can't be a closure, we need to pass ourselves through the
        // thread-local variable
        HANDLE.with(|h| *h.borrow_mut() = Some(self.handle()));
        self.0.main_context.set(false);
        let transfer = context.resume(instruction as usize);
        self.0.main_context.set(true);
        if transfer.data == CoroutineStatus::Ready as usize {
            self.0.retired.borrow_mut().push(transfer.context);
        } else if transfer.data == CoroutineStatus::Terminated as usize {
            drop(transfer.context);
        } else {
            unreachable!("Context returned invalid state {}", transfer.data);
        }
    }
    /// Get an unstarted task.
    ///
    /// # Panics
    ///
    /// If there are no tasks waiting to be started.
    fn get_task(&self) -> Task {
        self.0.unstarted.borrow_mut().pop_front().unwrap()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SchedulerDropped;

#[derive(Clone)]
pub struct Handle(Weak<Internal>);

impl Handle {
    pub fn spawn<R, F>(&self, f: F) -> Result<Spawned<R>, SchedulerDropped>
        where
            R: 'static,
            F: FnOnce(&Await) -> R + 'static,
    {
        self.0
            .upgrade()
            .map(|internal| Scheduler(internal).spawn(f))
            .ok_or(SchedulerDropped)
    }
    /// Provide the scheduler at the end of the handle.
    ///
    /// # Panics
    ///
    /// This assumes the scheduler is still alive. This is the case in all cases when we call this
    /// in the child context, because the main context holds at least one strong reference.
    fn into_scheduler(&self) -> Scheduler {
        Scheduler(self.0.upgrade().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use tokio_core::reactor::Core;

    use super::*;

    /// Test spawning and execution of tasks.
    #[test]
    fn spawn_some() {
        let mut core = Core::new().unwrap();
        let scheduler = Scheduler::new(core.handle());
        let s1 = Rc::new(Cell::new(false));
        let s2 = Rc::new(Cell::new(false));
        let s1c = s1.clone();
        let s2c = s2.clone();
        let handle = scheduler.handle();

        let result = scheduler.spawn(move |_| {
            let result = handle.spawn(move |_| {
                    s2c.set(true);
                    42
                })
                .unwrap();
            s1c.set(true);
            result
        });
        // Both coroutines run
        assert!(s1.get(), "The outer closure didn't run");
        assert!(s2.get(), "The inner closure didn't run");
        // The result gets propagated through.
        let extract = result.and_then(|r| r);
        assert_eq!(42, core.run(extract).unwrap());
    }

    /// The panic doesn't kill the scheduler.
    #[test]
    fn panic_unobserved() {
        let mut core = Core::new().unwrap();
        let scheduler = Scheduler::new(core.handle());
        scheduler.spawn(|_| panic!("Test!"));
        // The second coroutine will work and the panic gets lost ‒ similar to threads.
        assert_eq!(42, core.run(scheduler.spawn(|_| 42)).unwrap());
    }

    /// If we try to resolve the `Spawned`, we get the panic.
    #[test]
    #[should_panic]
    fn panic_observed() {
        let mut core = Core::new().unwrap();
        let scheduler = Scheduler::new(core.handle());
        drop(core.run(scheduler.spawn(|_| panic!())));
    }

    /// Spawning on a handle fails if the scheduler already died.
    #[test]
    fn dead_handle() {
        let handle = Scheduler::new(Core::new().unwrap().handle()).handle();
        assert!(handle.spawn(|_| true).is_err());
    }
}
