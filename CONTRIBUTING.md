# Contributing to Rainbow

Thanks for helping! Bug reports, translations and code are all welcome.

## Reporting a bug or asking for a feature

- **Bugs and feature requests:** open an issue at
  <https://github.com/renzora/rainbow/issues>. In Rainbow, the **🐞 Report an issue** button in
  the status bar can copy your version, operating system and recent log for you to paste in.
- **Questions and quick help:** ask on the [Renzora Discord](https://discord.gg/x6KHzEe2j2).
- Problems with Chorus Encore itself (missing or broken charts on enchor.us) belong on the
  [Encore Discord](https://discord.gg/cqaUXGm), not here.

A good report says what you did, what you expected, what happened instead, and which chart
(its MD5 is at the top of the details panel) if one is involved.

## Building

You need [Rust](https://rustup.rs) (stable).

```sh
cargo build --release      # target/release/rainbow(.exe)
cargo run --release        # build and start it
cargo test --release       # the quick tests
```

On Linux you also need the usual windowing libraries; see the `Install system libraries`
step in `.github/workflows/build.yml` for the exact packages.

The web version is the same code built for `wasm32-unknown-unknown`: `sh tools/build_web.sh`
builds it into `dist/` (see the README). Check both builds when you change anything that
touches the network, files or threads:

```sh
cargo check && cargo check --target wasm32-unknown-unknown
```

Background work goes through `task::spawn` (a thread on the desktop, the page's event loop on
the web), HTTP through `net`, and Rainbow's own files through `store`, so the same code runs on
both. Code that only makes sense on one is marked `#[cfg(target_arch = "wasm32")]` or
`#[cfg(not(target_arch = "wasm32"))]`.

## Where things are

| File | What it does |
|---|---|
| `src/main.rs` | The app: tabs, search, results table, details panel, settings |
| `src/api.rs`, `src/catalogue.rs`, `src/filter.rs`, `src/query.rs` | Encore's API, the local chart catalogue, and searching it |
| `src/chart.rs` | Reading `notes.chart` and `notes.mid` |
| `src/preview.rs`, `src/pathchart.rs` | The falling-notes preview and the star power path chart |
| `src/optimal.rs`, `src/drums.rs` | Optimal score: Clone Hero scoring and the star power path search (drums have their own rules) |
| `src/players.rs`, `src/leaderboard.rs`, `src/trackhash/` | Clone Hero players, profiles and leaderboards |
| `src/rhythmverse.rs` | RhythmVerse's charts: the published list, fetching it for the workflow, and installing downloads |
| `src/rockband.rs`, `src/stfs.rs`, `src/dta.rs` | Converting Rock Band songs |
| `src/updater.rs` | Updating from GitHub releases (`src/updater_web.rs` stands in for it on the web) |
| `src/links.rs`, `docs/c/` | `rainbow://` chart links, and the web page shared links open |
| `src/task.rs`, `src/net.rs`, `src/store.rs` | Background work, HTTP and Rainbow's own files, for both the desktop and the web |
| `src/web.rs`, `web/`, `tools/build_web.sh` | The web version: the page, its link to the browser (storage, songs folder, downloads), and building it |
| `tools/clonehero-proxy/` | The Cloudflare Worker the web version reaches Clone Hero's leaderboards through |
| `src/theme.rs`, `src/i18n.rs`, `src/flags.rs`, `src/datepicker.rs` | Look and feel, translations (and right-to-left text), language flags |
| `assets/lang/` | One file per language |

## Code style

- **Match the code around you**: its naming, comment density and layout. Comments say *why*,
  in plain sentences.
- **Don't run `cargo fmt` on the whole project.** The code isn't laid out by rustfmt, and a
  project-wide reformat buries real changes in a huge diff. Format only the lines you touch.
- Keep changes focused: one fix or feature per pull request, with no unrelated tidying.
- `cargo build` should finish **without warnings**.
- All visible text goes through translation: `t("...")`, `tr!("... {name}", name = ...)`,
  or `n("...")` for text defined in one place and translated where it's shown.

## Tests

`cargo test --release` runs the quick tests. Some tests need real charts and are marked
`#[ignore]`; each one's comment says which environment variable points it at its files, e.g.

```sh
CHART_FIXTURES=<folder of chart folders> cargo test --release optimal_matches_reference -- --ignored --nocapture
```

Don't commit charts or other people's files as test fixtures; point the tests at a local
folder instead.

## Translations

Each language is one file in `assets/lang/` mapping the English text to its translation.

- After changing visible text in the code, run `python tools/extract_strings.py` to refresh
  `assets/lang/template.json`.
- `python tools/check_translations.py` lists missing strings and broken `{placeholders}` in
  every language.
- Korean and Chinese use trimmed fonts holding only the characters the translations need. After
  changing `ko.json`, `zh-CN.json` or `zh-TW.json`, rebuild them with
  `python tools/subset_cjk.py <folder with the full Noto Sans KR/SC/TC fonts>` (see the
  script for where to get them).
- To add a language, copy `template.json` to `<code>.json` (e.g. `pt-PT.json`), fill in
  `name`, `english_name` and the strings, and open a pull request.

## Pull requests

1. Fork the repo and create a branch for your change.
2. Make the change, build it without warnings, and run the tests.
3. Open a pull request saying what it changes and why, with screenshots for anything visual.

The **Build** workflow builds Windows, macOS and Linux for every pull request, so you can see
that it compiles everywhere.

## Releases (maintainers)

Bump `version` in `Cargo.toml`, commit, and push a matching tag (`v0.2.0` for version `0.2.0`).
The workflow builds every platform, publishes the release with a `SHA256SUMS` file, and
Rainbow's built-in updater offers it to everyone.

## License

Rainbow is dual-licensed under MIT or Apache-2.0. Unless you say otherwise, anything you
contribute is licensed the same way (see the License section of the README).
