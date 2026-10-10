//! Reader for Xbox 360 STFS packages ("CON"/"LIVE" files), the format Rock Band customs are
//! distributed in. Only reading is supported.
//!
//! Layout notes (all from the public STFS documentation, as implemented by e.g. Velocity):
//! data lives in 0x1000-byte blocks after the header; every 0xAA data blocks are preceded by a
//! level-0 hash table, every 0xAA level-0 tables by a level-1 table, and so on. "Female"
//! packages store one copy of each hash table, "male" packages two (an active/inactive pair).
//! Each hash entry also stores the number of the file's next block.

use std::path::Path;

use anyhow::{Context, Result, bail};

const BLOCK: usize = 0x1000;
const ENTRY: usize = 0x18;
const PER_TABLE: u32 = 0xAA;
const PER_L1: u32 = 0x70E4;

pub struct Stfs {
    data: Vec<u8>,
    first_hash_table: usize,
    /// 0 for female packages (one hash table copy), 1 for male (two).
    sex: u32,
    block_step: [u32; 2],
    top_level: u32,
    block_separation: u8,
    pub files: Vec<Entry>,
}

#[derive(Clone)]
pub struct Entry {
    /// Full path inside the package, e.g. "songs/mysong/mysong.mid".
    pub path: String,
    pub is_dir: bool,
    consecutive: bool,
    blocks: u32,
    first_block: u32,
    pub size: usize,
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}

fn le24(b: &[u8], at: usize) -> u32 {
    b[at] as u32 | (b[at + 1] as u32) << 8 | (b[at + 2] as u32) << 16
}

fn be24(b: &[u8], at: usize) -> u32 {
    (b[at] as u32) << 16 | (b[at + 1] as u32) << 8 | b[at + 2] as u32
}

/// Whether a file starts like an STFS package.
pub fn is_package(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok()
        && matches!(&magic, b"CON " | b"LIVE" | b"PIRS")
}

impl Stfs {
    pub fn open(path: &Path) -> Result<Self> {
        Self::parse(std::fs::read(path)?)
    }

    pub fn parse(data: Vec<u8>) -> Result<Self> {
        if data.len() < 0x3B0 || !matches!(&data[..4], b"CON " | b"LIVE" | b"PIRS") {
            bail!("not an Xbox 360 package");
        }
        let header_size = be32(&data, 0x340) as usize;
        if be32(&data, 0x3A9) != 0 {
            bail!("this package uses SVOD, not STFS (not a Rock Band song)");
        }
        let vd = 0x379;
        let block_separation = data[vd + 2];
        let table_blocks = u16::from_le_bytes([data[vd + 3], data[vd + 4]]) as u32;
        let table_start = le24(&data, vd + 5);
        let allocated = be32(&data, vd + 0x1C);

        let sex = (!block_separation & 1) as u32;
        let block_step = if sex == 0 { [0xAB, 0x718F] } else { [0xAC, 0x723A] };
        let top_level = if allocated <= PER_TABLE { 0 } else if allocated <= PER_L1 { 1 } else { 2 };
        let mut stfs = Stfs {
            data,
            first_hash_table: (header_size + 0xFFF) & !0xFFF,
            sex,
            block_step,
            top_level,
            block_separation,
            files: Vec::new(),
        };
        stfs.read_file_table(table_start, table_blocks)?;
        Ok(stfs)
    }

    /// Physical block number of logical data block `b` (skipping interleaved hash tables).
    fn backing_block(&self, b: u32) -> u32 {
        let base = (((b + PER_TABLE) / PER_TABLE) << self.sex) + b;
        if b < PER_TABLE {
            base
        } else if b < PER_L1 {
            base + (((b + PER_L1) / PER_L1) << self.sex)
        } else {
            (1 << self.sex) + base + (((b + PER_L1) / PER_L1) << self.sex)
        }
    }

    fn block_offset(&self, b: u32) -> usize {
        ((self.backing_block(b) as usize) << 12) + self.first_hash_table
    }

    fn level0_table_block(&self, b: u32) -> u32 {
        if b < PER_TABLE {
            return 0;
        }
        let num = (b / PER_TABLE) * self.block_step[0] + (((b / PER_L1) + 1) << self.sex);
        if b / PER_L1 == 0 { num } else { num + (1 << self.sex) }
    }

    fn level1_table_block(&self, b: u32) -> u32 {
        if b < PER_L1 { self.block_step[0] } else { (1 << self.sex) + (b / PER_L1) * self.block_step[1] }
    }

    fn table_offset(&self, table_block: u32) -> usize {
        ((table_block as usize) << 12) + self.first_hash_table
    }

    fn status(&self, entry_offset: usize) -> Result<u8> {
        self.data.get(entry_offset + 0x14).copied().context("truncated hash table")
    }

