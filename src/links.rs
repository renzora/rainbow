//! `rainbow://` links that open a chart in Rainbow, e.g. `rainbow://9403b89cc3d2`: the chart's
//! MD5, or just its start (12 characters is plenty to tell Encore's charts apart).
//!
//! Rainbow registers itself as the handler for these when it starts (in the user's own
//! settings, no installer needed). Opening a link starts Rainbow with the link as its argument;
//! if Rainbow is already running, the new copy hands the link over and quits.
//!
//! Chat apps only make web addresses clickable, so shared links go through a small page on
//! GitHub Pages (`docs/c/index.html`) that opens the `rainbow://` link.

#[cfg(not(target_arch = "wasm32"))]
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::net::{Ipv4Addr, TcpListener, TcpStream};
#[cfg(not(target_arch = "wasm32"))]
use web_time::Duration;

/// The link scheme.
pub const SCHEME: &str = "rainbow";
/// The web page shared links go through, which opens Rainbow (`<page>?<id>`).
pub const WEB_PAGE: &str = "https://renzora.github.io/rainbow/c/";
/// The web version of Rainbow, which opens a chart from `<app>?c=<id>`.
pub const WEB_APP: &str = "https://renzora.github.io/rainbow/";
/// How many characters of the MD5 a copied link uses.
pub const SHORT: usize = 12;
/// The shortest chart id a link may have.
const MIN_ID: usize = 8;
/// A running Rainbow listens here (on this computer only) for links from new copies.
#[cfg(not(target_arch = "wasm32"))]
const PORT: u16 = 47_917;

/// The chart id (lower-case hex, 8 to 32 characters) in a `rainbow://` or Rainbow web link.
pub fn chart_id(link: &str) -> Option<String> {
    let link = link.trim();
    let rest = if let Some(rest) = link.strip_prefix(&format!("{SCHEME}:")) {
        rest
    } else if let Some(rest) = link.strip_prefix(WEB_PAGE) {
        rest
    } else {
        link.strip_prefix(WEB_APP)?.strip_prefix("?c=")?
    };
    let id = rest
        .trim_start_matches('/')
        .trim_start_matches('?')
        .trim_start_matches("chart/")
        .split(['/', '?', '&', '#'])
        .find(|p| !p.is_empty())?
        .trim_start_matches("md5=")
        .to_lowercase();
    (id.len() >= MIN_ID && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some(id)
}

/// The shareable web link for a chart (clickable in chat apps; it opens Rainbow).
pub fn share_link(md5: &str) -> String {
    format!("{WEB_PAGE}?{}", &md5[..md5.len().min(SHORT)])
}

/// The `rainbow://` link for a chart, which opens Rainbow directly.
pub fn app_link(md5: &str) -> String {
    format!("{SCHEME}://{}", &md5[..md5.len().min(SHORT)])
}

/// A link Rainbow was started with, if any.
#[cfg(not(target_arch = "wasm32"))]
pub fn from_args() -> Option<String> {
    std::env::args().skip(1).find_map(|a| chart_id(&a))
}

/// Gives the link to a Rainbow that's already running. Returns false if there isn't one.
#[cfg(not(target_arch = "wasm32"))]
pub fn hand_over(id: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, PORT).into(), Duration::from_millis(500)) else {
        return false;
    };
    writeln!(stream, "{id}").is_ok()
}

/// Listens for links from copies started later, calling `open` with each chart id.
#[cfg(not(target_arch = "wasm32"))]
pub fn listen(open: impl Fn(String) + Send + 'static) {
    let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, PORT)) else {
        return; // another Rainbow has it
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut line = String::new();
            // One short line; only a well-formed chart id is acted on.
            if BufReader::new(stream.take(64)).read_line(&mut line).is_ok()
                && let Some(id) = chart_id(&format!("{SCHEME}:{}", line.trim()))
            {
                open(id);
            }
        }
    });
}

/// Makes this copy of Rainbow the handler for `rainbow://` links, for this user.
#[cfg(not(target_arch = "wasm32"))]
pub fn register() {
    let Ok(exe) = std::env::current_exe() else { return };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x0800_0000;
        let exe = exe.to_string_lossy();
        let key = format!(r"HKCU\Software\Classes\{SCHEME}");
        let command = format!("\"{exe}\" \"%1\"");
        let icon = format!("\"{exe}\",0");
        for args in [
            vec!["add", key.as_str(), "/ve", "/d", "URL:Rainbow", "/f"],
            vec!["add", key.as_str(), "/v", "URL Protocol", "/d", "", "/f"],
            vec!["add", &format!(r"{key}\DefaultIcon"), "/ve", "/d", &icon, "/f"],
            vec!["add", &format!(r"{key}\shell\open\command"), "/ve", "/d", &command, "/f"],
        ] {
            let _ = std::process::Command::new("reg").args(&args).creation_flags(NO_WINDOW).output();
        }
    }
    #[cfg(target_os = "linux")]
    {
        let Some(dir) = dirs::data_dir().map(|d| d.join("applications")) else { return };
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=Rainbow\nExec=\"{}\" %u\nNoDisplay=true\nMimeType=x-scheme-handler/{SCHEME};\n",
            exe.display()
        );
        if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(dir.join("rainbow-link.desktop"), desktop).is_ok() {
            let _ = std::process::Command::new("xdg-mime").args(["default", "rainbow-link.desktop", &format!("x-scheme-handler/{SCHEME}")]).output();
        }
    }
    // macOS registers links from the app bundle's Info.plist (see the build workflow).
    #[cfg(target_os = "macos")]
    let _ = exe;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_links() {
        let md5 = "9403b89cc3d2a928ad431e625c2e6aad";
        assert_eq!(chart_id(&format!("rainbow://{md5}")).as_deref(), Some(md5));
        assert_eq!(chart_id("rainbow://9403B89CC3D2/").as_deref(), Some("9403b89cc3d2"));
        assert_eq!(chart_id("rainbow:9403b89cc3d2").as_deref(), Some("9403b89cc3d2"));
        assert_eq!(chart_id("rainbow://chart/9403b89cc3d2").as_deref(), Some("9403b89cc3d2"));
        assert_eq!(chart_id(&share_link(md5)).as_deref(), Some("9403b89cc3d2"));
        assert_eq!(chart_id(&format!("{WEB_APP}?c=9403b89cc3d2")).as_deref(), Some("9403b89cc3d2"));
        assert_eq!(chart_id("rainbow://9403"), None);
        assert_eq!(chart_id("rainbow://not-a-hash!"), None);
        assert_eq!(chart_id("https://example.com/?9403b89cc3d2"), None);
    }
}
