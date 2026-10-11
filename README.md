<p align="center"><img src="assets/logo.png" width="320" alt="Rainbow"></p>

# Rainbow

A fast desktop app for searching and downloading Clone Hero / YARG charts from
[Chorus Encore](https://www.enchor.us). It's a lightweight alternative to
[Bridge](https://github.com/Geomitron/Bridge), written in Rust.

It also runs in your browser, nothing to install: **<https://renzora.github.io/rainbow/>**
(see [The web version](#the-web-version)).

![Rainbow's search window, listing charts with their artist, album, genre, year, charter, length, difficulty and tags](assets/preview.png)

## Features

- **Instant search** across Encore's whole library (~95k charts). Rainbow keeps a local
  catalogue of chart metadata, so results update as you type; it only fetches what's new on
  later launches.
- **Advanced search** like Bridge's: per-field name/artist/album/genre/year/charter filters
  with *Exact* and *Exclude*, length, intensity, average/max NPS, year range, modified-after
  date, MD5 hash, track hash, and any/yes/no toggles for forced/open/tap notes, solos,
  lyrics, vocals, chart issues, video backgrounds, modcharts, roll lanes and 2x kick.
- **Instrument, difficulty and drum type** pickers.
- **Quick options:** *All songs*, *No videos*, *No broken songs* (hides charts Encore reports
  as unplayable).
- **Sortable, configurable table** (choose columns, default or compact layout).
- **Downloads** as `.sng` files or extracted **chart folders** (with `song.ini`), with or
  without video backgrounds, named by a template like `{artist} - {name} ({charter})`
  (use `/` for subfolders).
- **Bulk downloads** of selected charts, every search result, or the entire library, with
  progress, stop/resume (already-downloaded charts are skipped), and a polite delay between
  downloads.
- **Rock Band song conversion**: drop Rock Band CON files (e.g. from RhythmVerse) onto the
  window, or put them in your songs folder, and Rainbow turns them into Clone Hero charts:
  `song.ini` from `songs.dta`, `notes.mid`, `album.png`, and the multitrack audio split into
  per-instrument Ogg Vorbis stems. It's a quick path for unencrypted customs only: encrypted
  official DLC can't be converted. For anything beyond the basics, use
  [Nautilus](https://github.com/NemosNautilus/Nautilus).
- **Chart preview**: watch any chart's notes fall down a highway before downloading it.
- **Clone Hero leaderboards**: a song's details show its top scores on the
  [Clone Hero leaderboards](https://leaderboards.clonehero.net) for each instrument and difficulty.
  Rainbow fetches just the chart's notes and works out the track hash the leaderboards use (a port
  of [scan-chart](https://github.com/Geomitron/scan-chart)'s).
- **Automatic .zip extraction** for charts downloaded from other sites.
- **Accessibility**: a choice of fonts (including Atkinson Hyperlegible, Lexend and
  OpenDyslexic), colour-blind safe colours, a high-contrast theme, reduce motion, full
  keyboard control (press F1 for the list) and screen reader support through AccessKit.
- Clickable artists, albums, charters, games, difficulties and more, a Legacy filter for the
  official Guitar Hero / Rock Band charts, album art, and several colour themes.
- Respects Encore's API rate limits automatically.

## Building

Requires Rust (stable). On Windows the icon is embedded into the `.exe`.

```sh
cargo build --release
# target/release/rainbow(.exe)
```

## The web version

The same app, compiled to WebAssembly, at <https://renzora.github.io/rainbow/>. It has
everything the desktop app has except updating itself, `rainbow://` links, and importing zips
and Rock Band songs. Differences:

- **Downloads:** in Chrome and Edge you choose your songs folder once (Settings → Chart library,
  or the first time you download)
  and charts are saved straight into it as `.sng` files (browsers won't create the `song.ini` a
  chart folder needs); the browser asks for permission once per visit. Other
  browsers save each chart as an ordinary download (a `.sng` file).
- **Catalogue:** a copy of Encore's whole chart list is published with the page and refreshed
  hourly, so it's ready in seconds. While it's open, Rainbow checks for newer charts every five
  minutes, shared between all visitors through the proxy below (so Encore is asked at most once
  every five minutes, however many people are using it).
- **Clone Hero leaderboards** need a small proxy, because Clone Hero's API only answers its own
  website; see [`tools/clonehero-proxy`](tools/clonehero-proxy). Without it, the Players and
  Leaderboards tabs are left out.
- Settings, the catalogue and caches are kept in the browser's storage for the site.

To build it yourself you need the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` (at
the same version as the `wasm-bindgen` crate in `Cargo.lock`):

```sh
rustup target add wasm32-unknown-unknown
sh tools/build_web.sh                 # builds dist/
python -m http.server 8765 -d dist    # then open http://localhost:8765
```

`.github/workflows/web.yml` builds it and publishes it to GitHub Pages on every push to `main`
(and hourly, for the catalogue; the app itself is only rebuilt when its code changes). The repository's Pages source must be set to **GitHub Actions**.

## Translations

Rainbow follows your system language and has a language picker in the status bar. It ships
with English plus Spanish, Brazilian Portuguese, French, German, Italian, Dutch, Polish,
Swedish, Czech, Turkish, Russian, Ukrainian, Indonesian, Vietnamese, Japanese, Korean and
Simplified and Traditional Chinese.

Each language is one file in `assets/lang/` that maps the English text to its translation.
To fix a translation or add a language without rebuilding, put a file in the `lang` folder
inside Rainbow's data folder (see [Data](#data)); copy `assets/lang/template.json` to start a
new one, and name the file after the language code, e.g. `pt-PT.json`. For contributors:

- `python tools/extract_strings.py` refreshes `template.json` after code changes.
- `python tools/check_translations.py` checks every language for missing strings and
  broken `{placeholders}`.
- `python tools/subset_cjk.py <fonts folder>` rebuilds the small Korean and Chinese fonts
  after their translations change.

Arabic is supported, with the layout mirrored right to left: the UI library can't join
Arabic letters or lay text out right to left, so Rainbow does that itself (`src/i18n.rs`).
Other complex scripts (Hindi, Thai) aren't supported yet.

## Fonts

Rainbow bundles its fonts, all under the SIL Open Font License (see `assets/fonts`). The
default, Rainbow Rounded, is Doppio One by Sorkin Type with its ligature table removed.

## Data

Rainbow keeps its settings, chart catalogue and language overrides in each system's usual
place, and album covers in the system's cache folder:

| | Data | Cover cache |
|---|---|---|
| Windows | `%LOCALAPPDATA%\rainbow` | `%LOCALAPPDATA%\rainbow\covers` |
| macOS | `~/Library/Application Support/Rainbow` | `~/Library/Caches/Rainbow/covers` |
| Linux | `~/.local/share/rainbow` (`$XDG_DATA_HOME`) | `~/.cache/rainbow/covers` (`$XDG_CACHE_HOME`) |

Each songs folder also gets a `.rainbow-downloaded.txt` list, so re-runs skip charts you
already have. (The web version reads and adds to it too.)

## Be nice to Encore

Encore is a free, donation-funded service. Rainbow uses the same API as Bridge, but please
keep bulk-download delays reasonable, and consider
[supporting Encore on Patreon](https://www.patreon.com/cw/ChorusEncore701?l=en-GB).
Questions about Encore itself? Join the [Encore Discord](https://discord.gg/cqaUXGm).
Rainbow is not affiliated with Chorus Encore, Bridge or Clone Hero.

## Credits

Created by [Renzora Studios](https://github.com/renzora).

Big thanks to [Nautilus](https://github.com/NemosNautilus/Nautilus) by NemosNautilus, the Rock
Band tool suite the community has relied on since 2013. Rainbow's song conversion only covers the
basics; Nautilus does far more, and it's well worth supporting.
Find it at [nemosnautilus.com](https://nemosnautilus.com/nautilus/) and on its Discord.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed
as above, without any additional terms or conditions.

The bundled fonts in `assets/fonts` keep their own SIL Open Font License (see the `OFL*.txt`
files there). The flags in `assets/flags` are from flag-icons, under the MIT License (see the
`LICENSE` file there).
