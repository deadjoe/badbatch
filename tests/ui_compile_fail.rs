//! Compile-fail UI tests for residual soundness surface gates.
//!
//! These complement the `compile_fail` doctests on `SimpleProducer` and
//! `SingleProducerMode` with an explicit trybuild suite.

#[test]
fn ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
