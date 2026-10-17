//! Two-way sync between daily-note to-dos and an iCloud calendar over CalDAV
//! (the same protocol the iPhone uses), plus read-only events from your other calendars.
use std::collections::{HashMap, HashSet};

use chrono::{Duration, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::ics::Event;

pub const ICLOUD: &str = "https://caldav.icloud.com";
pub const CAL_NAME: &str = "Penguin Notes";

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Creds {
    pub user: String,
    pub pass: String,
    #[serde(default)]
    pub enabled: bool,
    /// Override for tests / other CalDAV servers; empty = iCloud.
    #[serde(default)]
    pub server: String,
}

/// A to-do we have put on the server, remembered so edits and deletions can be matched up.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Link {
    pub uid: String,
    pub href: String,
    pub etag: String,
    pub date: NaiveDate,
    pub line: String,
}

/// An event from one of your other calendars, shown read-only.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Outside {
    pub date: NaiveDate,
    pub time: String,
    pub text: String,
    pub calendar: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct State {
    pub cal_href: String,
    pub links: Vec<Link>,
    pub outside: Vec<Outside>,
    #[serde(default)]
    pub last_ok: i64,
}

#[derive(Debug, PartialEq, Clone)]
pub enum LocalOp {
    Add { date: NaiveDate, line: String },
    Remove { date: NaiveDate, line: String },
}

#[derive(Debug, PartialEq)]
pub enum RemoteOp {
    /// `if_match`: None = create.
    Put {
        ev: Event,
        href: String,
        if_match: Option<String>,
        line: String,
    },
    Delete {
        href: String,
        etag: String,
    },
}

pub struct Remote {
    pub href: String,
    pub etag: String,
    pub ev: Event,
}

fn key(date: NaiveDate, line: &str) -> Option<(NaiveDate, String, String)> {
    Event::from_line(date, line).map(|e| (e.date, e.time, e.text))
}

fn new_uid() -> String {
    format!("penguin-{}@penguin-notes", crate::model::new_id())
}

/// Decide what to change on each side. `items` are (date, line) of every daily-note to-do.
pub fn plan(
    items: &[(NaiveDate, String)],
    links: &[Link],
    remote: &[Remote],
    cal_href: &str,
) -> (Vec<RemoteOp>, Vec<LocalOp>, Vec<Link>) {
    let mut used = vec![false; items.len()];
    let by_uid: HashMap<&str, &Remote> = remote.iter().map(|r| (r.ev.uid.as_str(), r)).collect();
    let known: HashSet<&str> = links.iter().map(|l| l.uid.as_str()).collect();
    let (mut rops, mut lops, mut keep) = (vec![], vec![], vec![]);
    for link in links {
        let lk = key(link.date, &link.line);
        let li = (0..items.len()).find(|&i| !used[i] && key(items[i].0, &items[i].1) == lk);
        match (li, by_uid.get(link.uid.as_str())) {
            (Some(i), Some(r)) => {
                used[i] = true;
                let item = &items[i];
                let local_done = Event::from_line(item.0, &item.1).is_some_and(|e| e.done);
                if r.etag != link.etag {
                    // Changed on the phone: the phone wins.
                    if r.ev.line() != item.1 || r.ev.date != item.0 {
                        lops.push(LocalOp::Remove {
                            date: item.0,
                            line: item.1.clone(),
                        });
                        lops.push(LocalOp::Add {
                            date: r.ev.date,
                            line: r.ev.line(),
                        });
                    }
                    keep.push(Link {
                        uid: link.uid.clone(),
                        href: r.href.clone(),
                        etag: r.etag.clone(),
                        date: r.ev.date,
                        line: r.ev.line(),
                    });
                } else if local_done != r.ev.done {
                    // Ticked or unticked here.
                    let ev = Event {
                        uid: link.uid.clone(),
                        done: local_done,
                        ..r.ev.clone()
                    };
                    rops.push(RemoteOp::Put {
                        line: ev.line(),
                        ev,
                        href: r.href.clone(),
                        if_match: Some(r.etag.clone()),
                    });
                } else {
                    keep.push(link.clone());
                }
            }
            (Some(i), None) => {
                // Deleted on the phone.
                used[i] = true;
                lops.push(LocalOp::Remove {
                    date: items[i].0,
                    line: items[i].1.clone(),
                });
            }
            (None, Some(r)) => {
                if r.etag != link.etag {
                    // Deleted or edited here, but edited on the phone too: keep the phone's version.
                    lops.push(LocalOp::Add {
                        date: r.ev.date,
                        line: r.ev.line(),
                    });
                    keep.push(Link {
                        uid: link.uid.clone(),
                        href: r.href.clone(),
                        etag: r.etag.clone(),
                        date: r.ev.date,
                        line: r.ev.line(),
                    });
                } else {
                    rops.push(RemoteOp::Delete {
                        href: r.href.clone(),
                        etag: r.etag.clone(),
                    });
                }
            }
            (None, None) => {}
        }
    }
    for (i, (date, line)) in items.iter().enumerate() {
        if used[i] {
            continue;
        }
        if let Some(mut ev) = Event::from_line(*date, line) {
            ev.uid = new_uid();
            let href = format!("{}{}.ics", cal_href, ev.uid.replace('@', "-"));
            rops.push(RemoteOp::Put {
                line: ev.line(),
                ev,
                href,
                if_match: None,
            });
        }
    }
    for r in remote {
        if !known.contains(r.ev.uid.as_str()) {
            // Added on the phone.
            lops.push(LocalOp::Add {
                date: r.ev.date,
                line: r.ev.line(),
            });
            keep.push(Link {
                uid: r.ev.uid.clone(),
                href: r.href.clone(),
                etag: r.etag.clone(),
                date: r.ev.date,
                line: r.ev.line(),
            });
        }
    }
    (rops, lops, keep)
}

