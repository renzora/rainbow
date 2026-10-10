//! Parser for Rock Band `.dta` files (a Lisp-like format: nested lists of symbols, strings
//! and numbers), and extraction of the song metadata we need for conversion.

use anyhow::{Result, bail};

#[derive(Debug, Clone)]
pub enum Node {
    List(Vec<Node>),
    /// A symbol (`'name'` or `name`) or string (`"text"`).
    Text(String),
    Int(i64),
    Float(f64),
}

impl Node {
    fn text(&self) -> Option<&str> {
        match self {
            Node::Text(s) => Some(s),
            _ => None,
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Node::Int(i) => Some(*i as f64),
            Node::Float(f) => Some(*f),
            _ => None,
        }
    }

    fn list(&self) -> Option<&[Node]> {
        match self {
            Node::List(l) => Some(l),
            _ => None,
        }
    }
}

/// Parses a whole .dta file into its top-level nodes.
pub fn parse(src: &str) -> Result<Vec<Node>> {
    let chars: Vec<char> = src.chars().collect();
    let mut pos = 0;
    let mut stack: Vec<Vec<Node>> = vec![Vec::new()];
    while pos < chars.len() {
        let c = chars[pos];
        match c {
            ';' => {
                while pos < chars.len() && chars[pos] != '\n' {
                    pos += 1;
                }
            }
            '(' | '{' | '[' => {
                stack.push(Vec::new());
                pos += 1;
            }
            ')' | '}' | ']' => {
                let list = stack.pop().unwrap_or_default();
                match stack.last_mut() {
                    Some(parent) => parent.push(Node::List(list)),
                    None => bail!("unbalanced parentheses"),
                }
                if stack.is_empty() {
                    bail!("unbalanced parentheses");
                }
                pos += 1;
            }
            '"' => {
                let mut s = String::new();
                pos += 1;
                while pos < chars.len() && chars[pos] != '"' {
                    if chars[pos] == '\\' && pos + 1 < chars.len() {
                        pos += 1;
                    }
                    s.push(chars[pos]);
                    pos += 1;
                }
                pos += 1;
                stack.last_mut().unwrap().push(Node::Text(s));
            }
            '\'' => {
                let mut s = String::new();
                pos += 1;
                while pos < chars.len() && chars[pos] != '\'' {
                    s.push(chars[pos]);
                    pos += 1;
                }
                pos += 1;
                stack.last_mut().unwrap().push(Node::Text(s));
            }
            c if c.is_whitespace() => pos += 1,
            _ => {
                let start = pos;
                while pos < chars.len() && !chars[pos].is_whitespace() && !"()[]{};\"'".contains(chars[pos]) {
                    pos += 1;
                }
                let word: String = chars[start..pos].iter().collect();
                let node = if let Ok(i) = word.parse::<i64>() {
                    Node::Int(i)
                } else if let Ok(f) = word.parse::<f64>() {
                    Node::Float(f)
                } else {
                    Node::Text(word)
                };
                stack.last_mut().unwrap().push(node);
            }
        }
    }
    if stack.len() != 1 {
        bail!("unbalanced parentheses");
    }
    Ok(stack.pop().unwrap())
}

/// The value list of `(key ...)` inside `list`, i.e. everything after the key.
fn get<'a>(list: &'a [Node], key: &str) -> Option<&'a [Node]> {
    list.iter().filter_map(Node::list).find(|l| l.first().and_then(Node::text) == Some(key)).map(|l| &l[1..])
}

fn get_text(list: &[Node], key: &str) -> Option<String> {
    get(list, key)?.first().and_then(|n| match n {
        Node::Text(s) => Some(s.clone()),
        Node::Int(i) => Some(i.to_string()),
        Node::Float(f) => Some(f.to_string()),
        _ => None,
    })
}

fn get_num(list: &[Node], key: &str) -> Option<f64> {
    get(list, key)?.first().and_then(Node::number)
}

fn numbers(nodes: &[Node]) -> Vec<f64> {
    // Accept both (key (1 2 3)) and (key 1 2 3).
    match nodes {
        [Node::List(inner)] => inner.iter().filter_map(Node::number).collect(),
        _ => nodes.iter().filter_map(Node::number).collect(),
    }
}

#[derive(Debug, Default, Clone)]
pub struct Song {
    pub shortname: String,
    pub name: String,
    pub artist: String,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i64>,
    pub author: Option<String>,
    pub album_track: Option<i64>,
    /// Base path of the song's files inside the package, e.g. "songs/foo/foo".
    pub path: String,
    /// Instrument name ("drum", "bass", ...) and its audio channels.
    pub tracks: Vec<(String, Vec<usize>)>,
    pub crowd: Vec<usize>,
    pub pans: Vec<f64>,
    pub vols: Vec<f64>,
    pub ranks: Vec<(String, i64)>,
    pub preview_start: Option<i64>,
    pub length_ms: Option<i64>,
}

