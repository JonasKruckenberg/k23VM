use crate::store::Store;
use crate::vmcontext::VMVal;

struct Func {}

impl Func {
    pub fn call(&self, store: &mut Store<'_>, params: &[VMVal], results: &mut [VMVal]) {
        // TODO check signature matches provided params and results capacity
        unsafe { self.call_unchecked(store, params, results) }
    }

    pub async fn call_async(&self, store: &mut Store<'_>, params: &[VMVal], results: &mut [VMVal]) {
        // TODO check signature matches provided params and results capacity
        // TODO on separate stack (Fiber) do self.call_unchecked()
    }

    unsafe fn call_unchecked(
        &self,
        store: &mut Store<'_>,
        params: &[VMVal],
        results: &mut [VMVal],
    ) {
        // TODO take store's `wasm_val_raw_storage` vec for array call
        // TODO resize array call vec
        // TODO copy params into array call vec

        // self.call_unchecked_raw(store, );

        // TODO copy results out of array call vec into `results`
        // TODO return store's `wasm_val_raw_storage`
    }

    unsafe fn call_unchecked_raw(
        &self,
        store: &mut Store<'_>,
        params_and_returns: *mut VMVal,
        params_and_returns_capacity: usize,
    ) {
        // TODO setup catch_traps HOW???
        // TODO obtain `NonNull<VMFuncRef>` for this from store
        // TODO call `func_ref.array_call`

        // let func_ref = func_ref.as_ref();
        // (func_ref.array_call)(
        //     func_ref.vmctx,
        //     caller.cast::<VMOpaqueContext>(),
        //     params_and_returns,
        //     params_and_returns_capacity,
        // )
    }
}
