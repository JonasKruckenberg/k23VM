use k23_vm::{ConstExprEvaluator, Engine, Linker, Module, PlaceholderAllocatorDontUse, Store};
use wasmparser::Validator;

#[test_log::test]
fn main() {
    let engine = Engine::default();
    let mut validator = Validator::new();
    let mut linker = Linker::new(&engine);
    let mut store = Store::new(&engine);
    let mut const_eval = ConstExprEvaluator::default();

    // instantiate & define the fib_cpp module
    {
        let module = Module::from_wat(
            &engine,
            &mut validator,
            include_str!("./fib_cpp.wat"),
        )
        .unwrap();

        let instance = linker
            .instantiate(
                &mut store,
                &PlaceholderAllocatorDontUse,
                &mut const_eval,
                &module,
            )
            .unwrap();
        instance.debug_vmctx(&store);

        linker
            .define_instance(&mut store, "fib_cpp", instance)
            .unwrap();
    }

    // instantiate the test module
    {
        let module = Module::from_wat(
            &engine,
            &mut validator,
            include_str!("./fib_test.wat"),
        )
        .unwrap();

        let instance = linker
            .instantiate(
                &mut store,
                &PlaceholderAllocatorDontUse,
                &mut const_eval,
                &module,
            )
            .unwrap();

        instance.debug_vmctx(&store);

        let func = instance.get_func(&mut store, "fib_test").unwrap();
        func.call(&mut store, &[], &mut []).unwrap();
    }
}
