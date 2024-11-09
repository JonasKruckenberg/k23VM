use crate::indices::VMSharedTypeIndex;
use crate::runtime::{StaticVMOffsets, VMContext, VMFunctionImport, VMVal};
use crate::store::Stored;
use crate::translate::WasmFuncType;
use crate::type_registry::RegisteredType;
use crate::values::Val;
use crate::{placeholder, runtime, Store, MAX_WASM_STACK};
use core::ffi::c_void;
use core::{mem, ptr};

#[derive(Debug, Clone, Copy)]
pub struct Func(Stored<runtime::ExportedFunction>);

impl Func {
    pub fn ty<'s>(&self, store: &'s Store) -> FuncType {
        let func_ref = unsafe { store[self.0].func_ref.as_ref() };
        let ty = store
            .engine
            .type_registry()
            .get_type(&store.engine, func_ref.type_index)
            .unwrap();
        FuncType(ty)
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
        let ty = self.ty(store);
        let ty = ty.as_wasm_func_type();
        let values_vec_size = params.len().max(ty.results.len());

        // take out the argument storage from the store
        let mut values_vec = store.take_wasm_vmval_storage();
        debug_assert!(values_vec.is_empty());

        // copy the arguments into the storage
        values_vec.resize_with(values_vec_size, || VMVal::v128(0));
        for (arg, slot) in params.iter().cloned().zip(&mut values_vec) {
            *slot = arg.as_vmval(store)?;
        }

        // do the actual call
        self.call_unchecked_raw(store, values_vec.as_mut_ptr(), values_vec_size)?;

        // copy the results out of the storage
        for ((i, slot), vmval) in results.iter_mut().enumerate().zip(&values_vec) {
            let ty = ty.results[i].clone();
            *slot = unsafe { Val::from_vmval(store, *vmval, ty) };
        }

        // clean up and return the argument storage
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
        let module = store[store.get_instance_from_vmctx(vmctx)].module();

        let _guard = enter_wasm(vmctx, &module.offsets().static_);

        // TODO catch traps

        (func_ref.array_call)(vmctx, ptr::null_mut(), args_results_ptr, args_results_len);

        // TODO convert trap to error
        // TODO restore previous stack limit

        Ok(())
    }

    pub(crate) fn to_raw(&self, store: &mut Store) -> *mut c_void {
        store[self.0].func_ref.as_ptr().cast()
    }

    pub(crate) fn as_vmfunction_import(&self, store: &Store) -> VMFunctionImport {
        let func_ref = unsafe { store[self.0].func_ref.as_ref() };
        VMFunctionImport {
            wasm_call: func_ref.wasm_call,
            array_call: func_ref.array_call,
            vmctx: func_ref.vmctx,
        }
    }

    pub(crate) fn from_vm_export(store: &mut Store, export: runtime::ExportedFunction) -> Self {
        Self(store.push_function(export))
    }
}

pub fn enter_wasm(vmctx: *mut VMContext, offsets: &StaticVMOffsets) -> WasmExecutionGuard {
    let stack_pointer = placeholder::arch::get_stack_pointer();
    let wasm_stack_limit = stack_pointer - MAX_WASM_STACK;
    unsafe {
        let stack_limit_ptr = vmctx
            .byte_add(offsets.vmctx_stack_limit() as usize)
            .cast::<usize>();
        let prev_stack = mem::replace(&mut *stack_limit_ptr, wasm_stack_limit);
        WasmExecutionGuard {
            stack_limit_ptr,
            prev_stack,
        }
    }
}

struct WasmExecutionGuard {
    stack_limit_ptr: *mut usize,
    prev_stack: usize,
}

impl Drop for WasmExecutionGuard {
    fn drop(&mut self) {
        unsafe {
            *self.stack_limit_ptr = self.prev_stack;
        }
    }
}

pub struct FuncType(RegisteredType);

impl FuncType {
    pub(crate) fn type_index(&self) -> VMSharedTypeIndex {
        self.0.index()
    }

    pub fn as_wasm_func_type(&self) -> &WasmFuncType {
        self.0.unwrap_func()
    }

    pub(crate) fn into_registered_type(self) -> RegisteredType {
        self.0
    }
}
