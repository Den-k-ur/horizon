//! `horizon-data.bin` (#4267): prepared vanilla tables in one file that
//! `horizon-prepare` writes from the player's own copy of the game, next to
//! the files it re-encodes, and that the game maps instead of parsing them
//! into its heap. The pages are file-backed: shared between processes and
//! read in only when touched. Nothing here is ever shipped.
//!
//! Layout (little-endian): a header, a section table, then the sections,
//! each 8-aligned.
//!
//! ```text
//! magic "HZDATA\0\0" | format u32 | sections u32 | key [u8; 32]
//! sections × (name [u8; 24], offset u64, length u64)
//! ```
//!
//! `key` names what the file was built from (the caller hashes its source
//! files): a file whose key, format, magic or any bound does not match is
//! refused, and the caller falls back to its normal loader.
//!
//! A [`Table`] section maps sorted keys to byte strings:
//!
//! ```text
//! count u32 | 0 u32 | count × (key offset u32, key length u32,
//!                              value offset u32, value length u32) | bytes
//! ```
//! with offsets into the bytes after the entries.
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub mod prepared;

pub const FILE_NAME: &str = "horizon-data.bin";
const MAGIC: [u8; 8] = *b"HZDATA\0\0";
const FORMAT: u32 = 1;
const NAME_BYTES: usize = 24;
const HEADER_BYTES: usize = 8 + 4 + 4 + 32;
const SECTION_BYTES: usize = NAME_BYTES + 8 + 8;
const ENTRY_BYTES: usize = 16;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}
fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// Builds a data file in memory, then writes it in one rename.
pub struct Writer {
    key: [u8; 32],
    sections: Vec<(String, Vec<u8>)>,
}
impl Writer {
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            key,
            sections: Vec::new(),
        }
    }
    /// A raw section. Names are at most 24 bytes and unique.
    pub fn section(&mut self, name: &str, bytes: Vec<u8>) -> io::Result<()> {
        if name.is_empty() || name.len() > NAME_BYTES {
            return Err(invalid(format!("section name {name:?}")));
        }
        if self.sections.iter().any(|(n, _)| n == name) {
            return Err(invalid(format!("section {name} twice")));
        }
        self.sections.push((name.to_owned(), bytes));
        Ok(())
    }
    /// A [`Table`] section of `entries` (keys unique).
    pub fn table(&mut self, name: &str, mut entries: Vec<(Vec<u8>, Vec<u8>)>) -> io::Result<()> {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        if entries.windows(2).any(|w| w[0].0 == w[1].0) {
            return Err(invalid(format!("section {name}: a key twice")));
        }
        let count = u32::try_from(entries.len()).map_err(|_| invalid("too many entries"))?;
        let mut head = Vec::with_capacity(8 + entries.len() * ENTRY_BYTES);
        head.extend(count.to_le_bytes());
        head.extend(0u32.to_le_bytes());
        let mut blob = Vec::new();
        for (key, value) in &entries {
            for part in [key, value] {
                let offset = u32::try_from(blob.len()).map_err(|_| invalid("table too big"))?;
                let length = u32::try_from(part.len()).map_err(|_| invalid("entry too big"))?;
                head.extend(offset.to_le_bytes());
                head.extend(length.to_le_bytes());
                blob.extend_from_slice(part);
            }
        }
        head.extend(blob);
        self.section(name, head)
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let align = |n: usize| n.next_multiple_of(8);
        let mut offset = align(HEADER_BYTES + self.sections.len() * SECTION_BYTES);
        let mut out = Vec::with_capacity(offset);
        out.extend(MAGIC);
        out.extend(FORMAT.to_le_bytes());
        out.extend((self.sections.len() as u32).to_le_bytes());
        out.extend(self.key);
        for (name, bytes) in &self.sections {
            let mut padded = [0u8; NAME_BYTES];
            padded[..name.len()].copy_from_slice(name.as_bytes());
            out.extend(padded);
            out.extend((offset as u64).to_le_bytes());
            out.extend((bytes.len() as u64).to_le_bytes());
            offset = align(offset + bytes.len());
        }
        for (_, bytes) in &self.sections {
            out.resize(align(out.len()), 0);
            out.extend_from_slice(bytes);
        }
        out
    }
    /// Write to `path` through a temporary file of this call's own and a
    /// rename. However many write at once (a window and its server thread,
    /// two servers on one install), a reader maps the old file or a new one,
    /// never one being written, and a game keeps the file it has mapped.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut temp = PathBuf::from(path);
        temp.set_extension(format!(
            "tmp{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&self.to_bytes())
            .and_then(|()| file.sync_all())
            .and_then(|()| fs::rename(&temp, path))
            .inspect_err(|_| {
                let _ = fs::remove_file(&temp);
            })
    }
}