// ---------------------------------------------------------------- CalDAV client

fn b64(s: &str) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let b = s.as_bytes();
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            out.push(if k <= c.len() {
                T[(n >> (18 - 6 * k) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

struct Dav {
    agent: ureq::Agent,
    auth: String,
}

struct Resp {
    status: u16,
    etag: String,
    body: String,
}

fn origin(url: &str) -> &str {
    let after = url.find("://").map_or(0, |i| i + 3);
    url[after..].find('/').map_or(url, |i| &url[..after + i])
}

fn join(base: &str, href: &str) -> String {
    if href.starts_with("http") {
        href.to_string()
    } else {
        format!("{}{}", origin(base), href)
    }
}

impl Dav {
    fn new(c: &Creds) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .allow_non_standard_methods(true)
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .build()
            .into();
        Self {
            agent,
            auth: format!(
                "Basic {}",
                b64(&format!("{}:{}", c.user.trim(), c.pass.trim()))
            ),
        }
    }

    fn req(
        &self,
        method: &str,
        url: &str,
        depth: Option<&str>,
        body: Option<String>,
        headers: &[(&str, &str)],
    ) -> Result<Resp, String> {
        let mut b = ureq::http::Request::builder()
            .method(method)
            .uri(url)
            .header("Authorization", &self.auth);
        if let Some(d) = depth {
            b = b.header("Depth", d);
        }
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        let ct = if method == "PUT" {
            "text/calendar; charset=utf-8"
        } else {
            "application/xml; charset=utf-8"
        };
        let req = b
            .header("Content-Type", ct)
            .body(body.unwrap_or_default())
            .map_err(|e| e.to_string())?;
        let mut r = self
            .agent
            .run(req)
            .map_err(|e| format!("can't reach the calendar server: {e}"))?;
        let status = r.status().as_u16();
        if status == 401 {
            return Err("Apple ID or app-specific password was rejected".into());
        }
        let etag = r
            .headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = r.body_mut().read_to_string().unwrap_or_default();
        if status >= 400 && status != 404 && status != 412 {
            return Err(format!("{method} failed: HTTP {status}"));
        }
        Ok(Resp { status, etag, body })
    }

    fn propfind(&self, url: &str, depth: &str, props: &str) -> Result<String, String> {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/"><d:prop>{props}</d:prop></d:propfind>"#
        );
        Ok(self
            .req("PROPFIND", url, Some(depth), Some(body), &[])?
            .body)
    }
}

/// Each `<response>`: (href, {prop local name -> node text / child names}).
struct Item {
    href: String,
    text: HashMap<String, String>,
    kinds: Vec<String>,
}

fn responses(xml: &str) -> Vec<Item> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return vec![];
    };
    doc.descendants()
        .filter(|n| n.tag_name().name() == "response")
        .map(|r| {
            let href = r
                .descendants()
                .find(|n| n.tag_name().name() == "href")
                .and_then(|n| n.text())
                .unwrap_or("")
                .trim()
                .to_string();
            let mut text = HashMap::new();
            let mut kinds = vec![];
            for prop in r.descendants().filter(|n| n.tag_name().name() == "prop") {
                for p in prop.children().filter(|n| n.is_element()) {
                    let name = p.tag_name().name().to_string();
                    let inner = p
                        .descendants()
                        .find(|n| n.tag_name().name() == "href")
                        .and_then(|n| n.text())
                        .or_else(|| p.text())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if name == "resourcetype" || name == "supported-calendar-component-set" {
                        for c in p.children().filter(|n| n.is_element()) {
                            kinds.push(
                                c.attribute("name")
                                    .unwrap_or(c.tag_name().name())
                                    .to_string(),
                            );
                        }
                    }
                    text.insert(name, inner);
                }
            }
            Item { href, text, kinds }
        })
        .collect()
}

