use corona::prelude::*;

#[test]
fn bug_4_weird_stack_size_assert() {
    Coroutine::new()
        .stack_size(10_000)
        .run(|| {})
        .expect("failed")
}
