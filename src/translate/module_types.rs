//! Interning of types within WASM.
//!
//! WASM contains a lot of types, and many of them are repeated, especially in under the
//! function-references or GC proposals (with gc types can even be recursive to this becomes
//! even more important). Interning these types makes it possible to efficiently compare
//! types.

use crate::indices::{SharedOrModuleTypeIndex, ModuleInternedRecGroupIndex, ModuleInternedTypeIndex, TypeIndex};
use crate::translate::{
    TranslatedModule, WasmArrayType, WasmCompositeType, WasmCompositeTypeInner, WasmFieldType,
    WasmFuncType, WasmHeapType, WasmHeapTypeInner, WasmRefType, WasmStorageType, WasmStructType,
    WasmSubType, WasmValType,
};
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;
use cranelift_entity::{EntityRef, PrimaryMap};
use hashbrown::HashMap;
use wasmparser::{UnpackedIndex, Validator, ValidatorId};

/// Types defined within a single WebAssembly module.
#[derive(Debug, Default)]
pub struct ModuleTypes {
    /// WASM types (functions for MVP as well as arrays and structs when the GC proposal is enabled).
    wasm_types: PrimaryMap<ModuleInternedTypeIndex, WasmSubType>,
    /// Recursion groups defined within this module (only used when the GC proposal is enabled).
    rec_groups: PrimaryMap<ModuleInternedRecGroupIndex, Range<ModuleInternedTypeIndex>>,
    /// Types that have already been interned.
    wasmparser2k23: HashMap<wasmparser::types::CoreTypeId, ModuleInternedTypeIndex>,
}

impl fmt::Display for ModuleTypes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, ty) in self.wasm_types() {
            writeln!(f, "{index:?}: {ty}")?;
        }
        Ok(())
    }
}

impl ModuleTypes {
    /// Returns an iterator over all the WASM types (functions, arrays, and structs) defined in this module.
    pub fn wasm_types(
        &self,
    ) -> impl ExactSizeIterator<Item = (ModuleInternedTypeIndex, &WasmSubType)> {
        self.wasm_types.iter()
    }

    /// Returns the number of types WASM types defined in this module.
    pub fn len_wasm_types(&self) -> usize {
        self.wasm_types.len()
    }

    /// Get the WASM type specified by `index` if it exists.
    pub fn get_wasm_type(&self, ty: ModuleInternedTypeIndex) -> Option<&WasmSubType> {
        self.wasm_types.get(ty)
    }

    /// Get the elements within a defined recursion group.
    pub fn rec_group_elements(
        &self,
        rec_group: ModuleInternedRecGroupIndex,
    ) -> impl ExactSizeIterator<Item = ModuleInternedTypeIndex> {
        let range = &self.rec_groups[rec_group];
        (range.start.as_u32()..range.end.as_u32()).map(|i| ModuleInternedTypeIndex::from_u32(i))
    }
}

/// A recursion group that is currently being defined.
struct RecGroupInProgress {
    /// The index of this recursion group.
    rec_group_index: ModuleInternedRecGroupIndex,
    /// Index into the `wasm_types` list where this recursion group starts.
    start: ModuleInternedTypeIndex,
    /// Index into the `wasm_types` list where this recursion group ends.
    end: ModuleInternedTypeIndex,
}

pub struct ModuleTypesBuilder {
    /// The `wasmparser` validator ID this builder has been crated with. Mixing types from
    /// different validators since defined IDs are only unique within a single validator.
    validator_id: ValidatorId,
    /// The types being built.
    pub types: ModuleTypes,
    /// Recursion groups that have already interned.
    seen_rec_groups: HashMap<wasmparser::types::RecGroupId, ModuleInternedRecGroupIndex>,
    /// The recursion group currently being defined.
    rec_group_in_progress: Option<RecGroupInProgress>,
}

impl ModuleTypesBuilder {
    pub fn new(validator: &Validator) -> Self {
        Self {
            validator_id: validator.id(),
            types: ModuleTypes::default(),
            seen_rec_groups: HashMap::default(),
            rec_group_in_progress: None,
        }
    }

