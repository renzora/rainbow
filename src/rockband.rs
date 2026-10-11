//! Converts Rock Band CON packages (as found on RhythmVerse etc.) into Clone Hero chart
//! folders, like Nautilus does: song.ini from songs.dta, notes.mid, album.png, and the
//! multitrack .mogg split into per-instrument Ogg Vorbis stems.

use std::fs::{self, File};
use std::io::{BufWriter, Cursor};
use std::num::NonZeroU8;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisDecoder, VorbisEncoderBuilder};

use crate::api::Chart;
use crate::dta::{self, Song};
use crate::stfs::Stfs;

pub use crate::stfs::is_package;

/// Converts every song in the package into `<songs_dir>/<folder name>/`. Returns the folders.
/// `charter` is used for songs whose songs.dta doesn't say who charted them.
pub fn convert(package: &Path, songs_dir: &Path, folder_template: &str, charter: Option<&str>) -> Result<Vec<PathBuf>> {
    let pkg = Stfs::open(package)?;
    let dta_file = pkg.find("songs/songs.dta").context("no songs/songs.dta: not a Rock Band song package")?;
    let dta_bytes = pkg.read(dta_file)?;
    let latin1 = || dta_bytes.iter().map(|&b| b as char).collect::<String>();
    let text = match std::str::from_utf8(&dta_bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => latin1(),
    };
    let songs = dta::songs(&text)?;
    if songs.is_empty() {
        bail!("songs.dta has no songs");
    }
    let mut out = Vec::new();
    for mut song in songs {
        if song.author.as_deref().is_none_or(|a| a.trim().is_empty()) {
            song.author = charter.map(str::to_owned);
        }
        let song = &song;
        let dir = convert_song(&pkg, song, songs_dir, folder_template)
            .with_context(|| format!("{} - {}", song.artist, song.name))?;
        out.push(dir);
    }
    Ok(out)
}

