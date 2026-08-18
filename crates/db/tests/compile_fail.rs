/// The typed-token guarantee is only real if a regression cannot silently undo
/// it. These cases must fail to compile; if one starts compiling, someone has
/// re-exposed the pool or loosened a signature.
#[test]
fn unscoped_and_mistyped_queries_do_not_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/no_authctx.rs");
    t.compile_fail("tests/ui/wrong_domain.rs");
    t.compile_fail("tests/ui/wrong_action.rs");
}
