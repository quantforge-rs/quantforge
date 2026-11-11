//! The little of FlatBuffers a result file needs: a builder that lays a
//! buffer out as the reference FlatBuffers builder does, from its end
//! towards its start, so that the same calls always give the same bytes,
//! and a reader that looks a table's fields up and never reads outside the
//! bytes it was given.
//!
//! A buffer starts with the offset of its root table. A table starts with
//! the distance back to its vtable, which lists where each of its fields
//! is; a field left out has its default value. Tables, vectors and strings
//! are named by offsets, each counted forward from where it is written.

/// Builds one buffer. Every offset it hands out counts bytes from the end
/// of the finished buffer.
pub(super) struct Builder {
    /// The buffer's bytes, the last one first.
    reversed: Vec<u8>,
    /// The widest alignment anything in the buffer needs.
    widest: usize,
    /// The table being built: where each field is, 0 for one left out.
    fields: Vec<usize>,
    /// Where the table being built ends.
    table_end: usize,
    /// The vtables written so far: where each starts, and its bytes.
    vtables: Vec<(usize, Vec<u8>)>,
}

impl Builder {
    pub(super) fn new() -> Self {
        Self {
            reversed: Vec::new(),
            widest: 1,
            fields: Vec::new(),
            table_end: 0,
            vtables: Vec::new(),
        }
    }

    fn used(&self) -> usize {
        self.reversed.len()
    }

    /// Pads so that, once `more` further bytes are written, what follows
    /// them is aligned to `size`, a power of two.
    fn align(&mut self, size: usize, more: usize) {
        self.widest = self.widest.max(size);
        let padding = (self.used() + more).wrapping_neg() & (size - 1);
        self.reversed.resize(self.used() + padding, 0);
    }

    /// Writes `bytes` so that they read in order in the finished buffer.
    fn place(&mut self, bytes: &[u8]) {
        self.reversed.extend(bytes.iter().rev());
    }

    /// Writes the offset of the table, vector or string at `target`.
    fn offset_to(&mut self, target: usize) {
        self.align(4, 0);
        let distance = self.used() - target + 4;
        self.place(&u32_bytes(distance));
    }

    /// Writes a string and answers where it is.
    pub(super) fn string(&mut self, text: &str) -> usize {
        self.align(4, text.len() + 1);
        self.place(&[0]);
        self.place(text.as_bytes());
        self.place(&u32_bytes(text.len()));
        self.used()
    }

    /// Writes a vector of the tables or strings at `targets`.
    pub(super) fn offsets(&mut self, targets: &[usize]) -> usize {
        self.align(4, 4 * targets.len());
        for &target in targets.iter().rev() {
            self.offset_to(target);
        }
        self.place(&u32_bytes(targets.len()));
        self.used()
    }

    /// Writes a vector of structs, each `width` bytes of 64-bit fields,
    /// given as their bytes one after another.
    pub(super) fn structs(&mut self, width: usize, bytes: &[u8]) -> usize {
        self.align(4, bytes.len());
        self.align(8, bytes.len());
        self.place(bytes);
        self.place(&u32_bytes(bytes.len() / width));
        self.used()
    }

    /// Starts a table of up to `fields` fields.
    pub(super) fn start_table(&mut self, fields: usize) {
        self.fields = vec![0; fields];
        self.table_end = self.used();
    }

    /// Writes the table's field `slot`: a scalar, as its little-endian
    /// bytes.
    pub(super) fn scalar(&mut self, slot: usize, bytes: &[u8]) {
        self.align(bytes.len(), 0);
        self.place(bytes);
        self.fields[slot] = self.used();
    }

    /// Writes the table's field `slot`: the table, vector or string at
    /// `target`.
    pub(super) fn reference(&mut self, slot: usize, target: usize) {
        self.offset_to(target);
        self.fields[slot] = self.used();
    }

    /// Ends the table and answers where it is. A vtable equal to an earlier
    /// one is not written again: the table points at the earlier one.
    pub(super) fn end_table(&mut self) -> usize {
        self.align(4, 0);
        self.place(&[0; 4]);
        let table = self.used();
        let mut fields = std::mem::take(&mut self.fields);
        while fields.last() == Some(&0) {
            fields.pop();
        }
        let mut vtable = Vec::with_capacity(2 * fields.len() + 4);
        vtable.extend(u16_bytes(2 * fields.len() + 4));
        vtable.extend(u16_bytes(table - self.table_end));
        for &field in &fields {
            vtable.extend(u16_bytes(if field == 0 { 0 } else { table - field }));
        }
        let shared = self
            .vtables
            .iter()
            .find(|(_, bytes)| *bytes == vtable)
            .map(|(start, _)| *start);
        let start = match shared {
            Some(start) => start,
            None => {
                self.place(&vtable);
                self.vtables.push((self.used(), vtable));
                self.used()
            }
        };
        // The distance from the table back to its vtable, negative for a
        // vtable written before the table and so after it in the buffer.
        let back = start as i64 - table as i64;
        let back = (back as i32).to_le_bytes();
        for (byte, value) in self.reversed[table - 4..table]
            .iter_mut()
            .zip(back.iter().rev())
        {
            *byte = *value;
        }
        table
    }

