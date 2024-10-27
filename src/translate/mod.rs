mod code_translator;
mod const_expr;
mod env;
mod func_translator;
mod heap;
mod module_translator;
mod state;
mod table;
mod utils;

use crate::indices::{
    DataIndex, DefinedFuncIndex, DefinedGlobalIndex, DefinedMemoryIndex, DefinedTableIndex,
    ElemIndex, EntityIndex, FieldIndex, FuncIndex, FuncRefIndex, GlobalIndex, LabelIndex,
    LocalIndex, MemoryIndex, TableIndex, TagIndex, TypeIndex,
};
use crate::{enum_accessors, DEFAULT_OFFSET_GUARD_SIZE, WASM32_MAX_SIZE};
use alloc::boxed::Box;
use alloc::vec::Vec;
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_entity::packed_option::ReservedValue;
use cranelift_entity::{entity_impl, EntityRef, PrimaryMap};
use hashbrown::{HashMap, HashSet};
use serde_derive::{Deserialize, Serialize};
use wasmparser::{
    FuncToValidate, FuncType, FunctionBody, GlobalType, MemoryType, TableType, ValidatorResources,
    WasmFeatures,
};

use crate::errors::SizeOverflow;
pub use const_expr::{ConstExpr, ConstOp};
pub use env::TranslationEnvironment;
pub use func_translator::FuncTranslator;
pub use module_translator::ModuleTranslator;

#[derive(Debug)]
pub struct Translation<'wasm> {
    pub module: TranslatedModule<'wasm>,
    pub debug_info: DebugInfo<'wasm>,
    pub required_features: WasmFeatures,
    pub func_compile_inputs: PrimaryMap<DefinedFuncIndex, FuncCompileInput<'wasm>>,
}

impl Default for Translation<'_> {
    fn default() -> Self {
        Self {
            module: TranslatedModule::default(),
            debug_info: DebugInfo::default(),
            required_features: WasmFeatures::empty(),
            func_compile_inputs: PrimaryMap::default(),
        }
    }
}

#[derive(Debug)]
pub struct FuncCompileInput<'wasm> {
    pub body: FunctionBody<'wasm>,
    pub validator: FuncToValidate<ValidatorResources>,
}

#[derive(Debug, Default)]
pub struct TranslatedModule<'wasm> {
    pub types: PrimaryMap<TypeIndex, FuncType>,

    pub functions: PrimaryMap<FuncIndex, FunctionType>,
    pub table_plans: PrimaryMap<TableIndex, TablePlan>,
    pub memory_plans: PrimaryMap<MemoryIndex, MemoryPlan>,
    pub globals: PrimaryMap<GlobalIndex, GlobalType>,

    // Instead of storing elements and data segments as-is like the spec says,
    // we split them up into active and passive initializers.
    // Active initializers are executed when the module is instantiated while passive initializers
    // are kept around and *may* be executed by table.init and memory.init instructions.
    // The spec doesn't differentiate between the two, instead it will call elem.drop/data.drop on
    // each executed *active* element during initialization, our approach is a bit cleaner IMO.
    pub global_initializers: PrimaryMap<DefinedGlobalIndex, ConstExpr>,
    pub table_initializers: TableInitializers,
    pub memory_initializers: Vec<MemoryInitializer<'wasm>>,

    pub passive_table_initializers: HashMap<ElemIndex, TableSegmentElements>,
    pub passive_memory_initializers: HashMap<DataIndex, &'wasm [u8]>,
    pub active_table_initializers: HashSet<ElemIndex>,
    pub active_memory_initializers: HashSet<DataIndex>,

    pub start: Option<FuncIndex>,
    pub imports: Vec<Import<'wasm>>,
    pub exports: HashMap<&'wasm str, EntityIndex>,

    pub num_imported_functions: u32,
    pub num_imported_tables: u32,
    pub num_imported_memories: u32,
    pub num_imported_globals: u32,
    pub num_escaped_functions: u32,
}

