use std::cell::RefCell;

use context::{Context, Transfer};
use context::stack::ProtectedFixedSizeStack;
use futures::Future;
use tokio_core::reactor::Handle;

use stack_cache;

// A workaround befause Box<FnOnce> is currently very unusable in rust :-(.
pub trait BoxableTask {
    fn perform(&mut self, Transfer) -> Transfer;
}

impl<F: FnOnce(Transfer) -> Transfer> BoxableTask for Option<F> {
    fn perform(&mut self, transfer: Transfer) -> Transfer {
        self.take().unwrap()(transfer)
    }
}

type BoxedTask = Box<BoxableTask>;

// TODO: We could actually pass this through the data field of the transfer
pub enum Switch {
    StartTask {
        stack: ProtectedFixedSizeStack,
        task: BoxedTask,
    },
    ScheduleWakeup {
        after: Box<Future<Item = (), Error = ()>>,
        handle: Handle,
    },
    Resume,
    Destroy {
        stack: ProtectedFixedSizeStack,
    },
}

thread_local! {
    static SWITCH: RefCell<Option<Switch>> = RefCell::new(None);
}

impl Switch {
    pub fn put(self) {
        SWITCH.with(|s| {
            let mut s = s.borrow_mut();
            assert!(s.is_none(), "Leftover switch instruction");
            *s = Some(self);
        });
    }
    pub fn get() -> Self {
        SWITCH.with(|s| s.borrow_mut().take().expect("Missing switch instruction"))
    }
    pub fn run_child(self, context: Context) {
        use Switch::*;
        self.put();
        let transfer = unsafe { context.resume(0) };
        let switch = Self::get();
        match switch {
            Destroy { stack } => {
                drop(transfer.context);
                stack_cache::put(stack);
            },
            ScheduleWakeup { after, handle } => {
                // TODO: We may want some kind of our own future here and implement Drop, so we can
                // unwind the stack and destroy it.
                let wakeup = after.then(move |_| {
                    Resume.run_child(transfer.context);
                    Ok(())
                });
                handle.spawn(wakeup);
            },
            _ => unreachable!("Invalid switch instruction when switching out"),
        }
    }
}