    /// Finish building the module types.
    pub fn finish(self) -> ModuleTypes {
        self.types
    }

    /// Define a new recursion group, or return the existing one's index if it's already been defined.
    pub fn intern_rec_group(
        &mut self,
        module: &TranslatedModule,
        validator_types: wasmparser::types::TypesRef<'_>,
        rec_group_id: wasmparser::types::RecGroupId,
    ) -> crate::Result<ModuleInternedRecGroupIndex> {
        assert_eq!(validator_types.id(), self.validator_id);

        if let Some(interned) = self.seen_rec_groups.get(&rec_group_id) {
            return Ok(*interned);
        }

        self.define_new_rec_group(module, validator_types, rec_group_id)
    }

    /// Define a new recursion group that we haven't already interned.
    fn define_new_rec_group(
        &mut self,
        module: &TranslatedModule,
        validator_types: wasmparser::types::TypesRef<'_>,
        rec_group_id: wasmparser::types::RecGroupId,
    ) -> crate::Result<ModuleInternedRecGroupIndex> {
        self.start_rec_group(
            validator_types,
            validator_types.rec_group_elements(rec_group_id),
        );

        for id in validator_types.rec_group_elements(rec_group_id) {
            let ty = &validator_types[id];
            let wasm_ty = WasmparserTypeConverter::new(&self.types, module)
                // .with_rec_group(validator_types, rec_group_id) TODO
                .convert_sub_type(ty);
            self.wasm_sub_type_in_rec_group(id, wasm_ty);
        }

        Ok(self.end_rec_group(rec_group_id))
    }

    /// Start defining a new recursion group.
    fn start_rec_group(
        &mut self,
        validator_types: wasmparser::types::TypesRef<'_>,
        elems: impl ExactSizeIterator<Item = wasmparser::types::CoreTypeId>,
    ) {
        tracing::trace!("Starting rec group of length {}", elems.len());

        assert!(self.rec_group_in_progress.is_none());
        assert_eq!(validator_types.id(), self.validator_id);

        let len = elems.len();
        for (i, wasmparser_id) in elems.enumerate() {
            let interned = ModuleInternedTypeIndex::new(self.types.len_wasm_types() + i);
            tracing::trace!(
                "Reserving {interned:?} for {wasmparser_id:?} = {:?}",
                validator_types[wasmparser_id]
            );

            let old_entry = self.types.wasmparser2k23.insert(wasmparser_id, interned);
            debug_assert_eq!(
                old_entry, None,
                "should not have already inserted {wasmparser_id:?}"
            );
        }

        self.rec_group_in_progress = Some(RecGroupInProgress {
            rec_group_index: self.next_rec_group_index(),
            start: self.next_type_index(),
            end: ModuleInternedTypeIndex::new(self.types.len_wasm_types() + len),
        });
    }

    /// Finish defining a recursion group returning it's index.
    fn end_rec_group(
        &mut self,
        rec_group_id: wasmparser::types::RecGroupId,
    ) -> ModuleInternedRecGroupIndex {
        let RecGroupInProgress {
            rec_group_index,
            start,
            end,
        } = self
            .rec_group_in_progress
            .take()
            .expect("should be defining a rec group");

        tracing::trace!("Ending rec group {start:?}..{end:?}");

        debug_assert!(start.index() < self.types.len_wasm_types());
        debug_assert_eq!(
            end,
            self.next_type_index(),
            "should have defined the number of types declared in `start_rec_group`"
        );

        let idx = self.push_rec_group(start..end);
        debug_assert_eq!(idx, rec_group_index);

        self.seen_rec_groups.insert(rec_group_id, rec_group_index);
        rec_group_index
    }

    /// Define a new type within the current recursion group.
    fn wasm_sub_type_in_rec_group(&mut self, id: wasmparser::types::CoreTypeId, ty: WasmSubType) {
        assert!(
            self.rec_group_in_progress.is_some(),
            "must be defining a rec group to define new types"
        );

        let module_interned_index = self.push_type(ty);
        debug_assert_eq!(
            self.types.wasmparser2k23.get(&id),
            Some(&module_interned_index),
            "should have reserved the right module-interned index for this wasmparser type already"
        );
    }