    /// Offset of the hash entry describing data block `b`, using the active table copies.
    fn hash_entry(&self, b: u32) -> Result<usize> {
        let top_copy = ((self.block_separation & 2) as usize) << 0xB;
        let mut addr = self.table_offset(self.level0_table_block(b)) + (b % PER_TABLE) as usize * ENTRY;
        match self.top_level {
            0 => addr += top_copy,
            1 => {
                let l1 = self.table_offset(self.level1_table_block(b)) + top_copy + ((b / PER_TABLE) % PER_TABLE) as usize * ENTRY;
                addr += ((self.status(l1)? & 0x40) as usize) << 6;
            }
            _ => {
                let l2 = self.table_offset(self.block_step[1]) + top_copy + (b / PER_L1) as usize * ENTRY;
                let l1_copy = ((self.status(l2)? & 0x40) as usize) << 6;
                let l1 = self.table_offset(self.level1_table_block(b)) + l1_copy + ((b / PER_TABLE) % PER_TABLE) as usize * ENTRY;
                addr += ((self.status(l1)? & 0x40) as usize) << 6;
            }
        }
        Ok(addr)
    }

    fn next_block(&self, b: u32) -> Result<u32> {
        let e = self.hash_entry(b)?;
        if e + ENTRY > self.data.len() {
            bail!("truncated hash table");
        }
        Ok(be24(&self.data, e + 0x15))
    }

    fn block(&self, b: u32) -> Result<&[u8]> {
        let at = self.block_offset(b);
        let end = (at + BLOCK).min(self.data.len());
        self.data.get(at..end).context("block past end of file")
    }

    fn read_file_table(&mut self, start: u32, count: u32) -> Result<()> {
        let mut records: Vec<(String, u8, u32, u32, i16, u32)> = Vec::new();
        let mut b = start;
        for i in 0..count {
            let block = self.block(b)?.to_vec();
            for rec in block.chunks_exact(0x40) {
                let flags = rec[0x28];
                let len = (flags & 0x3F) as usize;
                if len == 0 {
                    continue;
                }
                let name = String::from_utf8_lossy(&rec[..len.min(0x28)]).into_owned();
                let blocks = le24(rec, 0x29);
                let first = le24(rec, 0x2F);
                let parent = i16::from_be_bytes([rec[0x32], rec[0x33]]);
                let size = be32(rec, 0x34);
                records.push((name, flags, blocks, first, parent, size));
            }
            if i + 1 < count {
                b = self.next_block(b)?;
            }
        }
        // Full paths: each record names its parent directory by index.
        let path_of = |mut idx: usize| -> String {
            let mut parts = Vec::new();
            for _ in 0..64 {
                let (name, _, _, _, parent, _) = &records[idx];
                parts.push(name.clone());
                if *parent < 0 || *parent as usize >= records.len() {
                    break;
                }
                idx = *parent as usize;
            }
            parts.reverse();
            parts.join("/")
        };
        self.files = (0..records.len())
            .map(|i| {
                let (_, flags, blocks, first, _, size) = records[i];
                Entry {
                    path: path_of(i),
                    is_dir: flags & 0x80 != 0,
                    consecutive: flags & 0x40 != 0,
                    blocks,
                    first_block: first,
                    size: size as usize,
                }
            })
            .collect();
        Ok(())
    }

    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>> {
        let mut out = Vec::with_capacity(entry.size);
        let mut b = entry.first_block;
        for i in 0..entry.blocks {
            let block = self.block(b)?;
            let take = (entry.size - out.len()).min(BLOCK).min(block.len());
            out.extend_from_slice(&block[..take]);
            if i + 1 < entry.blocks {
                b = if entry.consecutive { b + 1 } else { self.next_block(b)? };
            }
        }
        if out.len() != entry.size {
            bail!("{} is truncated ({} of {} bytes)", entry.path, out.len(), entry.size);
        }
        Ok(out)
    }

    /// Finds a file by path, ignoring case.
    pub fn find(&self, path: &str) -> Option<&Entry> {
        self.files.iter().find(|e| !e.is_dir && e.path.eq_ignore_ascii_case(path))
    }
}

#[cfg(test)]
mod tests {
    /// Run with: CON_SAMPLE=<file> cargo test stfs -- --ignored --nocapture
    #[test]
    #[ignore]
    fn list_sample() {
        let path = std::env::var("CON_SAMPLE").expect("set CON_SAMPLE");
        let pkg = super::Stfs::open(path.as_ref()).unwrap();
        for f in &pkg.files {
            println!("{:>10}  {}{}", f.size, f.path, if f.is_dir { "/" } else { "" });
        }
        for f in pkg.files.iter().filter(|f| !f.is_dir) {
            let data = pkg.read(f).unwrap();
            println!("read {} ok, starts {:02x?}", f.path, &data[..data.len().min(8)]);
        }
    }
}
