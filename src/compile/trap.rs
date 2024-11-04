use object::{Bytes, LittleEndian, SectionKind, U32Bytes, U32};
use alloc::vec::Vec;
use core::ops::Range;
use object::write::{Object, StandardSegment};
use crate::compile::compiled_function::TrapInfo;
use crate::compile::ELF_K23_TRAPS;
use crate::Error;

#[derive(Default)]
pub struct TrapSectionBuilder {
    offsets: Vec<U32Bytes<LittleEndian>>,
    traps: Vec<u8>,
    last_offset: u32,
}

impl TrapSectionBuilder {
    pub fn push_traps(
        &mut self,
        func: &Range<u64>,
        traps: impl ExactSizeIterator<Item = TrapInfo>,
    ) {
        let func_start = u32::try_from(func.start).unwrap();

        self.offsets.reserve_exact(traps.len());
        self.traps.reserve_exact(traps.len());

        for trap in traps {
            let pos = func_start + trap.offset;
            debug_assert!(pos >= self.last_offset);
            // sanity check to make sure everything is sorted. 
            // otherwise we won't be able to use lookup later.
            self.offsets.push(U32Bytes::new(LittleEndian, pos));
            self.traps.push(trap.trap as u8);
            self.last_offset = pos;
        }

        self.last_offset = u32::try_from(func.end).unwrap();
    }

    pub fn append(self, obj: &mut Object) {
        let section = obj.add_section(
            obj.segment_name(StandardSegment::Data).to_vec(),
            ELF_K23_TRAPS.as_bytes().to_vec(),
            SectionKind::ReadOnlyData,
        );

        let amt = u32::try_from(self.offsets.len()).unwrap();
        obj.append_section_data(section, &amt.to_le_bytes(), 1);
        obj.append_section_data(section, object::bytes_of_slice(&self.offsets), 1);
        obj.append_section_data(section, &self.traps, 1);
    }
}

pub fn parse_trap_section(section: &[u8]) -> crate::Result<(&[U32<LittleEndian>], &[u8])> {
    let mut section = Bytes(section);

    let count = section
        .read::<U32<LittleEndian>>()
        .map_err(|_| Error::ObjectRead)?;
    let offsets = section
        .read_slice::<U32<LittleEndian>>(count.get(LittleEndian) as usize)
        .map_err(|_| Error::ObjectRead)?;
    let traps = section
        .read_slice::<u8>(offsets.len())
        .map_err(|_| Error::ObjectRead)?;

    Ok((offsets, traps))
}