impl TranslatedModule<'_> {
    #[inline]
    pub fn func_index(&self, index: DefinedFuncIndex) -> FuncIndex {
        FuncIndex::from_u32(self.num_imported_functions + index.as_u32())
    }

    #[inline]
    pub fn defined_func_index(&self, index: FuncIndex) -> Option<DefinedFuncIndex> {
        if self.is_imported_func(index) {
            None
        } else {
            Some(DefinedFuncIndex::from_u32(
                index.as_u32() - self.num_imported_functions,
            ))
        }
    }

    #[inline]
    pub fn is_imported_func(&self, index: FuncIndex) -> bool {
        index.as_u32() < self.num_imported_functions
    }

    #[inline]
    pub fn table_index(&self, index: DefinedTableIndex) -> TableIndex {
        TableIndex::from_u32(self.num_imported_tables + index.as_u32())
    }

    #[inline]
    pub fn defined_table_index(&self, index: TableIndex) -> Option<DefinedTableIndex> {
        if self.is_imported_table(index) {
            None
        } else {
            Some(DefinedTableIndex::from_u32(
                index.as_u32() - self.num_imported_tables,
            ))
        }
    }

    #[inline]
    pub fn is_imported_table(&self, index: TableIndex) -> bool {
        index.as_u32() < self.num_imported_tables
    }

    #[inline]
    pub fn defined_memory_index(&self, index: MemoryIndex) -> Option<DefinedMemoryIndex> {
        if self.is_imported_memory(index) {
            None
        } else {
            Some(DefinedMemoryIndex::from_u32(
                index.as_u32() - self.num_imported_memories,
            ))
        }
    }

    #[inline]
    pub fn is_imported_memory(&self, index: MemoryIndex) -> bool {
        index.as_u32() < self.num_imported_memories
    }

    #[inline]
    pub fn global_index(&self, index: DefinedGlobalIndex) -> GlobalIndex {
        GlobalIndex::from_u32(self.num_imported_globals + index.as_u32())
    }

    #[inline]
    pub fn defined_global_index(&self, index: GlobalIndex) -> Option<DefinedGlobalIndex> {
        if self.is_imported_global(index) {
            None
        } else {
            Some(DefinedGlobalIndex::from_u32(
                index.as_u32() - self.num_imported_globals,
            ))
        }
    }

    #[inline]
    pub fn is_imported_global(&self, index: GlobalIndex) -> bool {
        index.as_u32() < self.num_imported_globals
    }

    pub fn num_imported_functions(&self) -> u32 {
        self.num_imported_functions
    }
    pub fn num_imported_tables(&self) -> u32 {
        self.num_imported_tables
    }
    pub fn num_imported_memories(&self) -> u32 {
        self.num_imported_memories
    }
    pub fn num_imported_globals(&self) -> u32 {
        self.num_imported_globals
    }
    pub fn num_defined_tables(&self) -> u32 {
        u32::try_from(self.table_plans.len()).unwrap() - self.num_imported_tables
    }
    pub fn num_defined_memories(&self) -> u32 {
        u32::try_from(self.memory_plans.len()).unwrap() - self.num_imported_memories
    }
    pub fn num_defined_globals(&self) -> u32 {
        u32::try_from(self.globals.len()).unwrap() - self.num_imported_globals
    }
    pub fn num_escaped_funcs(&self) -> u32 {
        self.num_escaped_functions
    }
}

/// The value of a WebAssembly global variable.
#[derive(Clone, Copy)]
pub enum IRGlobal {
    /// This is a constant global with a value known at compile time.
    Const(ir::Value),

    /// This is a variable in memory that should be referenced through a `GlobalValue`.
    Memory {
        /// The address of the global variable storage.
        gv: ir::GlobalValue,
        /// An offset to add to the address.
        offset: ir::immediates::Offset32,
        /// The global variable's type.
        ty: ir::Type,
    },

    /// This is a global variable that needs to be handled by the environment.
    Custom,
}

