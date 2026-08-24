//! The periodic update checker: parses Dynon's own download pages, decides
//! which package is current, and downloads it — never installs it to a
//! drive. That last step stays a human's decision, made on the prepare
//! page, exactly like a folder-scanned `.duc` would be.
//!
//! Split deliberately into a pure half (`parse_dynon_page`, `select`) that
//! never touches the network — tested against saved fixtures in
//! `tests/data/` — and a network half (`fetch_page`, `download_package`)
//! that only real runs exercise.

use crate::scan::Cycle;
use std::fmt;
use std::path::{Path, PathBuf};

const USER_AGENT: &str = concat!(
    "DynonUSBUpdater/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/yfilali/dynon-usb-updater)"
);

pub const EXPERIMENTAL_PAGE_URL: &str = "https://dynonavionics.com/us-aviation-obstacle-data.php";
pub const CERTIFIED_PAGE_URL: &str = "https://www.dynoncertified.com/us-aviation-obstacle-data.php";

/// Which page to check, decided by `system-type` — never inferred from
/// anything else.
pub fn page_url(system_type: &str) -> &'static str {
    if system_type == "certified" {
        CERTIFIED_PAGE_URL
    } else {
        EXPERIMENTAL_PAGE_URL
    }
}

// ---------------------------------------------------------------------------
// Pure parsing
// ---------------------------------------------------------------------------

/// A plain calendar date, good enough for comparing AIRAC validity windows —
/// deliberately not a general-purpose date type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SimpleDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl fmt::Display for SimpleDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// A validity window as printed on the page ("Valid: August 6 - September
/// 2"), plus the parsed bounds used to decide whether it covers today. The
/// page never prints a year, so the year is inferred from the cycle number
/// beside it (`Cycle: 2608` implies 2026); a range whose end month is
/// earlier than its start month is assumed to cross into the next year.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validity {
    pub start: SimpleDate,
    pub end: SimpleDate,
    pub text: String,
}

impl Validity {
    pub fn contains(&self, day: SimpleDate) -> bool {
        day >= self.start && day <= self.end
    }
}

/// One download link on a provider page, with whatever validity data was
/// found next to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// The `href` exactly as printed — absolute on one page, relative on
    /// the other. Resolve with `resolve_url` against the page's own URL.
    pub href: String,
    pub aviation_cycle: Option<Cycle>,
    pub obstacle_cycle: Option<Cycle>,
    pub aviation_valid: Option<Validity>,
    pub obstacle_valid: Option<Validity>,
}

impl Listing {
    pub fn filename(&self) -> &str {
        self.href.rsplit('/').next().unwrap_or(&self.href)
    }
}

fn href_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r#"href="([^"]+\.duc)""#).unwrap())
}

fn valid_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    // "Valid: August 6 - September 2 (Cycle: 2608)"
    RE.get_or_init(|| regex::Regex::new(r"Valid:\s*([^(<]+?)\s*\(Cycle:\s*(\d{3,4})\)").unwrap())
}

