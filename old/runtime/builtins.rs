use crate::builtins::BuiltinFunctionIndex;

macro_rules! define_builtin_array {
    (
        $(
            $( #[$attr:meta] )*
            $name:ident( $( $pname:ident: $param:ident ),* ) $( -> $result:ident )?;
        )*
    ) => {
        /// An array that stores addresses of builtin functions. We translate code
        /// to use indirect calls. This way, we don't have to patch the code.
        #[repr(C)]
        pub struct VMBuiltinFunctionsArray {
            $(
                $name: unsafe extern "C" fn(
                    $(define_builtin_array!(@ty $param)),*
                ) $( -> define_builtin_array!(@ty $result))?,
            )*
        }

        impl VMBuiltinFunctionsArray {
            #[allow(unused_doc_comments)]
            pub const INIT: VMBuiltinFunctionsArray = VMBuiltinFunctionsArray {
                $(
                    $name: crate::vm::builtins::$name,
                )*
            };
        }
    };

    (@ty i32) => (u32);
    (@ty i64) => (u64);
    (@ty u8) => (u8);
    (@ty reference) => (u32);
    (@ty pointer) => (*mut u8);
    (@ty vmctx) => (*mut VMContext);
}

crate::foreach_builtin_function!(define_builtin_array);

const _: () = {
    assert!(
        size_of::<VMBuiltinFunctionsArray>()
            == size_of::<usize>()
                * (BuiltinFunctionIndex::builtin_functions_total_number() as usize)
    )
};