/// All songs described in a songs.dta (packs contain several).
pub fn songs(src: &str) -> Result<Vec<Song>> {
    let mut out = Vec::new();
    for node in parse(src)? {
        let Some(list) = node.list() else { continue };
        let Some(shortname) = list.first().and_then(Node::text) else { continue };
        let Some(song) = get(list, "song") else { continue };
        let tracks = get(song, "tracks")
            .and_then(|t| t.first())
            .and_then(Node::list)
            .map(|t| {
                t.iter()
                    .filter_map(Node::list)
                    .filter_map(|pair| {
                        let name = pair.first()?.text()?.to_owned();
                        Some((name, numbers(&pair[1..]).into_iter().map(|c| c as usize).collect()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let ranks = get(list, "rank")
            .map(|r| {
                r.iter()
                    .filter_map(Node::list)
                    .filter_map(|pair| Some((pair.first()?.text()?.to_owned(), pair.get(1)?.number()? as i64)))
                    .collect()
            })
            .unwrap_or_default();
        out.push(Song {
            shortname: shortname.to_owned(),
            name: get_text(list, "name").unwrap_or_else(|| shortname.to_owned()),
            artist: get_text(list, "artist").unwrap_or_default(),
            album: get_text(list, "album_name"),
            genre: get_text(list, "genre"),
            year: get_num(list, "year_released").map(|y| y as i64),
            author: get_text(list, "author"),
            album_track: get_num(list, "album_track_number").map(|n| n as i64),
            path: get_text(song, "name").unwrap_or_else(|| format!("songs/{shortname}/{shortname}")),
            tracks,
            crowd: get(song, "crowd_channels").map(numbers).unwrap_or_default().into_iter().map(|c| c as usize).collect(),
            pans: get(song, "pans").map(numbers).unwrap_or_default(),
            vols: get(song, "vols").map(numbers).unwrap_or_default(),
            ranks,
            preview_start: get(list, "preview").map(numbers).and_then(|p| p.first().map(|v| *v as i64)),
            length_ms: get_num(list, "song_length").map(|l| l as i64),
        });
    }
    Ok(out)
}

/// Converts a Rock Band difficulty rank to Clone Hero's 0–6 tier (-1 = no part), using
/// Rock Band 3's tier boundaries.
pub fn rank_to_tier(instrument: &str, rank: i64) -> i64 {
    let bounds: &[i64] = match instrument {
        "drum" => &[124, 151, 178, 242, 345, 448],
        "guitar" | "real_guitar" => &[139, 176, 221, 267, 333, 409],
        "bass" | "real_bass" => &[135, 181, 228, 293, 364, 436],
        "vocals" => &[132, 175, 218, 279, 353, 427],
        "keys" | "real_keys" => &[153, 211, 269, 327, 385, 443],
        _ => &[163, 215, 243, 267, 292, 345],
    };
    if rank <= 0 {
        return -1;
    }
    bounds.iter().filter(|&&b| rank >= b).count() as i64
}

/// Rock Band genre symbols as Clone Hero shows them, e.g. "numetal" -> "Nu-Metal".
pub fn genre_name(symbol: &str) -> String {
    let known = [
        ("alternative", "Alternative"), ("blues", "Blues"), ("classical", "Classical"), ("classicrock", "Classic Rock"),
        ("country", "Country"), ("emo", "Emo"), ("fusion", "Fusion"), ("glam", "Glam"), ("grunge", "Grunge"),
        ("hiphoprap", "Hip-Hop/Rap"), ("indierock", "Indie Rock"), ("inspirational", "Inspirational"), ("jazz", "Jazz"),
        ("jrock", "J-Rock"), ("latin", "Latin"), ("metal", "Metal"), ("new_wave", "New Wave"), ("novelty", "Novelty"),
        ("numetal", "Nu-Metal"), ("popdanceelectronic", "Pop/Dance/Electronic"), ("poprock", "Pop-Rock"),
        ("prog", "Prog"), ("punk", "Punk"), ("rbsoulfunk", "R&B/Soul/Funk"), ("reggaeska", "Reggae/Ska"),
        ("rock", "Rock"), ("southernrock", "Southern Rock"), ("world", "World"), ("other", "Other"),
    ];
    known.iter().find(|(k, _)| *k == symbol).map(|(_, v)| v.to_string()).unwrap_or_else(|| {
        let mut c = symbol.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    })
}