enum Bytes {
    Mapped(memmap2::Mmap),
    #[cfg_attr(not(test), allow(dead_code))]
    Owned(Vec<u8>),
}
impl std::ops::Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Mapped(map) => map,
            Self::Owned(bytes) => bytes,
        }
    }
}

/// An opened data file: its sections, every bound checked once.
pub struct DataFile {
    bytes: Bytes,
    sections: Vec<(String, usize, usize)>,
}
impl DataFile {
    /// Map `path` if it was built from `key`.
    pub fn open(path: &Path, key: &[u8; 32]) -> io::Result<Self> {
        let file = fs::File::open(path)?;
        // SAFETY: the file is only ever replaced by a rename (`Writer::write`),
        // never written in place, so the mapped bytes do not change under us.
        #[allow(unsafe_code)]
        let map = unsafe { memmap2::Mmap::map(&file)? };
        Self::parse(Bytes::Mapped(map), key)
    }
    fn parse(bytes: Bytes, key: &[u8; 32]) -> io::Result<Self> {
        if bytes.get(..8) != Some(&MAGIC[..]) {
            return Err(invalid("not a Horizon data file"));
        }
        if u32_at(&bytes, 8) != Some(FORMAT) {
            return Err(invalid("data file format"));
        }
        if bytes.get(16..48) != Some(&key[..]) {
            return Err(invalid("data file built from other files"));
        }
        let count = u32_at(&bytes, 12).ok_or_else(|| invalid("truncated header"))? as usize;
        let mut sections = Vec::with_capacity(count);
        for i in 0..count {
            let at = HEADER_BYTES + i * SECTION_BYTES;
            let name = bytes
                .get(at..at + NAME_BYTES)
                .ok_or_else(|| invalid("truncated section table"))?;
            let name = std::str::from_utf8(name)
                .map_err(|_| invalid("section name"))?
                .trim_end_matches('\0')
                .to_owned();
            let offset = u64_at(&bytes, at + NAME_BYTES).ok_or_else(|| invalid("section"))?;
            let length = u64_at(&bytes, at + NAME_BYTES + 8).ok_or_else(|| invalid("section"))?;
            let (offset, length) = (offset as usize, length as usize);
            if offset % 8 != 0
                || offset
                    .checked_add(length)
                    .is_none_or(|end| end > bytes.len())
            {
                return Err(invalid(format!("section {name} out of bounds")));
            }
            sections.push((name, offset, length));
        }
        Ok(Self { bytes, sections })
    }
    pub fn section(&self, name: &str) -> Option<&[u8]> {
        let (_, offset, length) = self.sections.iter().find(|(n, ..)| n == name)?;
        self.bytes.get(*offset..offset + length)
    }
    /// A [`Table`] section, its entries checked.
    pub fn table(&self, name: &str) -> io::Result<Table<'_>> {
        let bytes = self
            .section(name)
            .ok_or_else(|| invalid(format!("no section {name}")))?;
        Table::new(bytes).map_err(|e| invalid(format!("section {name}: {e}")))
    }
    /// A [`Table`] section kept with its file, checked once here.
    pub fn into_table(self, name: &str) -> io::Result<OwnedTable> {
        let count = self.table(name)?.len();
        let (_, offset, length) = self
            .sections
            .iter()
            .find(|(n, ..)| n == name)
            .cloned()
            .ok_or_else(|| invalid(format!("no section {name}")))?;
        Ok(OwnedTable {
            file: self,
            offset,
            length,
            count,
        })
    }
}

/// A checked [`Table`] section that owns its file.
pub struct OwnedTable {
    file: DataFile,
    offset: usize,
    length: usize,
    count: usize,
}
impl OwnedTable {
    pub fn table(&self) -> Table<'_> {
        let bytes = &self.file.bytes[self.offset..self.offset + self.length];
        let end = 8 + self.count * ENTRY_BYTES;
        Table {
            entries: &bytes[8..end],
            blob: &bytes[end..],
        }
    }
}

