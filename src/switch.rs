use std::cell::RefCell;

use context::Transfer;
use context::stack::ProtectedFixedSizeStack;
use futures::Future;
use tokio_core::reactor::Handle;

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
}

