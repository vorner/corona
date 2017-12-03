# 0.3.0

Redesign of the API. The async context is implicit in thread local storage. The
interface is easier to work with and looks cleaner, at the cost of checking some
misuses at runtime.

Old code can be adapted mostly by removing the parameter of the closure passed
to `Coroutine::new()`.