/// Sorted keys to byte strings, read in place.
#[derive(Clone, Copy)]
pub struct Table<'a> {
    entries: &'a [u8],
    blob: &'a [u8],
}
impl<'a> Table<'a> {
    fn new(bytes: &'a [u8]) -> io::Result<Self> {
        let count = u32_at(bytes, 0).ok_or_else(|| invalid("truncated table"))? as usize;
        let end = count
            .checked_mul(ENTRY_BYTES)
            .and_then(|n| n.checked_add(8))
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| invalid("truncated table"))?;
        let table = Self {
            entries: &bytes[8..end],
            blob: &bytes[end..],
        };
        let mut previous: Option<&[u8]> = None;
        for i in 0..count {
            let key = table
                .part(i, 0)
                .ok_or_else(|| invalid("key out of bounds"))?;
            table
                .part(i, 8)
                .ok_or_else(|| invalid("value out of bounds"))?;
            if previous.is_some_and(|p| p >= key) {
                return Err(invalid("keys not sorted"));
            }
            previous = Some(key);
        }
        Ok(table)
    }
    fn part(&self, i: usize, at: usize) -> Option<&'a [u8]> {
        let offset = u32_at(self.entries, i * ENTRY_BYTES + at)? as usize;
        let length = u32_at(self.entries, i * ENTRY_BYTES + at + 4)? as usize;
        self.blob.get(offset..offset.checked_add(length)?)
    }
    pub fn len(&self) -> usize {
        self.entries.len() / ENTRY_BYTES
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn get(&self, key: &[u8]) -> Option<&'a [u8]> {
        self.value(self.position(key)?)
    }
    /// The index of `key`'s entry, in key order.
    pub fn position(&self, key: &[u8]) -> Option<usize> {
        let (mut low, mut high) = (0, self.len());
        while low < high {
            let mid = (low + high) / 2;
            match self.part(mid, 0)?.cmp(key) {
                std::cmp::Ordering::Less => low = mid + 1,
                std::cmp::Ordering::Greater => high = mid,
                std::cmp::Ordering::Equal => return Some(mid),
            }
        }
        None
    }
    pub fn value(&self, i: usize) -> Option<&'a [u8]> {
        self.part(i, 8)
    }
    pub fn iter(&self) -> impl Iterator<Item = (&'a [u8], &'a [u8])> + '_ {
        (0..self.len()).filter_map(|i| Some((self.part(i, 0)?, self.part(i, 8)?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Writer {
        let mut writer = Writer::new([7; 32]);
        writer.section("raw", b"abc".to_vec()).unwrap();
        writer
            .table(
                "loot",
                vec![
                    (b"minecraft:b".to_vec(), b"{\"pools\":[]}".to_vec()),
                    (b"minecraft:a".to_vec(), b"{}".to_vec()),
                ],
            )
            .unwrap();
        writer
    }

    #[test]
    fn sections_and_tables_round_trip() {
        let file = DataFile::parse(Bytes::Owned(sample().to_bytes()), &[7; 32]).unwrap();
        assert_eq!(file.section("raw"), Some(&b"abc"[..]));
        let table = file.table("loot").unwrap();
        assert_eq!(table.len(), 2);
        assert_eq!(table.get(b"minecraft:a"), Some(&b"{}"[..]));
        assert_eq!(table.get(b"minecraft:b"), Some(&b"{\"pools\":[]}"[..]));
        assert_eq!(table.get(b"minecraft:c"), None);
        let keys: Vec<_> = table.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, [&b"minecraft:a"[..], &b"minecraft:b"[..]]);
    }

    #[test]
    fn other_sources_and_damage_are_refused() {
        let bytes = sample().to_bytes();
        assert!(DataFile::parse(Bytes::Owned(bytes.clone()), &[8; 32]).is_err());
        assert!(
            DataFile::parse(Bytes::Owned(bytes[..bytes.len() - 1].to_vec()), &[7; 32]).is_err()
        );
        let mut wrong = bytes.clone();
        wrong[0] = b'X';
        assert!(DataFile::parse(Bytes::Owned(wrong), &[7; 32]).is_err());
        // A table entry pointing past its bytes.
        let file = DataFile::parse(Bytes::Owned(bytes), &[7; 32]).unwrap();
        let (_, offset, _) = file
            .sections
            .iter()
            .find(|(n, ..)| n == "loot")
            .unwrap()
            .clone();
        let mut broken = file.bytes.to_vec();
        broken[offset + 8 + 4..offset + 8 + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        let file = DataFile::parse(Bytes::Owned(broken), &[7; 32]).unwrap();
        assert!(file.table("loot").is_err());
    }

    /// Writers at once never show a reader a file being written: each
    /// writes its own temporary file and renames it whole.
    #[test]
    fn concurrent_writers_never_show_a_partial_file() {
        let dir = std::env::temp_dir().join(format!("horizon-data-race-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        let mut big = Writer::new([7; 32]);
        big.section("raw", vec![5; 4 << 20]).unwrap();
        big.write(&path).unwrap();
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| (0..8).for_each(|_| big.write(&path).unwrap()));
            }
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..64 {
                        let file = DataFile::open(&path, &[7; 32]).unwrap();
                        let raw = file.section("raw").unwrap();
                        assert!(raw.len() == 4 << 20 && raw.iter().all(|&b| b == 5));
                    }
                });
            }
        });
        // No temporary file left behind.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_by_rename_and_maps() {
        let dir = std::env::temp_dir().join(format!("horizon-data-file-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        sample().write(&path).unwrap();
        let file = DataFile::open(&path, &[7; 32]).unwrap();
        assert_eq!(
            file.table("loot").unwrap().get(b"minecraft:a"),
            Some(&b"{}"[..])
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