struct Cal {
    href: String,
    name: String,
}

fn calendars(dav: &Dav, server: &str) -> Result<Vec<Cal>, String> {
    let root = format!("{server}/");
    let x = dav.propfind(&root, "0", "<d:current-user-principal/>")?;
    let principal = responses(&x).into_iter().find_map(|i| {
        i.text
            .get("current-user-principal")
            .cloned()
            .filter(|s| !s.is_empty())
    });
    let principal = join(
        &root,
        &principal.ok_or("couldn't find your calendar account (check the Apple ID)")?,
    );
    let x = dav.propfind(&principal, "0", "<c:calendar-home-set/>")?;
    let home = responses(&x).into_iter().find_map(|i| {
        i.text
            .get("calendar-home-set")
            .cloned()
            .filter(|s| !s.is_empty())
    });
    let home = join(&principal, &home.ok_or("couldn't find your calendars")?);
    let x = dav.propfind(
        &home,
        "1",
        "<d:displayname/><d:resourcetype/><c:supported-calendar-component-set/>",
    )?;
    Ok(responses(&x)
        .into_iter()
        .filter(|i| {
            i.kinds.iter().any(|k| k == "calendar")
                && (i.kinds.iter().any(|k| k == "VEVENT") || !i.kinds.iter().any(|k| k == "VTODO"))
        })
        .map(|i| Cal {
            href: join(&home, &i.href),
            name: i.text.get("displayname").cloned().unwrap_or_default(),
        })
        .chain(std::iter::once(Cal {
            href: home.clone(),
            name: "\u{0}home".into(),
        }))
        .collect())
}

fn ensure_calendar(dav: &Dav, cals: &[Cal]) -> Result<String, String> {
    if let Some(c) = cals.iter().find(|c| c.name == CAL_NAME) {
        return Ok(c.href.clone());
    }
    let home = &cals
        .iter()
        .find(|c| c.name == "\u{0}home")
        .ok_or("no calendar home")?
        .href;
    let href = format!(
        "{}{}/",
        if home.ends_with('/') {
            home.clone()
        } else {
            format!("{home}/")
        },
        crate::model::new_id()
    );
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><c:mkcalendar xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:a="http://apple.com/ns/ical/"><d:set><d:prop><d:displayname>{CAL_NAME}</d:displayname><a:calendar-color>#D94D2AFF</a:calendar-color><c:supported-calendar-component-set><c:comp name="VEVENT"/></c:supported-calendar-component-set></d:prop></d:set></c:mkcalendar>"#
    );
    let r = dav.req("MKCALENDAR", &href, None, Some(body), &[])?;
    if r.status >= 300 {
        return Err(format!(
            "couldn't create the “{CAL_NAME}” calendar (HTTP {})",
            r.status
        ));
    }
    Ok(href)
}

