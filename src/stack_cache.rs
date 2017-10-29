use std::cell::RefCell;
use std::collections::HashMap;

use context::stack::ProtectedFixedSizeStack;

use errors::StackError;

thread_local! {
    static CACHE: RefCell<HashMap<usize, Vec<ProtectedFixedSizeStack>>> =
        RefCell::new(HashMap::new());
}

/// Get a stack of the given size.
///
/// Retrieve it from the cache or create a new one, if none is available.
///
/// The cache is thread local.
pub(crate) fn get(size: usize) -> Result<ProtectedFixedSizeStack, StackError> {
    CACHE.with(|c| {
        let mut cell = c.borrow_mut();
        cell.get_mut(&size)
            .and_then(|v| v.pop().map(Ok))
            .unwrap_or_else(|| {
                ProtectedFixedSizeStack::new(size)
            })
    })
}

/// Put a stack into the cache, for future reuse.
///
/// The cache is thread local and the stack will be returned in some future
/// [`get`](function.get.html) call.
pub(crate) fn put(stack: ProtectedFixedSizeStack) {
    let len = stack.len();
    CACHE.with(|c| c.borrow_mut().entry(len).or_insert_with(Vec::new).push(stack));
}
