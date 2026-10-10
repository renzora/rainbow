//! Unpacks .sng files (https://github.com/mdsitton/SngFileFormat) into a chart folder,
//! generating song.ini from the .sng's metadata, the same way Bridge's "Chart Folder" format does.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len()).context("truncated .sng header")?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn i32(&mut self) -> Result<usize> {
        let v = i32::from_le_bytes(self.take(4)?.try_into()?);
        usize::try_from(v).context("negative length in .sng")
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
    fn string(&mut self, n: usize) -> Result<String> {
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

/// Extracts `sng_path` into the directory `out_dir` (created if needed).
pub fn extract(sng_path: &Path, out_dir: &Path) -> Result<()> {
    let data = fs::read(sng_path)?;
    let mut r = Reader { buf: &data, pos: 0 };
    if r.take(6)? != b"SNGPKG" {
        bail!("not a .sng file");
    }
    let _version = r.take(4)?;
    let mask: [u8; 16] = r.take(16)?.try_into()?;

    let _metadata_len = r.u64()?;
    let mut ini = String::from("[song]\n");
    for _ in 0..r.u64()? {
        let key_len = r.i32()?;
        let key = r.string(key_len)?;
        let value_len = r.i32()?;
        let value = r.string(value_len)?;
        ini.push_str(&format!("{key} = {value}\n"));
    }

    let _file_meta_len = r.u64()?;
    let mut files = Vec::new();
    for _ in 0..r.u64()? {
        let name_len = r.u8()? as usize;
        let name = r.string(name_len)?;
        let len = r.u64()? as usize;
        let offset = r.u64()? as usize;
        files.push((name, offset, len));
    }

    fs::create_dir_all(out_dir)?;
    fs::write(out_dir.join("song.ini"), ini)?;
    for (name, offset, len) in files {
        // File names come from the download; never let them escape the chart folder.
        let name = Path::new(&name).file_name().context("bad file name in .sng")?.to_owned();
        let end = offset.checked_add(len).filter(|&e| e <= data.len()).context("truncated .sng file data")?;
        let unmasked: Vec<u8> = data[offset..end]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ (mask[i % 16] ^ (i % 256) as u8))
            .collect();
        fs::write(out_dir.join(name), unmasked)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Run with: SNG_SAMPLE=<file.sng> SNG_OUT=<dir> cargo test -- --ignored
    #[test]
    #[ignore]
    fn extract_sample() {
        let sample = std::env::var("SNG_SAMPLE").expect("set SNG_SAMPLE");
        let out = std::env::var("SNG_OUT").expect("set SNG_OUT");
        super::extract(sample.as_ref(), out.as_ref()).unwrap();
    }
}
