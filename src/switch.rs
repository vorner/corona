//! Module for the low-level switching of coroutines

use std::cell::RefCell;

use context::{Context, Transfer};
use context::stack::ProtectedFixedSizeStack;
use futures::Future;
use tokio_core::reactor::Handle;

use stack_cache;

/// A workaround befause Box<FnOnce> is currently very unusable in rust :-(.
pub trait BoxableTask {
    fn perform(&mut self, Context) -> Context;
}

impl<F: FnOnce(Context) -> Context> BoxableTask for Option<F> {
    fn perform(&mut self, context: Context) -> Context {
        self.take().unwrap()(context)
    }
}

/// A fake Box<FnOnce(Context) -> Context>.
type BoxedTask = Box<BoxableTask>;

/// Execution of a coroutine.
///
/// This holds the extracted logic, so once we leave the coroutine, all locals that may possibly
/// have any kind of destructor are gone.
fn coroutine_internal(transfer: Transfer) -> Context {
    let mut context = transfer.context;
    let switch = Switch::get();
    let result = match switch {
        Switch::StartTask { stack, mut task } => {
            context = task.perform(context);
            Switch::Destroy { stack }
        },
        _ => panic!("Invalid switch instruction on coroutine entry"),
    };
    result.put();
    context
}

/// Wrapper for the execution of a coroutine.
///
/// This is just a minimal wrapper that runs the `coroutine_internal` and then switches back to the
/// parent context. This contains very minimal amount of local variables and only the ones from the
/// `context` crate, so we don't have anything with destructor here. The problem is, this function
/// never finishes and therefore such destructors wouldn't be run.
extern "C" fn coroutine(transfer: Transfer) -> ! {
    let context = coroutine_internal(transfer);
    unsafe { context.resume(0) };
    unreachable!("Woken up after termination!");
}

// TODO: We could actually pass this through the data field of the transfer
/// An instruction carried across the coroutine boundary.
///
/// This describes what the receiving coroutine should do next (and contains parameters for that).
/// It also holds the methods to do the actual switching.
///
/// Note that not all instructions are valid at all contexts, but as this is not an API visible
/// outside of the crate, that's likely OK with checking not thing invalid gets received.
pub enum Switch {
    /// Start a new task in the coroutine.
    StartTask {
        stack: ProtectedFixedSizeStack,
        task: BoxedTask,
    },
    /// Please wake the sending coroutine once the `after` future resolves.
    ScheduleWakeup {
        after: Box<Future<Item = (), Error = ()>>,
        handle: Handle,
    },
    /// Continue operation, the future is resolved.
    Resume,
    /// Get rid of the sending coroutine, it terminated.
    Destroy {
        stack: ProtectedFixedSizeStack,
    },
}

thread_local! {
    static SWITCH: RefCell<Option<Switch>> = RefCell::new(None);
}

impl Switch {
    /// Stores the switch instruction in a thread-local variable, so it can be taken out in another
    /// coroutine.
    fn put(self) {
        SWITCH.with(|s| {
            let mut s = s.borrow_mut();
            assert!(s.is_none(), "Leftover switch instruction");
            *s = Some(self);
        });
    }
    /// Extracts the instruction stored in a thread-local variable.
    fn get() -> Self {
        SWITCH.with(|s| s.borrow_mut().take().expect("Missing switch instruction"))
    }
    /// Switches to a coroutine and back.
    ///
    /// Switches to the given context (coroutine) and sends it the current instruction. Returns the
    /// context that resumed us (after we are resumed) and provides the instruction it send us.
    pub fn exchange(self, context: Context) -> (Self, Context) {
        self.put();
        let transfer = unsafe { context.resume(0) };
        let switch = Self::get();
        (switch, transfer.context)
    }
    /// Runs a child coroutine (one that does the work, is not a control coroutine) and once it
    /// returns, handles its return instruction.
    fn run_child(self, context: Context) {
        let (reply, context) = self.exchange(context);
        use Switch::*;
        match reply {
            Destroy { stack } => {
                drop(context);
                stack_cache::put(stack);
            },
            ScheduleWakeup { after, handle } => {
                // TODO: We may want some kind of our own future here and implement Drop, so we can
                // unwind the stack and destroy it.
                let wakeup = after.then(move |_| {
                    Resume.run_child(context);
                    Ok(())
                });
                handle.spawn(wakeup);
            },
            _ => unreachable!("Invalid switch instruction when switching out"),
        }
    }
    /// Creates a new coroutine and runs it.
    pub fn run_new_coroutine(stack_size: usize, task: BoxedTask) {
        let stack = stack_cache::get(stack_size);
        assert_eq!(stack.len(), stack_size);
        let context = unsafe { Context::new(&stack, coroutine) };
        Switch::StartTask { stack, task }.run_child(context);
    }
}

