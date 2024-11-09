use crate::runtime::{VMFunctionImport, VMVal};
use crate::store::Stored;
use crate::values::Val;
use crate::{runtime, Store};
use core::ffi::c_void;

#[derive(Debug, Clone)]
pub struct Func(Stored<runtime::ExportedFunction>);

impl Func {
    pub(crate) fn to_raw(&self, store: &mut Store) -> *mut c_void {
        store[self.0].func_ref.as_ptr().cast()
    }
}

impl Func {
    // pub fn ty<'s>(&self, store: &'s Store) -> &'s FuncType {
    //     unsafe {
    //         let func_ref = store[self.0].func_ref.as_ref();
    //         let instance = &store[store.vmctx2instance(VMContext::from_opaque(func_ref.vmctx))];
    //         let ty_index = instance.module().compiled().module.types[func_ref.type_index];
    //         // instance.module()
    //
    //         todo!()
    //     }
    // }

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
        todo!()
        // let values_vec_size = params.len().max(self.ty(store).results().len());
        // let mut values_vec = store.take_wasm_vmval_storage();
        // debug_assert!(values_vec.is_empty());
        // values_vec.resize_with(values_vec_size, || VMVal::v128(0));
        // for (arg, slot) in params.iter().cloned().zip(&mut values_vec) {
        //     *slot = arg.as_vmval(store);
        // }
        //
        // self.call_unchecked_raw(store, values_vec.as_mut_ptr(), values_vec_size)?;
        //
        // for ((i, slot), vmval) in results.iter_mut().enumerate().zip(&values_vec) {
        //     let ty = self.ty(store).results()[i];
        //     *slot = unsafe { Val::from_vmval(store, *vmval, ty) };
        // }
        //
        // values_vec.truncate(0);
        // store.return_wasm_vmval_storage(values_vec);
        //
        // Ok(())
    }

    unsafe fn call_unchecked_raw(
        &self,
        store: &mut Store,
        args_results_ptr: *mut VMVal,
        args_results_len: usize,
    ) -> crate::Result<()> {
        todo!()
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
