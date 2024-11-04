use crate::enum_accessors;
use cranelift_entity::entity_impl;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeIndex(u32);
entity_impl!(TypeIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FuncIndex(u32);
entity_impl!(FuncIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinedFuncIndex(u32);
entity_impl!(DefinedFuncIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableIndex(u32);
entity_impl!(TableIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinedTableIndex(u32);
entity_impl!(DefinedTableIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemoryIndex(u32);
entity_impl!(MemoryIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinedMemoryIndex(u32);
entity_impl!(DefinedMemoryIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalIndex(u32);
entity_impl!(GlobalIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DefinedGlobalIndex(u32);
entity_impl!(DefinedGlobalIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ElemIndex(u32);
entity_impl!(ElemIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DataIndex(u32);
entity_impl!(DataIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FuncRefIndex(u32);
entity_impl!(FuncRefIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LocalIndex(u32);
entity_impl!(LocalIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldIndex(u32);
entity_impl!(FieldIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TagIndex(u32);
entity_impl!(TagIndex);

/// A reference to a label in a function. Only used for associating label names.
///
/// NOTE: These indices are local to the function they used in, they are also
/// **not** the same as the depth of their block. This means you cant just go
/// and take the relative branch depth of a `br` instruction and the label stack
/// height to get the label index.
/// According to the proposal the labels are assigned indices in the order their
/// blocks appear in the code.
///
/// Source:
/// https://github.com/WebAssembly/extended-name-section/blob/main/proposals/extended-name-section/Overview.md#label-names
///
/// ALSO NOTE: No existing tooling appears to emit label names, so this just doesn't
/// appear in the wild, probably.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LabelIndex(u32);
entity_impl!(LabelIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EntityIndex {
    Function(FuncIndex),
    Table(TableIndex),
    Memory(MemoryIndex),
    Global(GlobalIndex),
    Tag(TagIndex),
}

impl EntityIndex {
    enum_accessors! {
        e
        (Function(FuncIndex) is_func func unwrap_func *e)
        (Table(TableIndex) is_table table unwrap_table *e)
        (Memory(MemoryIndex) is_memory memory unwrap_memory *e)
        (Global(GlobalIndex) is_global global unwrap_global *e)
        (Tag(TagIndex) is_tag tag unwrap_tag *e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleInternedRecGroupIndex(u32);
entity_impl!(ModuleInternedRecGroupIndex);

#[repr(transparent)] // Used directly by JIT code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VMSharedTypeIndex(u32);
entity_impl!(VMSharedTypeIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleInternedTypeIndex(u32);
entity_impl!(ModuleInternedTypeIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SharedOrModuleTypeIndex {
    Shared(VMSharedTypeIndex),
    Module(ModuleInternedTypeIndex),
}

impl From<VMSharedTypeIndex> for SharedOrModuleTypeIndex {
    fn from(index: VMSharedTypeIndex) -> Self {
        Self::Shared(index)
    }
}
impl From<ModuleInternedTypeIndex> for SharedOrModuleTypeIndex {
    fn from(index: ModuleInternedTypeIndex) -> Self {
        Self::Module(index)
    }
}

impl SharedOrModuleTypeIndex {
    enum_accessors! {
        e
        (Shared(VMSharedTypeIndex) is_shared shared unwrap_shared *e)
        (Module(ModuleInternedTypeIndex) is_module module unwrap_module *e)
    }
}