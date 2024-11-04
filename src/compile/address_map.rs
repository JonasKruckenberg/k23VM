use alloc::vec::Vec;
use object::{Bytes, LittleEndian, SectionKind, U32Bytes, U32};
use core::ops::Range;
use object::write::{Object, StandardSegment};
use crate::compile::{FilePos, ELF_K23_ADDRESS_MAP};
use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionAddressMapping {
    /// Where in the source wasm binary this instruction comes from, specified
    /// in an offset of bytes from the front of the file.
    pub srcloc: FilePos,

    /// Offset from the start of the function's compiled code to where this
    /// instruction is located, or the region where it starts.
    pub code_offset: u32,
}

#[derive(Default)]
pub struct AddressMapSectionBuilder {
    offsets: Vec<U32Bytes<LittleEndian>>,
    positions: Vec<U32Bytes<LittleEndian>>,
    last_offset: u32,
}

impl AddressMapSectionBuilder {
    pub fn push<'a, I>(&mut self, func: &Range<u64>, instrs: I)
        where I: ExactSizeIterator<Item = &'a InstructionAddressMapping>
    {
        let func_start = u32::try_from(func.start).unwrap();

        self.offsets.reserve_exact(instrs.len());
        self.positions.reserve_exact(instrs.len());

        for instr in instrs {
            let pos = func_start + instr.code_offset;
            // sanity check to make sure everything is sorted.
            // otherwise we won't be able to use lookup later.
            debug_assert!(pos >= self.last_offset);
            self.offsets.push(U32Bytes::new(LittleEndian, pos));
            self.positions.push(U32Bytes::new(LittleEndian, instr.srcloc.file_offset().unwrap_or(u32::MAX)));
            self.last_offset = pos;
        }
        self.last_offset = u32::try_from(func.end).unwrap();
    }

    pub fn append(self, obj: &mut Object) {
        let section = obj.add_section(
            obj.segment_name(StandardSegment::Data).to_vec(),
            ELF_K23_ADDRESS_MAP.as_bytes().to_vec(),
            SectionKind::ReadOnlyData,
        );

        let amt = u32::try_from(self.offsets.len()).unwrap();
        obj.append_section_data(section, &amt.to_le_bytes(), 1);
        obj.append_section_data(section, object::bytes_of_slice(&self.offsets), 1);
        obj.append_section_data(section, object::bytes_of_slice(&self.positions), 1);
    }
}

pub fn parse_address_map_section(section: &[u8]) -> crate::Result<(&[U32<LittleEndian>], &[U32<LittleEndian>])> {
    let mut section = Bytes(section);

    let count = section
        .read::<U32<LittleEndian>>()
        .map_err(|_| Error::ObjectRead)?;
    let offsets = section
        .read_slice::<U32<LittleEndian>>(count.get(LittleEndian) as usize)
        .map_err(|_| Error::ObjectRead)?;
    let positions = section
        .read_slice::<U32<LittleEndian>>(offsets.len())
        .map_err(|_| Error::ObjectRead)?;

    Ok((offsets, positions))
}