#[derive(Debug)]
pub enum EntityType {
    /// A function
    Function(FuncIndex),
    /// A table with the specified element type and limits
    Table(TableIndex),
    /// A linear memory with the specified limits
    Memory(MemoryIndex),
    /// A global variable with the specified content type
    Global(GlobalIndex),
    /// An event definition.
    Tag(TagIndex),
}

impl EntityType {
    enum_accessors! {
        e
        (Function(FuncIndex) func unwrap_func *e)
        (Table(TableIndex) table unwrap_table *e)
        (Memory(MemoryIndex) memory unwrap_memory *e)
        (Global(GlobalIndex) global unwrap_global *e)
        (Tag(TagIndex) tag unwrap_tag *e)
    }
}

#[derive(Debug, Copy, Clone)]
pub struct FunctionType {
    /// The index of the function signature in the type section.
    pub signature: TypeIndex,
    /// And index identifying this function "to the outside world"
    /// or the reserved value if the function isn't escaping from its module.
    pub func_ref: FuncRefIndex,
}

impl FunctionType {
    pub fn is_escaping(self) -> bool {
        !self.func_ref.is_reserved_value()
    }
}

#[derive(Debug)]
pub struct Import<'wasm> {
    pub module: &'wasm str,
    pub name: &'wasm str,
    pub ty: EntityType,
}

/// A pre-processed version of `wasmparser::MemoryType` describing how
/// we'll implement the memory.
#[derive(Debug, Clone)]
pub struct MemoryPlan {
    /// The minimum number of pages in the memory.
    pub minimum: u64,
    /// The maximum number of pages in the memory.
    pub maximum: Option<u64>,
    /// Whether the memory may be shared between multiple threads.
    pub shared: bool,
    /// Whether this is a 64-bit memory
    pub memory64: bool,
    /// The log2 of this memory's page size, in bytes.
    ///
    /// By default, the page size is 64KiB (0x10000; 2**16; 1<<16; 65536) but the
    /// custom-page-sizes proposal allows opting into a page size of `1`.
    pub page_size_log2: u8,
    /// The size in bytes of the offset guard for static heaps.
    pub offset_guard_size: u64,
}

impl MemoryPlan {
    pub fn for_memory(ty: MemoryType) -> Self {
        Self {
            minimum: ty.initial,
            maximum: ty.maximum,
            shared: ty.shared,
            memory64: ty.memory64,
            page_size_log2: ty
                .page_size_log2
                .map(|log2| u8::try_from(log2).unwrap())
                .unwrap_or(Self::DEFAULT_PAGE_SIZE_LOG2),
            offset_guard_size: DEFAULT_OFFSET_GUARD_SIZE,
        }
    }

    /// WebAssembly page sizes are 64KiB by default.
    pub const DEFAULT_PAGE_SIZE: u32 = 0x10000;

    /// WebAssembly page sizes are 64KiB (or `2**16`) by default.
    pub const DEFAULT_PAGE_SIZE_LOG2: u8 = {
        let log2 = 16;
        assert!(1 << log2 == Self::DEFAULT_PAGE_SIZE);
        log2
    };

    /// Returns the minimum size, in bytes, that this memory must be.
    ///
    /// # Errors
    ///
    /// Returns an error if the calculation of the minimum size overflows the
    /// `u64` return type. This means that the memory can't be allocated but
    /// it's deferred to the caller to how to deal with that.
    pub fn minimum_byte_size(&self) -> Result<u64, SizeOverflow> {
        self.minimum
            .checked_mul(self.page_size())
            .ok_or(SizeOverflow)
    }