fn events(
    dav: &Dav,
    cal: &str,
    range: Option<(NaiveDate, NaiveDate)>,
) -> Result<Vec<Remote>, String> {
    let (tr, data) = match range {
        Some((a, b)) => {
            let (s, e) = (a.format("%Y%m%dT000000Z"), b.format("%Y%m%dT000000Z"));
            (
                format!(r#"<c:time-range start="{s}" end="{e}"/>"#),
                format!(r#"<c:calendar-data><c:expand start="{s}" end="{e}"/></c:calendar-data>"#),
            )
        }
        None => (String::new(), "<c:calendar-data/>".to_string()),
    };
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/>{data}</d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT">{tr}</c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#
    );
    let x = dav.req("REPORT", cal, Some("1"), Some(body), &[])?.body;
    Ok(responses(&x)
        .into_iter()
        .flat_map(|i| {
            let href = join(cal, &i.href);
            let etag = i.text.get("getetag").cloned().unwrap_or_default();
            crate::ics::parse(i.text.get("calendar-data").map_or("", |s| s.as_str()))
                .into_iter()
                .map(move |ev| Remote {
                    href: href.clone(),
                    etag: etag.clone(),
                    ev,
                })
        })
        .collect())
}

pub struct Outcome {
    pub ops: Vec<LocalOp>,
    pub state: State,
}

/// One full sync. Runs on a worker thread.
pub fn sync(
    creds: &Creds,
    items: &[(NaiveDate, String)],
    mut state: State,
) -> Result<Outcome, String> {
    if creds.user.trim().is_empty() || creds.pass.trim().is_empty() {
        return Err("enter your Apple ID and an app-specific password".into());
    }
    let server = if creds.server.is_empty() {
        ICLOUD
    } else {
        creds.server.trim_end_matches('/')
    };
    let dav = Dav::new(creds);
    let cals = calendars(&dav, server)?;
    let cal = ensure_calendar(&dav, &cals)?;
    if state.cal_href != cal {
        // A different (or brand-new) calendar: start matching from scratch.
        state.links.clear();
        state.cal_href = cal.clone();
    }
    let remote = events(&dav, &cal, None)?;
    let (rops, lops, mut links) = plan(items, &state.links, &remote, &cal);
    for op in rops {
        match op {
            RemoteOp::Put {
                ev,
                href,
                if_match,
                line,
            } => {
                let m = if_match.clone().unwrap_or_default();
                let hdr: Vec<(&str, &str)> = match &if_match {
                    Some(_) => vec![("If-Match", m.as_str())],
                    None => vec![("If-None-Match", "*")],
                };
                let r = dav.req("PUT", &href, None, Some(ev.to_ics()), &hdr)?;
                if r.status < 300 {
                    links.push(Link {
                        uid: ev.uid.clone(),
                        href,
                        etag: r.etag,
                        date: ev.date,
                        line,
                    });
                } else if if_match.is_some() {
                    // Changed on the phone meanwhile: a blank etag makes the next sync take the phone's version.
                    links.push(Link {
                        uid: ev.uid.clone(),
                        href,
                        etag: String::new(),
                        date: ev.date,
                        line,
                    });
                }
            }
            RemoteOp::Delete { href, etag } => {
                dav.req("DELETE", &href, None, None, &[("If-Match", etag.as_str())])?;
            }
        }
    }
    // Your other calendars, shown read-only.
    let today = Local::now().date_naive();
    let range = (today - Duration::days(62), today + Duration::days(400));
    let mut outside = vec![];
    for c in cals
        .iter()
        .filter(|c| c.href != cal && !c.name.starts_with('\u{0}'))
    {
        if let Ok(evs) = events(&dav, &c.href, Some(range)) {
            outside.extend(evs.into_iter().map(|r| Outside {
                date: r.ev.date,
                time: r.ev.time,
                text: r.ev.text,
                calendar: c.name.clone(),
            }));
        }
    }
    outside.sort_by(|a, b| (a.date, &a.time).cmp(&(b.date, &b.time)));
    state.links = links;
    state.outside = outside;
    state.last_ok = crate::model::now_ms();
    Ok(Outcome { ops: lops, state })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::sync::{Arc, Mutex};

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn base64() {
        assert_eq!(
            b64("me@icloud.com:abcd-efgh"),
            "bWVAaWNsb3VkLmNvbTphYmNkLWVmZ2g="
        );
        assert_eq!(b64("ab"), "YWI=");
    }

    /// In-memory CalDAV server shaped like iCloud: principal -> home on another path -> calendars.
    #[derive(Default)]
    pub struct Fake {
        pub cals: Vec<(String, String)>,
        /// href -> (etag, ics)
        pub objects: HashMap<String, (String, String)>,
        pub n: u32,
    }

    pub fn serve(fake: Arc<Mutex<Fake>>) -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { break };
                let fake = fake.clone();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(s.try_clone().unwrap());
                    let mut first = String::new();
                    r.read_line(&mut first).unwrap();
                    let mut parts = first.split_whitespace();
                    let (method, path) = (
                        parts.next().unwrap_or("").to_string(),
                        parts.next().unwrap_or("").to_string(),
                    );
                    let (mut len, mut if_match) = (0, String::new());
                    loop {
                        let mut line = String::new();
                        r.read_line(&mut line).unwrap();
                        let low = line.to_lowercase();
                        if let Some(v) = low.strip_prefix("content-length:") {
                            len = v.trim().parse().unwrap();
                        }
                        if low.starts_with("if-match:") {
                            if_match = line[9..].trim().to_string();
                        }
                        if line == "\r\n" || line.is_empty() {
                            break;
                        }
                    }
                    let mut body = vec![0; len];
                    r.read_exact(&mut body).unwrap();
                    let body = String::from_utf8_lossy(&body).to_string();
                    let (status, etag, out) = fake
                        .lock()
                        .unwrap()
                        .handle(&method, &path, &body, &if_match);
                    let resp = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/xml\r\nETag: {etag}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}",
                        out.len()
                    );
                    let _ = s.write_all(resp.as_bytes());
                });
            }
        });
        url
    }

    fn ms(inner: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">{inner}</d:multistatus>"#
        )
    }

    impl Fake {
        pub fn put_phone(&mut self, cal: &str, uid: &str, ics_body: &str) {
            self.n += 1;
            self.objects.insert(
                format!("{cal}{uid}.ics"),
                (format!("\"p{}\"", self.n), ics_body.to_string()),
            );
        }

        fn handle(
            &mut self,
            method: &str,
            path: &str,
            body: &str,
            if_match: &str,
        ) -> (u16, String, String) {
            match (method, path) {
                ("PROPFIND", "/") => (
                    207,
                    String::new(),
                    ms(
                        "<d:response><d:href>/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/123/principal/</d:href></d:current-user-principal></d:prop></d:propstat></d:response>",
                    ),
                ),
                ("PROPFIND", "/123/principal/") => (
                    207,
                    String::new(),
                    ms(
                        "<d:response><d:href>/123/principal/</d:href><d:propstat><d:prop><c:calendar-home-set><d:href>/123/calendars/</d:href></c:calendar-home-set></d:prop></d:propstat></d:response>",
                    ),
                ),
                ("PROPFIND", "/123/calendars/") => {
                    let mut x = String::from(
                        "<d:response><d:href>/123/calendars/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>",
                    );
                    for (href, name) in &self.cals {
                        x += &format!(
                            r#"<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:displayname>{name}</d:displayname><d:resourcetype><d:collection/><c:calendar/></d:resourcetype><c:supported-calendar-component-set><c:comp name="VEVENT"/></c:supported-calendar-component-set></d:prop></d:propstat></d:response>"#
                        );
                    }
                    (207, String::new(), ms(&x))
                }
                ("MKCALENDAR", p) => {
                    let name = body
                        .split("<d:displayname>")
                        .nth(1)
                        .and_then(|s| s.split('<').next())
                        .unwrap_or("")
                        .to_string();
                    self.cals.push((p.to_string(), name));
                    (201, String::new(), String::new())
                }
                ("REPORT", cal) => {
                    let mut x = String::new();
                    for (href, (etag, ics)) in
                        self.objects.iter().filter(|(h, _)| h.starts_with(cal))
                    {
                        let ics = ics.replace('&', "&amp;").replace('<', "&lt;");
                        x += &format!(
                            "<d:response><d:href>{href}</d:href><d:propstat><d:prop><d:getetag>{etag}</d:getetag><c:calendar-data>{ics}</c:calendar-data></d:prop></d:propstat></d:response>"
                        );
                    }
                    (207, String::new(), ms(&x))
                }
                ("PUT", p) => {
                    if !if_match.is_empty() && self.objects.get(p).is_none_or(|o| o.0 != if_match) {
                        return (412, String::new(), String::new());
                    }
                    self.n += 1;
                    let etag = format!("\"e{}\"", self.n);
                    self.objects
                        .insert(p.to_string(), (etag.clone(), body.to_string()));
                    (201, etag, String::new())
                }
                ("DELETE", p) => {
                    self.objects.remove(p);
                    (204, String::new(), String::new())
                }
                _ => (404, String::new(), String::new()),
            }
        }
    }

    pub fn creds(server: String) -> Creds {
        Creds {
            user: "me@icloud.com".into(),
            pass: "abcd-efgh-ijkl-mnop".into(),
            enabled: true,
            server,
        }
    }

    #[test]
    fn two_way_round_trip() {
        let fake = Arc::new(Mutex::new(Fake {
            cals: vec![("/123/calendars/home/".into(), "Home".into())],
            ..Default::default()
        }));
        let c = creds(serve(fake.clone()));
        let mut items = vec![(d("2026-10-20"), "- [ ] 15:00 Dentist".to_string())];

        // 1. First sync creates the calendar and pushes the to-do.
        let out = sync(&c, &items, State::default()).unwrap();
        assert!(out.ops.is_empty());
        let cal = out.state.cal_href.clone();
        assert!(fake.lock().unwrap().cals.iter().any(|(_, n)| n == CAL_NAME));
        assert_eq!(fake.lock().unwrap().objects.len(), 1);

        // 2. Nothing changed: nothing happens.
        let out = sync(&c, &items, out.state).unwrap();
        assert!(out.ops.is_empty());

        // 3. Tick it here: the event is updated on the phone.
        items[0].1 = "- [x] 15:00 Dentist".into();
        let out = sync(&c, &items, out.state).unwrap();
        assert!(out.ops.is_empty());
        assert!(
            fake.lock()
                .unwrap()
                .objects
                .values()
                .any(|(_, ics)| ics.contains("SUMMARY:✓ Dentist"))
        );

        // 4. Add an event on the phone (Penguin Notes calendar) and one on Home.
        let phone = Event {
            uid: "phone-1".into(),
            date: d("2026-10-22"),
            time: "09:30".into(),
            text: "Gym".into(),
            done: false,
        };
        fake.lock()
            .unwrap()
            .put_phone(&cal.replace(&c.server, ""), "phone-1", &phone.to_ics());
        let today = Local::now().date_naive();
        let home_ev = Event {
            uid: "h1".into(),
            date: today,
            time: String::new(),
            text: "Mom's birthday".into(),
            done: false,
        };
        fake.lock()
            .unwrap()
            .put_phone("/123/calendars/home/", "h1", &home_ev.to_ics());
        let out = sync(&c, &items, out.state).unwrap();
        assert_eq!(
            out.ops,
            vec![LocalOp::Add {
                date: d("2026-10-22"),
                line: "- [ ] 09:30 Gym".into()
            }]
        );
        assert!(
            out.state
                .outside
                .iter()
                .any(|o| o.text == "Mom's birthday" && o.calendar == "Home")
        );
        items.push((d("2026-10-22"), "- [ ] 09:30 Gym".into()));

        // 5. Delete the dentist here: deleted on the phone. Delete gym on the phone: removed here.
        items.remove(0);
        let gym_href = fake
            .lock()
            .unwrap()
            .objects
            .keys()
            .find(|k| k.contains("phone-1"))
            .unwrap()
            .clone();
        fake.lock().unwrap().objects.remove(&gym_href);
        let out = sync(&c, &items, out.state).unwrap();
        assert_eq!(
            out.ops,
            vec![LocalOp::Remove {
                date: d("2026-10-22"),
                line: "- [ ] 09:30 Gym".into()
            }]
        );
        assert!(
            fake.lock()
                .unwrap()
                .objects
                .keys()
                .all(|k| k.contains("home/"))
        );
        assert!(out.state.links.is_empty());
    }

    #[test]
    fn phone_edit_moves_the_line() {
        let r = Remote {
            href: "/c/x.ics".into(),
            etag: "\"2\"".into(),
            ev: Event {
                uid: "u".into(),
                date: d("2026-10-21"),
                time: "10:00".into(),
                text: "Dentist".into(),
                done: false,
            },
        };
        let link = Link {
            uid: "u".into(),
            href: "/c/x.ics".into(),
            etag: "\"1\"".into(),
            date: d("2026-10-20"),
            line: "- [ ] 15:00 Dentist".into(),
        };
        let items = vec![(d("2026-10-20"), "- [ ] 15:00 Dentist".to_string())];
        let (rops, lops, keep) = plan(&items, &[link], &[r], "/c/");
        assert!(rops.is_empty());
        assert_eq!(
            lops,
            vec![
                LocalOp::Remove {
                    date: d("2026-10-20"),
                    line: "- [ ] 15:00 Dentist".into()
                },
                LocalOp::Add {
                    date: d("2026-10-21"),
                    line: "- [ ] 10:00 Dentist".into()
                },
            ]
        );
        assert_eq!(keep[0].etag, "\"2\"");
    }

    #[test]
    fn wrong_password_is_reported() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        });
        let err = sync(&creds(url), &[], State::default()).err().unwrap();
        assert!(err.contains("rejected"), "{err}");
    }
}