fn convert_song(pkg: &Stfs, song: &Song, songs_dir: &Path, template: &str) -> Result<PathBuf> {
    let base = song.path.trim_matches('/');
    let mogg = pkg.find(&format!("{base}.mogg")).context("no .mogg audio")?;
    let mid = pkg.find(&format!("{base}.mid")).context("no .mid chart")?;

    // Folder name from the user's template, filled from the DTA.
    let chart = Chart {
        name: Some(song.name.clone()),
        artist: Some(song.artist.clone()),
        album: song.album.clone(),
        genre: song.genre.as_deref().map(dta::genre_name),
        year: song.year.map(|y| y.to_string()),
        charter: song.author.clone(),
        md5: song.shortname.clone(),
        ..Default::default()
    };
    let rel = crate::api::resolve_folder_name(template, &chart);
    let dir = unique_dir(&songs_dir.join(rel));
    fs::create_dir_all(&dir)?;

    let result = (|| -> Result<()> {
        fs::write(dir.join("notes.mid"), pkg.read(mid)?)?;
        fs::write(dir.join("song.ini"), song_ini(song))?;
        // Album art is optional; a bad texture shouldn't fail the whole song.
        let shortname = base.rsplit('/').next().unwrap_or(base);
        let parent = base.rsplit_once('/').map_or("", |(p, _)| p);
        if let Some(art) = pkg.find(&format!("{parent}/gen/{shortname}_keep.png_xbox"))
            && let Ok(png) = pkg.read(art).and_then(|b| xbox_texture_to_png(&b))
        {
            fs::write(dir.join("album.png"), png)?;
        }
        split_mogg(&pkg.read(mogg)?, song, &dir)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    result.map(|()| dir)
}

fn unique_dir(path: &Path) -> PathBuf {
    let mut p = path.to_path_buf();
    let mut n = 2;
    while p.exists() {
        p = PathBuf::from(format!("{} ({n})", path.display()));
        n += 1;
    }
    p
}

fn song_ini(s: &Song) -> String {
    let mut ini = String::from("[song]\n");
    let mut line = |k: &str, v: String| ini.push_str(&format!("{k} = {v}\n"));
    line("name", s.name.clone());
    line("artist", s.artist.clone());
    if let Some(a) = &s.album {
        line("album", a.clone());
    }
    if let Some(g) = &s.genre {
        line("genre", dta::genre_name(g));
    }
    if let Some(y) = s.year {
        line("year", y.to_string());
    }
    line("charter", s.author.clone().unwrap_or_else(|| "Unknown".into()));
    if let Some(t) = s.album_track {
        line("album_track", t.to_string());
    }
    if let Some(l) = s.length_ms {
        line("song_length", l.to_string());
    }
    if let Some(p) = s.preview_start {
        line("preview_start_time", p.to_string());
    }
    for (rb, ch) in [("guitar", "diff_guitar"), ("bass", "diff_bass"), ("drum", "diff_drums"), ("keys", "diff_keys"), ("vocals", "diff_vocals"), ("band", "diff_band"), ("real_keys", "diff_keys_real")] {
        let tier = s.ranks.iter().find(|(k, _)| k == rb).map_or(-1, |(_, r)| dta::rank_to_tier(rb, *r));
        line(ch, tier.to_string());
    }
    // Rock Band drums always have cymbal markers.
    line("pro_drums", "True".into());
    line("icon", "rb3".into());
    line("loading_phrase", "Converted from Rock Band by Rainbow".into());
    ini
}

/// Splits the multitrack audio into Clone Hero stems, mixing each instrument's channels to
/// stereo with the DTA's pans and volumes.
fn split_mogg(mogg: &[u8], song: &Song, dir: &Path) -> Result<()> {
    if mogg.len() < 8 {
        bail!("audio file is too small");
    }
    let version = u32::from_le_bytes(mogg[..4].try_into()?);
    let offset = u32::from_le_bytes(mogg[4..8].try_into()?) as usize;
    if version != 0x0A {
        bail!("the audio is encrypted (official Rock Band DLC can't be converted)");
    }
    let ogg = mogg.get(offset..).context("bad audio header")?;
    let mut decoder = VorbisDecoder::new(Cursor::new(ogg.to_vec()))?;
    let channels = decoder.channels().get() as usize;
    let rate = decoder.sampling_frequency();

    // Which stem each channel goes to; anything unassigned is the backing track.
    let stem_of = |name: &str| match name {
        "drum" => "drums",
        "bass" => "bass",
        "guitar" => "guitar",
        "vocals" => "vocals",
        "keys" => "keys",
        _ => "song",
    };
    let mut stems: Vec<&str> = vec!["song"; channels];
    for (inst, chans) in &song.tracks {
        for &c in chans {
            if c < channels {
                stems[c] = stem_of(inst);
            }
        }
    }
    for &c in &song.crowd {
        if c < channels {
            stems[c] = "crowd";
        }
    }
    // Per-channel stereo gains from pan (-1 left .. 1 right) and volume (dB).
    let gains: Vec<(f32, f32)> = (0..channels)
        .map(|c| {
            let pan = song.pans.get(c).copied().unwrap_or(0.0) as f32;
            let gain = 10f32.powf(song.vols.get(c).copied().unwrap_or(0.0) as f32 / 20.0);
            (gain * (1.0 - pan).min(1.0), gain * (1.0 + pan).min(1.0))
        })
        .collect();

    let mut names: Vec<&str> = stems.clone();
    names.sort();
    names.dedup();
    let mut encoders = Vec::new();
    for name in &names {
        let file = BufWriter::new(File::create(dir.join(format!("{name}.ogg")))?);
        let mut builder = VorbisEncoderBuilder::new(rate, NonZeroU8::new(2).unwrap(), file)?;
        builder.bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr { target_quality: 0.6 });
        encoders.push(builder.build()?);
    }

    // Loudest sample per stem; non-multitrack customs leave the instrument channels silent.
    let mut peak = vec![0f32; names.len()];
    let mut left: Vec<Vec<f32>> = vec![Vec::new(); names.len()];
    let mut right: Vec<Vec<f32>> = vec![Vec::new(); names.len()];
    while let Some(block) = decoder.decode_audio_block()? {
        let samples = block.samples();
        let frames = samples.first().map_or(0, |s| s.len());
        for k in 0..names.len() {
            left[k].clear();
            left[k].resize(frames, 0.0);
            right[k].clear();
            right[k].resize(frames, 0.0);
        }
        for (c, data) in samples.iter().enumerate() {
            let k = names.iter().position(|n| *n == stems[c]).unwrap();
            let (gl, gr) = gains[c];
            for (i, &v) in data.iter().enumerate() {
                left[k][i] += v * gl;
                right[k][i] += v * gr;
            }
        }
        for (k, enc) in encoders.iter_mut().enumerate() {
            for v in left[k].iter_mut().chain(right[k].iter_mut()) {
                peak[k] = peak[k].max(v.abs());
                *v = v.clamp(-1.0, 1.0);
            }
            enc.encode_audio_block([&left[k][..], &right[k][..]])?;
        }
    }
    for enc in encoders {
        enc.finish()?;
    }
    // Drop silent stems (but always keep the backing track).
    for (k, name) in names.iter().enumerate() {
        if *name != "song" && peak[k] < 1e-3 {
            let _ = fs::remove_file(dir.join(format!("{name}.ogg")));
        }
    }
    Ok(())
}

/// Decodes a Rock Band `.png_xbox` album texture (DXT1/DXT5, 16-bit byte-swapped) to PNG.
pub fn xbox_texture_to_png(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 32 {
        bail!("texture too small");
    }
    let bpp = data[1];
    let width = u16::from_le_bytes([data[7], data[8]]) as usize;
    let height = u16::from_le_bytes([data[9], data[10]]) as usize;
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        bail!("bad texture size {width}x{height}");
    }
    let block_bytes = match bpp {
        4 => 8,
        8 => 16,
        _ => bail!("unsupported texture format {bpp}"),
    };
    let (bw, bh) = (width.div_ceil(4), height.div_ceil(4));
    let mut raw = data.get(32..32 + bw * bh * block_bytes).context("texture truncated")?.to_vec();
    // Xbox 360 stores the data as big-endian 16-bit words.
    for pair in raw.chunks_exact_mut(2) {
        pair.swap(0, 1);
    }
    let mut rgba = vec![0u8; width * height * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let block = &raw[(by * bw + bx) * block_bytes..][..block_bytes];
            let (alpha, color) = if block_bytes == 16 { (Some(&block[..8]), &block[8..]) } else { (None, block) };
            let pixels = decode_bc_block(color, alpha);
            for (i, px) in pixels.iter().enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < width && y < height {
                    rgba[(y * width + x) * 4..][..4].copy_from_slice(px);
                }
            }
        }
    }
    let img = image::RgbaImage::from_raw(width as u32, height as u32, rgba).context("bad texture")?;
    let mut png = Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)?;
    Ok(png.into_inner())
}