    /// Returns the next return value of `push_rec_group`.
    fn next_rec_group_index(&self) -> ModuleInternedRecGroupIndex {
        self.types.rec_groups.next_key()
    }

    /// Adds a new recursion group.
    pub fn push_rec_group(
        &mut self,
        range: Range<ModuleInternedTypeIndex>,
    ) -> ModuleInternedRecGroupIndex {
        self.types.rec_groups.push(range)
    }

    /// Returns the next return value of `push_type`.
    fn next_type_index(&self) -> ModuleInternedTypeIndex {
        self.types.wasm_types.next_key()
    }

    /// Adds a new type to this interned list of types.
    fn push_type(&mut self, wasm_sub_type: WasmSubType) -> ModuleInternedTypeIndex {
        self.types.wasm_types.push(wasm_sub_type)
    }
}

/// A type that knows how to convert from `wasmparser` types to types in this crate.
pub struct WasmparserTypeConverter<'a> {
    types: &'a ModuleTypes,
    module: &'a TranslatedModule,
}

impl<'a> WasmparserTypeConverter<'a> {
    pub fn new(types: &'a ModuleTypes, module: &'a TranslatedModule) -> Self {
        Self { types, module }
    }

    pub fn convert_val_type(&self, ty: &wasmparser::ValType) -> WasmValType {
        use wasmparser::ValType;
        match ty {
            ValType::I32 => WasmValType::I32,
            ValType::I64 => WasmValType::I64,
            ValType::F32 => WasmValType::F32,
            ValType::F64 => WasmValType::F64,
            ValType::V128 => WasmValType::V128,
            ValType::Ref(ty) => WasmValType::Ref(self.convert_ref_type(ty)),
        }
    }

    pub fn convert_ref_type(&self, ty: &wasmparser::RefType) -> WasmRefType {
        WasmRefType {
            nullable: ty.is_nullable(),
            heap_type: self.convert_heap_type(&ty.heap_type()),
        }
    }

    pub fn convert_heap_type(&self, ty: &wasmparser::HeapType) -> WasmHeapType {
        match ty {
            wasmparser::HeapType::Concrete(index) => self.lookup_heap_type(*index),
            wasmparser::HeapType::Abstract { shared, ty } => {
                use wasmparser::AbstractHeapType;
                use WasmHeapTypeInner::*;
                let ty = match ty {
                    AbstractHeapType::Func => Func,
                    AbstractHeapType::Extern => Extern,
                    AbstractHeapType::Any => Any,
                    AbstractHeapType::None => None,
                    AbstractHeapType::NoExtern => NoExtern,
                    AbstractHeapType::NoFunc => NoFunc,
                    AbstractHeapType::Eq => Eq,
                    AbstractHeapType::Struct => Struct,
                    AbstractHeapType::Array => Array,
                    AbstractHeapType::I31 => I31,
                    AbstractHeapType::Exn => Exn,
                    AbstractHeapType::NoExn => NoExn,
                    AbstractHeapType::Cont => Cont,
                    AbstractHeapType::NoCont => NoCont,
                };

                WasmHeapType {
                    shared: *shared,
                    ty,
                }
            }
        }
    }

    pub fn convert_sub_type(&self, ty: &wasmparser::SubType) -> WasmSubType {
        WasmSubType {
            is_final: ty.is_final,
            supertype: ty
                .supertype_idx
                .map(|index| SharedOrModuleTypeIndex::Module(self.lookup_type_index(index.unpack()))),
            composite_type: self.convert_composite_type(&ty.composite_type),
        }
    }