    /// Ends the buffer with `root` as its root table, and answers its
    /// bytes, padded to the widest alignment anything in it needs.
    pub(super) fn finish(mut self, root: usize) -> Vec<u8> {
        self.align(self.widest, 4);
        self.offset_to(root);
        self.reversed.reverse();
        self.reversed
    }
}

fn u16_bytes(value: usize) -> [u8; 2] {
    (value as u16).to_le_bytes()
}

fn u32_bytes(value: usize) -> [u8; 4] {
    (value as u32).to_le_bytes()
}

/// A table in a buffer someone else wrote. Every read answers `None`
/// rather than look outside the buffer.
#[derive(Clone, Copy)]
pub(super) struct Table<'a> {
    buffer: &'a [u8],
    at: usize,
}

impl<'a> Table<'a> {
    /// The buffer's root table.
    pub(super) fn root(buffer: &'a [u8]) -> Option<Self> {
        let at = usize::try_from(read_u32(buffer, 0)?).ok()?;
        Self::at(buffer, at)
    }

    fn at(buffer: &'a [u8], at: usize) -> Option<Self> {
        // A table holds at least the distance to its vtable.
        buffer.get(at..at.checked_add(4)?)?;
        Some(Self { buffer, at })
    }

    /// Where the field `slot` is, or `Some(None)` when the table leaves it
    /// out.
    fn field(&self, slot: usize) -> Option<Option<usize>> {
        let back = i64::from(read_u32(self.buffer, self.at)? as i32);
        let vtable = usize::try_from(self.at as i64 - back).ok()?;
        let length = usize::from(read_u16(self.buffer, vtable)?);
        let entry = 4 + 2 * slot;
        if entry + 2 > length {
            return Some(None);
        }
        match usize::from(read_u16(self.buffer, vtable.checked_add(entry)?)?) {
            0 => Some(None),
            within => Some(Some(self.at.checked_add(within)?)),
        }
    }

    /// The one-byte field `slot`, 0 when left out.
    pub(super) fn byte(&self, slot: usize) -> Option<u8> {
        match self.field(slot)? {
            Some(at) => self.buffer.get(at).copied(),
            None => Some(0),
        }
    }

    /// The 64-bit field `slot`, 0 when left out.
    pub(super) fn long(&self, slot: usize) -> Option<i64> {
        match self.field(slot)? {
            Some(at) => read_i64(self.buffer, at),
            None => Some(0),
        }
    }

    /// The table the field `slot` names.
    pub(super) fn table(&self, slot: usize) -> Option<Table<'a>> {
        let at = self.field(slot)??;
        let target = at.checked_add(usize::try_from(read_u32(self.buffer, at)?).ok()?)?;
        Self::at(self.buffer, target)
    }

    /// The vector of structs the field `slot` names, each `width` bytes:
    /// their bytes, one after another.
    pub(super) fn structs(&self, slot: usize, width: usize) -> Option<&'a [u8]> {
        let at = self.field(slot)??;
        let vector = at.checked_add(usize::try_from(read_u32(self.buffer, at)?).ok()?)?;
        let count = usize::try_from(read_u32(self.buffer, vector)?).ok()?;
        let start = vector.checked_add(4)?;
        self.buffer
            .get(start..start.checked_add(count.checked_mul(width)?)?)
    }
}