    /// Returns the maximum size, in bytes, that this memory is allowed to be.
    ///
    /// Note that the return value here is not an `Option` despite the maximum
    /// size of a linear memory being optional in wasm. If a maximum size
    /// is not present in the memory's type then a maximum size is selected for
    /// it. For example the maximum size of a 32-bit memory is `1<<32`. The
    /// maximum size of a 64-bit linear memory is chosen to be a value that
    /// won't ever be allowed at runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the calculation of the maximum size overflows the
    /// `u64` return type. This means that the memory can't be allocated but
    /// it's deferred to the caller to how to deal with that.
    pub fn maximum_byte_size(&self) -> Result<u64, SizeOverflow> {
        match self.maximum {
            Some(max) => max.checked_mul(self.page_size()).ok_or(SizeOverflow),
            None => {
                let min = self.minimum_byte_size()?;
                Ok(min.max(self.max_size_based_on_index_type()))
            }
        }
    }

    /// Get the size of this memory's pages, in bytes.
    pub fn page_size(&self) -> u64 {
        debug_assert!(
            self.page_size_log2 == 16 || self.page_size_log2 == 0,
            "invalid page_size_log2: {}; must be 16 or 0",
            self.page_size_log2
        );
        1 << self.page_size_log2
    }

    /// Returns the maximum size memory is allowed to be only based on the
    /// index type used by this memory.
    ///
    /// For example 32-bit linear memories return `1<<32` from this method.
    pub fn max_size_based_on_index_type(&self) -> u64 {
        if self.memory64 {
            // Note that the true maximum size of a 64-bit linear memory, in
            // bytes, cannot be represented in a `u64`. That would require a u65
            // to store `1<<64`. Despite that no system can actually allocate a
            // full 64-bit linear memory so this is instead emulated as "what if
            // the kernel fit in a single Wasm page of linear memory". Shouldn't
            // ever actually be possible but it provides a number to serve as an
            // effective maximum.
            0_u64.wrapping_sub(self.page_size())
        } else {
            WASM32_MAX_SIZE
        }
    }
}

#[derive(Debug, Clone)]
pub struct TablePlan {
    pub ty: TableType,
}

impl TablePlan {
    pub fn for_table(ty: TableType) -> TablePlan {
        Self { ty }
    }
}

#[derive(Debug, Default)]
pub struct TableInitializers {
    pub initial_values: PrimaryMap<DefinedTableIndex, TableInitialValue>,
    pub segments: Vec<TableSegment>,
}

#[derive(Debug)]
pub enum TableInitialValue {
    RefNull,
    ConstExpr(ConstExpr),
}

#[derive(Debug)]
pub struct TableSegment {
    pub table_index: TableIndex,
    pub offset: ConstExpr,
    pub elements: TableSegmentElements,
}

#[derive(Debug, Clone)]
pub enum TableSegmentElements {
    Functions(Box<[FuncIndex]>),
    Expressions(Box<[ConstExpr]>),
}

#[derive(Debug)]
pub struct MemoryInitializer<'wasm> {
    pub memory_index: MemoryIndex,
    pub offset: ConstExpr,
    pub bytes: &'wasm [u8],
}

#[derive(Debug, Default)]
pub struct DebugInfo<'wasm> {
    pub names: Names<'wasm>,
    pub producers: Producers<'wasm>,
    pub dwarf: gimli::Dwarf<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_loc: gimli::DebugLoc<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_loclists: gimli::DebugLocLists<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_ranges: gimli::DebugRanges<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_rnglists: gimli::DebugRngLists<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_cu_index: gimli::DebugCuIndex<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
    pub debug_tu_index: gimli::DebugTuIndex<gimli::EndianSlice<'wasm, gimli::LittleEndian>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Names<'wasm> {
    pub module_name: Option<&'wasm str>,
    pub func_names: HashMap<FuncIndex, &'wasm str>,
    pub locals_names: HashMap<FuncIndex, HashMap<LocalIndex, &'wasm str>>,
    pub global_names: HashMap<GlobalIndex, &'wasm str>,
    pub data_names: HashMap<DataIndex, &'wasm str>,
    pub labels_names: HashMap<FuncIndex, HashMap<LabelIndex, &'wasm str>>,
    pub type_names: HashMap<TypeIndex, &'wasm str>,
    pub table_names: HashMap<TableIndex, &'wasm str>,
    pub memory_names: HashMap<MemoryIndex, &'wasm str>,
    pub element_names: HashMap<ElemIndex, &'wasm str>,
    pub fields_names: HashMap<FuncIndex, HashMap<FieldIndex, &'wasm str>>,
    pub tag_names: HashMap<TagIndex, &'wasm str>,
}

