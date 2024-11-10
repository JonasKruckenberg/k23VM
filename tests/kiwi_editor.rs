use wasmparser::Validator;
use k23_vm::{Engine, Module};

#[test_log::test]
pub fn main() {
    let engine = Engine::default();
    let mut validator = Validator::new();

    let _module = Module::from_wat(
        &engine,
        &mut validator,
        include_str!("./kiwi-editor.wat"),
    )
    .unwrap();
}