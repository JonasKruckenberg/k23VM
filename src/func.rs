use crate::instance::ExportFunction;
use crate::store::Store;
use crate::vmcontext::VMVal;
use core::ptr;
use std::process;
use std::ptr::NonNull;
use tracing::log;
use wasmparser::FuncType;

pub struct Func {
    inner: ExportFunction,
    ty: FuncType,
}

impl Func {
    pub unsafe fn from_raw(inner: ExportFunction, ty: FuncType) -> Self {
        Self { inner, ty }
    }

    pub fn call(&self, store: &mut Store<'static>, params: &[VMVal], results: &mut [VMVal]) {
        // TODO check signature matches provided params and results capacity
        unsafe { self.call_unchecked(store, params, results) }
    }

    pub async fn call_async(&self, store: &mut Store<'_>, params: &[VMVal], results: &mut [VMVal]) {
        // TODO check signature matches provided params and results capacity
        // TODO on separate stack (Fiber) do self.call_unchecked()
    }

    unsafe fn call_unchecked(
        &self,
        store: &mut Store<'static>,
        params: &[VMVal],
        results: &mut [VMVal],
    ) {
        let values_vec_size = params.len().max(self.ty.results().len());
        let mut values_vec = store.take_wasm_vmval_storage();
        debug_assert!(values_vec.is_empty());
        values_vec.resize_with(values_vec_size, || VMVal::v128(0));

        for (arg, slot) in params.iter().cloned().zip(&mut values_vec) {
            unsafe {
                *slot = arg;
            }
        }

        self.call_unchecked_raw(store, values_vec.as_mut_ptr(), values_vec_size);
        
        results.copy_from_slice(&values_vec);
        values_vec.truncate(0);
        store.return_wasm_vmval_storage(values_vec);
    }

    unsafe fn call_unchecked_raw(
        &self,
        store: &mut Store<'static>,
        params_and_returns: *mut VMVal,
        params_and_returns_capacity: usize,
    ) {
        let func_ref = self.inner.func_ref.as_ref();
        let instance_handle = store.vmctx2instance[&NonNull::new(func_ref.vmctx.cast()).unwrap()];
        let module = store.instance_data(instance_handle).module.clone();

        let signal_handler = Box::new(
            move |signum: libc::c_int,
                  siginfo: *const libc::siginfo_t,
                  context: *const libc::c_void|
                  -> bool {
                let regs = crate::placeholder::get_trap_registers(context.cast_mut(), signum);

                if let Some(trap) =
                    crate::trap::signals::lookup_code(regs.pc).and_then(|(code, text_offset)| {
                        let func_index = module.clone().0.info.text_offset_to_func(text_offset);
                        println!("{:?}", code.symbol_map());
                        println!(
                            "{:?} {func_index:?}",
                            code.symbol_map().get(text_offset as u64)
                        );
                        (code.lookup_trap_code(text_offset))
                    })
                {
                    println!("wasm failed with trap {trap:?} {regs:#x?}");
                } else {
                    println!("not in wasm code {regs:#x?}");
                }

                process::exit(1);
            },
        );

        let res =
            crate::placeholder::catch_traps(Some(signal_handler.as_ref() as *const _), || {
                (func_ref.array_call)(
                    func_ref.vmctx.cast(), // TODO is this cast here correct??
                    ptr::null_mut(),       // No caller for now TODO change
                    params_and_returns,
                    params_and_returns_capacity,
                );
            });
    }
}
