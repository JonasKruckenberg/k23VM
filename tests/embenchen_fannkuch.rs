use k23_vm::{Engine, Module};
use wasmparser::Validator;

#[test_log::test]
fn main() {
    let engine = Engine::default();
    let mut validator = Validator::new();

    let _module = Module::from_wat(
        &engine,
        &mut validator,
        include_str!("./embenchen_fannkuch.wat"),
    )
    .unwrap();
}