impl<'wasm> Names<'wasm> {
    pub fn module_name(&self) -> Option<&'wasm str> {
        self.module_name
    }
    pub fn func_name(&self, func_index: FuncIndex) -> Option<&'wasm str> {
        self.func_names.get(&func_index).copied()
    }
    pub fn local_name(&self, func_index: FuncIndex, local_index: LocalIndex) -> Option<&'wasm str> {
        self.locals_names
            .get(&func_index)?
            .get(&local_index)
            .copied()
    }
    pub fn global_name(&self, global_index: GlobalIndex) -> Option<&'wasm str> {
        self.global_names.get(&global_index).copied()
    }
    pub fn data_name(&self, data_index: DataIndex) -> Option<&'wasm str> {
        self.data_names.get(&data_index).copied()
    }
    pub fn labels_name(
        &self,
        func_index: FuncIndex,
        label_index: LabelIndex,
    ) -> Option<&'wasm str> {
        self.labels_names
            .get(&func_index)?
            .get(&label_index)
            .copied()
    }
    pub fn type_name(&self, type_index: TypeIndex) -> Option<&'wasm str> {
        self.type_names.get(&type_index).copied()
    }
    pub fn table_name(&self, table_index: TableIndex) -> Option<&'wasm str> {
        self.table_names.get(&table_index).copied()
    }
    pub fn memory_name(&self, memory_index: MemoryIndex) -> Option<&'wasm str> {
        self.memory_names.get(&memory_index).copied()
    }
    pub fn element_name(&self, elem_index: ElemIndex) -> Option<&'wasm str> {
        self.element_names.get(&elem_index).copied()
    }
    pub fn field_name(&self, func_index: FuncIndex, field_index: FieldIndex) -> Option<&'wasm str> {
        self.fields_names
            .get(&func_index)?
            .get(&field_index)
            .copied()
    }
    pub fn tag_name(&self, tag_index: TagIndex) -> Option<&'wasm str> {
        self.tag_names.get(&tag_index).copied()
    }
}

#[derive(Debug, Default)]
pub struct Producers<'wasm> {
    pub language: Vec<ProducersLanguageField<'wasm>>,
    pub processed_by: Vec<ProducersToolField<'wasm>>,
    pub sdk: Vec<ProducersSdkField<'wasm>>,
}

#[allow(unused)]
#[derive(Debug)]
pub struct ProducersLanguageField<'wasm> {
    pub name: ProducersLanguage<'wasm>,
    pub version: &'wasm str,
}

#[allow(unused)]
#[derive(Debug)]
pub enum ProducersLanguage<'wasm> {
    Wat,
    C,
    Cpp,
    Rust,
    JavaScript,
    Other(&'wasm str),
}

#[allow(unused)]
#[derive(Debug)]
pub struct ProducersToolField<'wasm> {
    pub name: ProducersTool<'wasm>,
    pub version: &'wasm str,
}

#[allow(unused)]
#[derive(Debug)]
pub enum ProducersTool<'wasm> {
    Wabt,
    Llvm,
    Clang,
    Lld,
    Binaryen,
    Rustc,
    WasmBindgen,
    WasmPack,
    Webassemblyjs,
    WasmSnip,
    Javy,
    Other(&'wasm str),
}

#[allow(unused)]
#[derive(Debug)]
pub struct ProducersSdkField<'wasm> {
    pub name: ProducersSdk<'wasm>,
    pub version: &'wasm str,
}

#[allow(unused)]
#[derive(Debug)]
pub enum ProducersSdk<'wasm> {
    Emscripten,
    Webpack,
    Other(&'wasm str),
}