/// Strip `<!-- ... -->` so a commented-out block (Dynon leaves last cycle's
/// "Upcoming Data" table commented out rather than deleting it) is never
/// mistaken for a live listing.
fn strip_html_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        rest = match rest[start..].find("-->") {
            Some(end) => &rest[start + end + 3..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

fn cycle_from_digits(s: &str) -> Option<Cycle> {
    if s.len() != 4 {
        return None;
    }
    let year: u16 = s[0..2].parse().ok()?;
    let number: u8 = s[2..4].parse().ok()?;
    if !(1..=13).contains(&number) {
        return None;
    }
    Some(Cycle {
        year: 2000 + year,
        number,
    })
}

fn month_number(name: &str) -> Option<u8> {
    Some(match name.to_ascii_lowercase().as_str() {
        "january" => 1,
        "february" => 2,
        "march" => 3,
        "april" => 4,
        "may" => 5,
        "june" => 6,
        "july" => 7,
        "august" => 8,
        "september" => 9,
        "october" => 10,
        "november" => 11,
        "december" => 12,
        _ => return None,
    })
}

fn parse_month_day(text: &str, year: i32) -> Option<SimpleDate> {
    let mut words = text.split_whitespace();
    let month = month_number(words.next()?)?;
    let day: u8 = words.next()?.trim_end_matches(',').parse().ok()?;
    Some(SimpleDate { year, month, day })
}

/// `text` is the bit between "Valid:" and "(Cycle: ...)", e.g.
/// "August 6 - September 2"; `cycle_digits` supplies the year.
fn parse_validity(text: &str, cycle_digits: &str) -> Option<Validity> {
    let year = 2000 + cycle_digits.get(0..2)?.parse::<i32>().ok()?;
    let (start_text, end_text) = text.split_once('-')?;
    let start = parse_month_day(start_text.trim(), year)?;
    let mut end = parse_month_day(end_text.trim(), year)?;
    if end.month < start.month {
        end.year += 1;
    }
    Some(Validity {
        start,
        end,
        text: text.to_string(),
    })
}

/// Extract every `.duc` download link on a Dynon aviation/obstacles page,
/// along with the validity text printed beside it. Never touches the
/// network — operates purely on already-fetched HTML, which is what makes
/// it testable against the fixtures in `tests/data/`.
///
/// Deliberately a light regex scan rather than a full HTML parser: both
/// real pages are simple, hand-authored markup (a `<table>` on one, Bootstrap
/// `<section>`s on the other), and the one structural fact this relies on —
/// the aviation validity line, then the obstacle validity line, then the
/// download link, in that order — holds on both.
pub fn parse_dynon_page(html: &str) -> Vec<Listing> {
    let cleaned = strip_html_comments(html);

    let hrefs: Vec<(usize, String)> = href_re()
        .captures_iter(&cleaned)
        .map(|c| (c.get(0).unwrap().start(), c[1].to_string()))
        .collect();
    let valids: Vec<(usize, String, String)> = valid_re()
        .captures_iter(&cleaned)
        .map(|c| {
            (
                c.get(0).unwrap().start(),
                c[1].trim().to_string(),
                c[2].to_string(),
            )
        })
        .collect();

    let mut listings = Vec::new();
    let mut block_start = 0usize;
    for (pos, href) in hrefs {
        let in_block: Vec<&(usize, String, String)> = valids
            .iter()
            .filter(|(p, _, _)| *p >= block_start && *p < pos)
            .collect();
        let aviation = in_block.first();
        let obstacle = in_block.get(1);
        listings.push(Listing {
            href,
            aviation_cycle: aviation.and_then(|(_, _, c)| cycle_from_digits(c)),
            obstacle_cycle: obstacle.and_then(|(_, _, c)| cycle_from_digits(c)),
            aviation_valid: aviation.and_then(|(_, t, c)| parse_validity(t, c)),
            obstacle_valid: obstacle.and_then(|(_, t, c)| parse_validity(t, c)),
        });
        block_start = pos;
    }
    listings
}

/// Which listing to treat as current, and — if none of several covers today
/// — the earliest one that will.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Current(Listing),
    /// Surfaced without ever being installed or even downloaded
    /// automatically: "available from {date}".
    UpcomingOnly(Listing),
}

/// Decide which listing is current as of `today`. A single listing (the
/// Experimental/LSA page normally offers exactly one, with its "Upcoming
/// Data" table commented out) is always treated as current — there is
/// nothing to choose between. With several listings (Dynon Certified lists
/// a current and an upcoming one separately), the one whose aviation
/// validity window actually covers `today` wins, regardless of which the
/// page itself labels "Current" — the page's own labels lag the calendar
/// right at a cycle boundary, which is exactly when this matters.
pub fn select(listings: &[Listing], today: SimpleDate) -> Option<Selection> {
    match listings {
        [] => None,
        [only] => Some(Selection::Current(only.clone())),
        many => {
            if let Some(current) = many
                .iter()
                .find(|l| l.aviation_valid.as_ref().is_some_and(|v| v.contains(today)))
            {
                return Some(Selection::Current(current.clone()));
            }
            many.iter()
                .filter_map(|l| l.aviation_valid.as_ref().map(|v| (v.start, l)))
                .filter(|(start, _)| *start > today)
                .min_by_key(|(start, _)| *start)
                .map(|(_, l)| Selection::UpcomingOnly(l.clone()))
        }
    }
}

/// Resolve a listing's `href` (absolute on one page, relative on the other)
/// against the page it came from.
pub fn resolve_url(page_url: &str, href: &str) -> Option<String> {
    gtk::glib::Uri::resolve_relative(Some(page_url), href, gtk::glib::UriFlags::NONE)
        .ok()
        .map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Network — never exercised by the test suite
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum FetchError {
    Network(String),
    Io(std::io::Error),
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Network(m) => write!(f, "{m}"),
            FetchError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for FetchError {}

pub struct PageFetch {
    /// `None` when the server answered 304 Not Modified — the caller already
    /// has the latest content and nothing further to parse.
    pub body: Option<String>,
    pub etag: Option<String>,
}

/// One conditional GET, identifying this app and its repo per the brief so
/// Dynon's server logs show a real contact point rather than an anonymous
/// bot. `cache_validator` is whatever was returned as `etag` last time —
/// sent back as `If-None-Match` (falling back to `If-Modified-Since` is not
/// needed: every `ETag` doubles as a Last-Modified-style token here, and if
/// the server has none the conditional header is simply omitted and a full
/// body comes back, which is still exactly one request).
pub async fn fetch_page(
    session: &soup::Session,
    url: &str,
    cache_validator: Option<&str>,
) -> Result<PageFetch, FetchError> {
    use soup::prelude::*;

    let message = soup::Message::new("GET", url).map_err(|e| FetchError::Network(e.to_string()))?;
    if let Some(headers) = message.request_headers() {
        headers.append("User-Agent", USER_AGENT);
        if let Some(validator) = cache_validator {
            headers.append("If-None-Match", validator);
            headers.append("If-Modified-Since", validator);
        }
    }

    let bytes = session
        .send_and_read_future(&message, soup::glib::Priority::DEFAULT)
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;

    if message.status() == soup::Status::NotModified {
        return Ok(PageFetch {
            body: None,
            etag: cache_validator.map(str::to_string),
        });
    }
    if message.status_code() >= 400 {
        return Err(FetchError::Network(format!(
            "server returned {}",
            message.status_code()
        )));
    }

    let etag = message
        .response_headers()
        .and_then(|h| h.one("ETag").or_else(|| h.one("Last-Modified")))
        .map(|g| g.to_string());
    let body = String::from_utf8_lossy(&bytes).into_owned();
    Ok(PageFetch {
        body: Some(body),
        etag,
    })
}

/// Download a package to `dest_dir`, atomically (`.part` + rename) — the
/// same safety property every other write in this app has. Returns the
/// final path. Never writes anywhere but `dest_dir`; the caller is
/// responsible for that being the download folder, never a drive.
pub async fn download_package(
    session: &soup::Session,
    url: &str,
    dest_dir: &Path,
) -> Result<PathBuf, FetchError> {
    use soup::prelude::*;

    let filename = url.rsplit('/').next().unwrap_or("download.duc").to_string();
    std::fs::create_dir_all(dest_dir).map_err(FetchError::Io)?;
    let dest = dest_dir.join(&filename);
    let part = dest.with_extension("part");

    let message = soup::Message::new("GET", url).map_err(|e| FetchError::Network(e.to_string()))?;
    if let Some(headers) = message.request_headers() {
        headers.append("User-Agent", USER_AGENT);
    }
    let bytes = session
        .send_and_read_future(&message, soup::glib::Priority::DEFAULT)
        .await
        .map_err(|e| FetchError::Network(e.to_string()))?;
    if message.status_code() >= 400 {
        return Err(FetchError::Network(format!(
            "server returned {}",
            message.status_code()
        )));
    }

    {
        use std::io::Write;
        let mut file = std::fs::File::create(&part).map_err(FetchError::Io)?;
        file.write_all(&bytes).map_err(FetchError::Io)?;
        file.sync_all().map_err(FetchError::Io)?;
    }
    std::fs::rename(&part, &dest).map_err(FetchError::Io)?;
    Ok(dest)
}
