use alloc::boxed::Box;
use crate::builtins::BuiltinFunctionIndex;
use crate::indices::FuncIndex;
use crate::{NS_BUILTIN, NS_WASM_FUNC};
use cranelift_codegen::ir::{ExternalName, StackSlots, UserExternalName, UserExternalNameRef};
use cranelift_codegen::{binemit, Final, FinalizedMachReloc, FinalizedRelocTarget, MachBufferFinalized, ValueLabelsRanges};
use cranelift_entity::PrimaryMap;
use crate::compile::address_map::InstructionAddressMapping;
use crate::compile::FilePos;
use crate::traps::Trap;

#[derive(Debug)]
pub struct CompiledFunction {
    /// The machine code buffer for this function.
    pub buffer: MachBufferFinalized<Final>,
    /// What names each name ref corresponds to.
    name_map: PrimaryMap<UserExternalNameRef, UserExternalName>,
    /// The alignment for the compiled function.
    pub alignment: u32,
    /// The metadata for the compiled function, including unwind information
    /// the function address map.
    pub metadata: CompiledFunctionMetadata,
}

impl CompiledFunction {
    pub(crate) fn new(
        buffer: MachBufferFinalized<Final>,
        name_map: PrimaryMap<UserExternalNameRef, UserExternalName>,
        alignment: u32,
    ) -> Self {
        Self {
            buffer,
            name_map,
            alignment,
            metadata: CompiledFunctionMetadata::default(),
        }
    }
    
    /// Returns an iterator to the function's relocation information.
    pub fn relocations(&self) -> impl Iterator<Item = Relocation> + '_ {
        self.buffer
            .relocs()
            .iter()
            .map(|r| Relocation::from_mach_reloc(r, &self.name_map))
    }

    /// Returns an iterator to the function's traps.
    pub fn traps(&self) -> impl ExactSizeIterator<Item = TrapInfo> + '_ {
        self.buffer.traps().iter().map(|trap| TrapInfo {
            trap: Trap::from_trap_code(trap.code).expect("unexpected trap code"),
            offset: trap.offset,
        })
    }

    /// Get a reference to the compiled function metadata.
    pub fn metadata(&self) -> &CompiledFunctionMetadata {
        &self.metadata
    }
}

#[derive(Debug, Default)]
pub struct CompiledFunctionMetadata {
    /// Mapping of value labels and their locations.
    pub value_labels_ranges: ValueLabelsRanges,
    /// Allocated stack slots.
    pub sized_stack_slots: StackSlots,
    /// Start source location.
    pub start_srcloc: FilePos,
    /// End source location.
    pub end_srcloc: FilePos,
    /// An array of data for the instructions in this function, indicating where
    /// each instruction maps back to in the original function.
    ///
    /// This array is sorted least-to-greatest by the `code_offset` field.
    /// Additionally the span of each `InstructionAddressMap` is implicitly the
    /// gap between it and the next item in the array.
    pub address_map: Box<[InstructionAddressMapping]>,
}

#[derive(Debug)]
pub struct Relocation {
    pub kind: binemit::Reloc,
    pub target: RelocationTarget,
    pub addend: binemit::Addend,
    pub offset: binemit::CodeOffset,
}

impl Relocation {
    fn from_mach_reloc(
        reloc: &FinalizedMachReloc,
        name_map: &PrimaryMap<UserExternalNameRef, UserExternalName>,
    ) -> Self {
        let &FinalizedMachReloc {
            offset,
            kind,
            ref target,
            addend,
        } = reloc;

        let target = match *target {
            FinalizedRelocTarget::ExternalName(ExternalName::User(user_func_ref)) => {
                let name = &name_map[user_func_ref];
                match name.namespace {
                    // A reference to another jit'ed WASM function
                    NS_WASM_FUNC => RelocationTarget::Wasm(FuncIndex::from_u32(name.index)),
                    // A reference to a WASM builtin
                    NS_BUILTIN => {
                        RelocationTarget::Builtin(BuiltinFunctionIndex::from_u32(name.index))
                    }
                    _ => panic!("unknown namespace {}", name.namespace),
                }
            }
            FinalizedRelocTarget::ExternalName(ExternalName::LibCall(libcall)) => {
                // cranelift libcalls are a lot like wasm builtins, they are emitted for instructions
                // that have no ISA equivalent and would be too complicated to emit as JIT code
                todo!("libcalls {libcall:?}")
            }
            _ => panic!("unsupported relocation target {target:?}"),
        };

        Self {
            kind,
            target,
            addend,
            offset,
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum RelocationTarget {
    Wasm(FuncIndex),
    Builtin(BuiltinFunctionIndex),
}

/// Information about a trap in a compiled function.
pub struct TrapInfo {
    /// The offset relative to the function start of the trapping address.
    pub offset: u32,
    /// The trap code corresponding to the trapping instruction.
    pub trap: Trap,
}