fn read_u16(buffer: &[u8], at: usize) -> Option<u16> {
    let bytes = buffer.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

pub(super) fn read_u32(buffer: &[u8], at: usize) -> Option<u32> {
    let bytes = buffer.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

pub(super) fn read_i64(buffer: &[u8], at: usize) -> Option<i64> {
    let bytes = buffer.get(at..at.checked_add(8)?)?;
    Some(i64::from_le_bytes(bytes.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table of one 16-bit field and one string, as the reference builder
    /// lays it out: the root offset, the vtable, the table, the string.
    #[test]
    fn a_table_is_laid_out_from_the_end_with_its_vtable_before_it() {
        let mut builder = Builder::new();
        let name = builder.string("ab");
        builder.start_table(2);
        builder.reference(1, name);
        builder.scalar(0, &7_i16.to_le_bytes());
        let table = builder.end_table();
        let bytes = builder.finish(table);
        assert_eq!(
            bytes,
            [
                12, 0, 0, 0, // the root table is 12 bytes on
                8, 0, 12, 0, 6, 0, 8, 0, // vtable: 8 bytes, a table of 12, fields at 6 and 8
                8, 0, 0, 0, // the table: its vtable is 8 bytes back
                0, 0, 7, 0, // padding, then field 0
                4, 0, 0, 0, // field 1: the string is 4 bytes on
                2, 0, 0, 0, b'a', b'b', 0, 0, // the string: its length, text, a NUL, padding
            ]
        );
        let root = Table::root(&bytes).expect("a root");
        assert_eq!(root.field(0), Some(Some(18)));
        assert_eq!(root.field(1), Some(Some(20)));
        assert_eq!(root.field(2), Some(None), "a field past the vtable");
    }

    #[test]
    fn a_field_left_out_reads_as_zero_and_trailing_ones_shorten_the_vtable() {
        let mut builder = Builder::new();
        builder.start_table(4);
        builder.scalar(1, &9_i64.to_le_bytes());
        let table = builder.end_table();
        let bytes = builder.finish(table);
        let root = Table::root(&bytes).expect("a root");
        assert_eq!(root.long(0), Some(0));
        assert_eq!(root.long(1), Some(9));
        assert_eq!(root.long(3), Some(0));
        assert_eq!(bytes.len() % 8, 0, "padded to its widest field");
        // The vtable: its own length, the table's, and two fields.
        let vtable = usize::try_from(read_u32(&bytes, 0).expect("root")).expect("an offset") - 8;
        assert_eq!(read_u16(&bytes, vtable), Some(8));
    }

    #[test]
    fn equal_vtables_are_written_once() {
        let mut builder = Builder::new();
        let tables: Vec<usize> = (1..=3_i32)
            .map(|value| {
                builder.start_table(1);
                builder.scalar(0, &value.to_le_bytes());
                builder.end_table()
            })
            .collect();
        let list = builder.offsets(&tables);
        builder.start_table(1);
        builder.reference(0, list);
        let root = builder.end_table();
        let bytes = builder.finish(root);
        // The first table and the root are 8 bytes each with one field 4
        // bytes in; the second and third carry 2 bytes of padding. Four
        // tables, two vtables.
        let count = |vtable: [u8; 6]| bytes.windows(6).filter(|w| *w == vtable).count();
        assert_eq!(count([6, 0, 8, 0, 4, 0]), 1);
        assert_eq!(count([6, 0, 10, 0, 4, 0]), 1);
        let list = Table::root(&bytes).expect("a root").field(0);
        assert!(matches!(list, Some(Some(_))));
    }

    #[test]
    fn a_vector_of_structs_is_aligned_to_eight_bytes_and_reads_back() {
        let mut builder = Builder::new();
        let pair: Vec<u8> = [3_i64, 5].iter().flat_map(|v| v.to_le_bytes()).collect();
        let vector = builder.structs(16, &pair);
        builder.start_table(1);
        builder.reference(0, vector);
        let table = builder.end_table();
        let bytes = builder.finish(table);
        let found = Table::root(&bytes)
            .expect("a root")
            .structs(0, 16)
            .expect("a vector");
        assert_eq!(found, pair.as_slice());
        let start = found.as_ptr() as usize - bytes.as_ptr() as usize;
        assert_eq!(start % 8, 0);
    }

    #[test]
    fn a_reader_never_looks_outside_the_buffer() {
        let mut builder = Builder::new();
        let name = builder.string("abc");
        builder.start_table(2);
        builder.reference(1, name);
        builder.scalar(0, &1_i64.to_le_bytes());
        let table = builder.end_table();
        let bytes = builder.finish(table);
        for cut in 0..bytes.len() {
            // Whatever a cut buffer answers, it answers without a panic.
            if let Some(root) = Table::root(&bytes[..cut]) {
                let _ = (
                    root.long(0),
                    root.byte(0),
                    root.table(1),
                    root.structs(1, 16),
                );
            }
        }
        let mut wild = bytes.clone();
        wild[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Table::root(&wild).is_none());
        for at in 0..bytes.len() {
            let mut flipped = bytes.clone();
            flipped[at] = 0xff;
            if let Some(root) = Table::root(&flipped) {
                let _ = (
                    root.long(0),
                    root.byte(0),
                    root.table(1),
                    root.structs(1, 16),
                );
            }
        }
    }
}