/// Decodes one 4x4 DXT block (BC1 colour, optional BC3 alpha) into 16 RGBA pixels.
fn decode_bc_block(color: &[u8], alpha: Option<&[u8]>) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([color[0], color[1]]);
    let c1 = u16::from_le_bytes([color[2], color[3]]);
    let rgb = |c: u16| {
        let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
        [(r * 255 / 31) as i32, (g * 255 / 63) as i32, (b * 255 / 31) as i32]
    };
    let (a, b) = (rgb(c0), rgb(c1));
    let mix = |wa: i32, wb: i32, d: i32| [0, 1, 2].map(|i| ((a[i] * wa + b[i] * wb) / d) as u8);
    let four_colour = c0 > c1 || alpha.is_some();
    let palette: [[u8; 4]; 4] = {
        let p2 = if four_colour { mix(2, 1, 3) } else { mix(1, 1, 2) };
        let p3 = if four_colour { mix(1, 2, 3) } else { [0, 0, 0] };
        let alpha3 = if four_colour { 255 } else { 0 };
        [
            [a[0] as u8, a[1] as u8, a[2] as u8, 255],
            [b[0] as u8, b[1] as u8, b[2] as u8, 255],
            [p2[0], p2[1], p2[2], 255],
            [p3[0], p3[1], p3[2], alpha3],
        ]
    };
    let indices = u32::from_le_bytes([color[4], color[5], color[6], color[7]]);
    let mut out = [[0u8; 4]; 16];
    for (i, px) in out.iter_mut().enumerate() {
        *px = palette[((indices >> (2 * i)) & 3) as usize];
    }
    if let Some(al) = alpha {
        let (a0, a1) = (al[0] as u32, al[1] as u32);
        let bits = al[2..8].iter().rev().fold(0u64, |acc, &b| (acc << 8) | b as u64);
        let levels: [u32; 8] = if a0 > a1 {
            [a0, a1, (6 * a0 + a1) / 7, (5 * a0 + 2 * a1) / 7, (4 * a0 + 3 * a1) / 7, (3 * a0 + 4 * a1) / 7, (2 * a0 + 5 * a1) / 7, (a0 + 6 * a1) / 7]
        } else {
            [a0, a1, (4 * a0 + a1) / 5, (3 * a0 + 2 * a1) / 5, (2 * a0 + 3 * a1) / 5, (a0 + 4 * a1) / 5, 0, 255]
        };
        for (i, px) in out.iter_mut().enumerate() {
            px[3] = levels[((bits >> (3 * i)) & 7) as usize] as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    /// Run with: CON_SAMPLE=<file> CON_OUT=<dir> cargo test convert_sample -- --ignored --nocapture
    #[test]
    #[ignore]
    fn convert_sample() {
        let path = std::env::var("CON_SAMPLE").expect("set CON_SAMPLE");
        let out = std::env::var("CON_OUT").expect("set CON_OUT");
        std::fs::create_dir_all(&out).unwrap();
        let dirs = super::convert(path.as_ref(), out.as_ref(), "{artist} - {name} ({charter})", None).unwrap();
        for d in dirs {
            println!("converted to {}", d.display());
            for f in std::fs::read_dir(&d).unwrap().flatten() {
                println!("  {:>10}  {}", f.metadata().unwrap().len(), f.file_name().to_string_lossy());
            }
        }
    }
}