    pub fn convert_composite_type(&self, ty: &wasmparser::CompositeType) -> WasmCompositeType {
        use wasmparser::CompositeInnerType;
        match &ty.inner {
            CompositeInnerType::Func(func) => {
                WasmCompositeType::new_func(ty.shared, self.convert_func_type(func))
            }
            CompositeInnerType::Array(array) => {
                WasmCompositeType::new_array(ty.shared, self.convert_array_type(array))
            }
            CompositeInnerType::Struct(strct) => {
                WasmCompositeType::new_struct(ty.shared, self.convert_struct_type(strct))
            }
            CompositeInnerType::Cont(_) => todo!(),
        }
    }

    pub fn convert_func_type(&self, ty: &wasmparser::FuncType) -> WasmFuncType {
        let mut params = Vec::with_capacity(ty.params().len());
        let mut results = Vec::with_capacity(ty.results().len());

        for param in ty.params() {
            params.push(self.convert_val_type(param));
        }

        for result in ty.results() {
            results.push(self.convert_val_type(result));
        }

        WasmFuncType {
            params: params.into_boxed_slice(),
            results: results.into_boxed_slice(),
        }
    }

    pub fn convert_array_type(&self, ty: &wasmparser::ArrayType) -> WasmArrayType {
        WasmArrayType(self.convert_field_type(&ty.0))
    }

    pub fn convert_struct_type(&self, ty: &wasmparser::StructType) -> WasmStructType {
        let fields: Vec<_> = ty
            .fields
            .iter()
            .map(|ty| self.convert_field_type(ty))
            .collect();
        WasmStructType {
            fields: fields.into_boxed_slice(),
        }
    }

    pub fn convert_field_type(&self, ty: &wasmparser::FieldType) -> WasmFieldType {
        WasmFieldType {
            mutable: ty.mutable,
            element_type: self.convert_storage_type(&ty.element_type),
        }
    }

    pub fn convert_storage_type(&self, ty: &wasmparser::StorageType) -> WasmStorageType {
        use wasmparser::StorageType;
        match ty {
            StorageType::I8 => WasmStorageType::I8,
            StorageType::I16 => WasmStorageType::I16,
            StorageType::Val(ty) => WasmStorageType::Val(self.convert_val_type(ty)),
        }
    }

    fn lookup_type_index(&self, index: UnpackedIndex) -> ModuleInternedTypeIndex {
        match index {
            UnpackedIndex::Module(index) => {
                let module_index = TypeIndex::from_u32(index);
                self.module.types[module_index]
            }
            UnpackedIndex::Id(id) => self.types.wasmparser2k23[&id],
            UnpackedIndex::RecGroup(_) => unreachable!(),
        }
    }

    fn lookup_heap_type(&self, index: UnpackedIndex) -> WasmHeapType {
        match index {
            UnpackedIndex::Module(module_index) => {
                let module_index = TypeIndex::from_u32(module_index);
                let index = self.module.types[module_index];
                if let Some(ty) = self.types.get_wasm_type(index) {
                    match ty.composite_type.inner {
                        WasmCompositeTypeInner::Func(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteFunc(SharedOrModuleTypeIndex::Module(index)),
                        ),
                        WasmCompositeTypeInner::Array(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteArray(SharedOrModuleTypeIndex::Module(index)),
                        ),
                        WasmCompositeTypeInner::Struct(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteStruct(SharedOrModuleTypeIndex::Module(index)),
                        ),
                    }
                } else {
                    todo!()
                }
            }
            UnpackedIndex::Id(id) => {
                let index = self.types.wasmparser2k23[&id];
                if let Some(ty) = self.types.get_wasm_type(index) {
                    match ty.composite_type.inner {
                        WasmCompositeTypeInner::Func(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteFunc(SharedOrModuleTypeIndex::Module(index)),
                        ),
                        WasmCompositeTypeInner::Array(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteArray(SharedOrModuleTypeIndex::Module(index)),
                        ),
                        WasmCompositeTypeInner::Struct(_) => WasmHeapType::new(
                            ty.composite_type.shared,
                            WasmHeapTypeInner::ConcreteStruct(SharedOrModuleTypeIndex::Module(index)),
                        ),
                    }
                } else {
                    todo!()
                }
            }
            UnpackedIndex::RecGroup(_) => unreachable!(),
        }
    }
}
