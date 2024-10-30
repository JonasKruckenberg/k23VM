use crate::store::Stored;
use crate::traps::WasmBacktrace;
use crate::vm::{TrapReason, VMContext, VMFunctionImport, VMVal};
use crate::{vm, Store, Val, MAX_WASM_STACK};
use std::mem;
use wasmparser::FuncType;

#[derive(Debug, Clone)]
pub struct Func(Stored<vm::ExportedFunction>);

impl Func {
    pub fn ty<'s>(&self, store: &'s Store) -> &'s FuncType {
        unsafe {
            let func_ref = store[self.0].func_ref.as_ref();
            let instance = &store[store.vmctx2instance(VMContext::from_opaque(func_ref.vmctx))];
            &instance.module().parsed().types[func_ref.type_index]
        }
    }
    pub fn call(
        &self,
        store: &mut Store,
        params: &[Val],
        results: &mut [Val],
    ) -> crate::Result<()> {
        // TODO typecheck params
        unsafe { self.call_unchecked(store, params, results) }
    }

    unsafe fn call_unchecked(
        &self,
        store: &mut Store,
        params: &[Val],
        results: &mut [Val],
    ) -> crate::Result<()> {
        let values_vec_size = params.len().max(self.ty(store).results().len());
        let mut values_vec = store.take_wasm_vmval_storage();
        debug_assert!(values_vec.is_empty());
        values_vec.resize_with(values_vec_size, || VMVal::v128(0));
        for (arg, slot) in params.iter().cloned().zip(&mut values_vec) {
            *slot = arg.as_vmval();
        }

        self.call_unchecked_raw(store, values_vec.as_mut_ptr(), values_vec_size)?;

        for ((i, slot), vmval) in results.iter_mut().enumerate().zip(&values_vec) {
            let ty = self.ty(store).results()[i];
            *slot = unsafe { Val::from_vmval(*vmval, ty) };
        }

        values_vec.truncate(0);
        store.return_wasm_vmval_storage(values_vec);

        Ok(())
    }

    unsafe fn call_unchecked_raw(
        &self,
        store: &mut Store,
        args_results_ptr: *mut VMVal,
        args_results_len: usize,
    ) -> crate::Result<()> {
        let func_ref = store[self.0].func_ref.as_ref();
        let vmctx = VMContext::from_opaque(func_ref.vmctx);
        let module = store[store.vmctx2instance(vmctx)].module();

        // Determine the stack pointer where, after which, any wasm code will
        // immediately trap. This is checked on the entry to all wasm functions.
        //
        // Note that this isn't 100% precise. We are requested to give wasm
        // `max_wasm_stack` bytes, but what we're actually doing is giving wasm
        // probably a little less than `max_wasm_stack` because we're
        // calculating the limit relative to this function's approximate stack
        // pointer. Wasm will be executed on a frame beneath this one (or next
        // to it). In any case it's expected to be at most a few hundred bytes
        // of slop one way or another. When wasm is typically given a MB or so
        // (a million bytes) the slop shouldn't matter too much.
        //
        // After we've got the stack limit then we store it into the `stack_limit`
        // variable.
        let stack_pointer = vm::arch::get_stack_pointer();
        let wasm_stack_limit = stack_pointer - MAX_WASM_STACK;
        let prev_stack = unsafe {
            mem::replace(
                &mut *vmctx
                    .byte_add(module.vmctx_plan().fixed.stack_limit as usize)
                    .cast::<usize>(),
                wasm_stack_limit,
            )
        };

        unsafe { crate::placeholder::register_signal_handler() };
        
        let res = vm::catch_traps(
            vmctx,
            module.vmctx_plan().fixed.clone(),
            |caller| {
                (func_ref.host_call)(vmctx, caller, args_results_ptr, args_results_len);
            },
        );

        if let Err(trap) = res {
            let (faulting_pc, trap_code, message) = match trap.reason {
                TrapReason::Wasm(trap_code) => (None, trap_code, "k23 builtin produced a trap"),
                TrapReason::Jit {
                    pc,
                    faulting_addr: _, // TODO make use of this
                    trap: trap_code,
                } => (Some(pc), trap_code, "JIT-compiled WASM produced a trap"),
            };

            let backtrace = trap
                .backtrace
                .map(|backtrace| WasmBacktrace::from_captured(module, backtrace, faulting_pc));

            return Err(crate::Error::Trap {
                backtrace,
                trap: trap_code,
                message: message.to_string(),
            });
        }

        unsafe {
            *vmctx
                .byte_add(module.vmctx_plan().fixed.stack_limit as usize)
                .cast::<usize>() = prev_stack;
        };

        Ok(())
    }

    pub(crate) fn as_vmfunction_import(&self, store: &Store) -> VMFunctionImport {
        // TODO check access
        let func_ref = unsafe { store[self.0].func_ref.as_ref() };
        VMFunctionImport {
            wasm_call: func_ref.wasm_call,
            host_call: func_ref.host_call,
            vmctx: func_ref.vmctx,
        }
    }

    pub(crate) fn from_vm_export(store: &mut Store, export: vm::ExportedFunction) -> Self {
        Self(store.push_exported_function(export))
    }
}
