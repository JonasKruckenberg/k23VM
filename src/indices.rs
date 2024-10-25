use crate::enum_accessors;
use cranelift_entity::entity_impl;
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TypeIndex(u32);
entity_impl!(TypeIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FuncIndex(u32);
entity_impl!(FuncIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DefinedFuncIndex(u32);
entity_impl!(DefinedFuncIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TableIndex(u32);
entity_impl!(TableIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DefinedTableIndex(u32);
entity_impl!(DefinedTableIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MemoryIndex(u32);
entity_impl!(MemoryIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DefinedMemoryIndex(u32);
entity_impl!(DefinedMemoryIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OwnedMemoryIndex(u32);
entity_impl!(OwnedMemoryIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GlobalIndex(u32);
entity_impl!(GlobalIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DefinedGlobalIndex(u32);
entity_impl!(DefinedGlobalIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ElemIndex(u32);
entity_impl!(ElemIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DataIndex(u32);
entity_impl!(DataIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FuncRefIndex(u32);
entity_impl!(FuncRefIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LocalIndex(u32);
entity_impl!(LocalIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FieldIndex(u32);
entity_impl!(FieldIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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
/// appear in the wild probably.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LabelIndex(u32);
entity_impl!(LabelIndex);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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
        (Function(FuncIndex) func unwrap_func *e)
        (Table(TableIndex) table unwrap_table *e)
        (Memory(MemoryIndex) memory unwrap_memory *e)
        (Global(GlobalIndex) global unwrap_global *e)
        (Tag(TagIndex) tag unwrap_tag *e)
    }
}
