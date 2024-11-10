use wasmparser::Validator;
use k23_vm::{Engine, Module};

#[test_log::test]
fn main() {
    let str = r"
    (module
        (type $ii (func (param i32) (result i32)))
        
        (func $apply (param $f (ref null $ii)) (param $x i32) (result i32)
            (call_ref $ii (local.get $x) (local.get $f))
        )
    )";

    let engine = Engine::default();
    let mut validator = Validator::new();

    let _module = Module::from_wat(&engine, &mut validator, str).unwrap();
}