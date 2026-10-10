//! Official Guitar Hero / Rock Band games, identified by the source-game `icon` that Encore
//! records for each chart (e.g. `gh3`, `rb2`, and `gh3dlc` for that game's DLC).

/// (icon, game name), in release order.
pub const GAMES: &[(&str, &str)] = &[
    ("gh1", "Guitar Hero"),
    ("gh2", "Guitar Hero II"),
    ("gh80s", "Guitar Hero Encore: Rocks the 80s"),
    ("gh3", "Guitar Hero III: Legends of Rock"),
    ("gha", "Guitar Hero: Aerosmith"),
    ("ghot", "Guitar Hero: On Tour"),
    ("ghotd", "Guitar Hero On Tour: Decades"),
    ("ghotmh", "Guitar Hero On Tour: Modern Hits"),
    ("ghwt", "Guitar Hero World Tour"),
    ("ghm", "Guitar Hero: Metallica"),
    ("ghsh", "Guitar Hero Smash Hits"),
    ("ghvh", "Guitar Hero: Van Halen"),
    ("gh5", "Guitar Hero 5"),
    ("bh", "Band Hero"),
    ("ghwor", "Guitar Hero: Warriors of Rock"),
    ("ghl", "Guitar Hero Live"),
    ("ghtv", "Guitar Hero TV"),
    ("rb1", "Rock Band"),
    ("rb2", "Rock Band 2"),
    ("rbacdc", "AC/DC Live: Rock Band"),
    ("tbrb", "The Beatles: Rock Band"),
    ("lrb", "Lego Rock Band"),
    ("gdrb", "Green Day: Rock Band"),
    ("rb3", "Rock Band 3"),
    ("rbn", "Rock Band Network"),
    ("rbb", "Rock Band Blitz"),
    ("rb4", "Rock Band 4"),
    ("rbr", "Rock Band Rivals"),
];

/// Studios that made the official charts, for charts without a game icon.
const STUDIOS: [&str; 6] = ["harmonix", "neversoft", "freestylegames", "vicarious visions", "budcat", "redoctane"];

/// The game key for an icon: the base game, with " DLC" charts keyed as `<game>dlc`.
pub fn game_key(icon: Option<&str>) -> Option<String> {
    let icon = icon?.trim().to_lowercase();
    let base = icon.strip_suffix("dlc").unwrap_or(&icon);
    GAMES.iter().any(|(id, _)| *id == base).then_some(icon)
}

/// Display name for a game key, e.g. "gh3dlc" -> "Guitar Hero III: Legends of Rock DLC".
pub fn game_name(key: &str) -> String {
    let (base, dlc) = match key.strip_suffix("dlc") {
        Some(base) => (base, " DLC"),
        None => (key, ""),
    };
    let name = GAMES.iter().find(|(id, _)| *id == base).map_or(base, |(_, name)| name);
    format!("{name}{dlc}")
}

/// Sort position for a game key: release order, with each game's DLC right after it.
pub fn game_order(key: &str) -> usize {
    let (base, dlc) = match key.strip_suffix("dlc") {
        Some(base) => (base, 1),
        None => (key, 0),
    };
    GAMES.iter().position(|(id, _)| *id == base).unwrap_or(GAMES.len()) * 2 + dlc
}

/// Whether a chart comes from an official game: by its icon, or failing that its charter.
pub fn is_legacy(icon: Option<&str>, charter: Option<&str>) -> bool {
    game_key(icon).is_some()
        || charter.is_some_and(|c| {
            let c = c.to_lowercase();
            STUDIOS.iter().any(|s| c.contains(s))
        })
}
