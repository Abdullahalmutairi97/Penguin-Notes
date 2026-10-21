//! Two-way sync between notes and Apple Notes, through an IMAP mailbox.
//! Apple Notes keeps the notes of a mail account as messages in its "Notes" folder
//! (X-Uniform-Type-Identifier: com.apple.mail-note); an edit on the phone appends a new
//! message with the same X-Universally-Unique-Identifier and deletes the old one.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};

use crate::model::{self, Note};

pub const FOLDER: &str = "Notes";

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Creds {
    pub host: String,
    pub user: String,
    pub pass: String,
    #[serde(default)]
    pub enabled: bool,
}

/// A note we have put in the mailbox (or taken from it).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub uuid: String,
    pub uid: u32,
    /// Hash of what we rendered for the note then; a different hash now means it was edited here.
    pub hash: u64,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct State {
    pub validity: u32,
    /// Note id -> its message.
    pub notes: HashMap<String, Entry>,
    #[serde(default)]
    pub last_ok: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// Changed on the phone. `base` is the note's `updated` when the run started (None = new note).
    Put { id: String, title: String, body: String, base: Option<i64> },
    /// Deleted on the phone.
    Delete { id: String, base: i64 },
}

pub struct Outcome {
    pub ops: Vec<Op>,
    pub state: State,
}

// ---------------------------------------------------------------- note <-> message

/// FNV-1a: stable across builds, unlike DefaultHasher.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The note as Apple Notes HTML: the title line, then the Markdown as it is, a div per line,
/// so nothing is lost on the way back.
pub fn html(n: &Note) -> String {
    let mut lines: Vec<&str> = n.body.lines().collect();
    if lines.first().is_some_and(|l| l.trim() == format!("# {}", n.title.trim())) {
        lines.remove(0);
        if lines.first().is_some_and(|l| l.trim().is_empty()) {
            lines.remove(0);
        }
    }
    let mut out = String::from("<html><head></head><body>");
    let title = if n.title.trim().is_empty() { "Untitled" } else { n.title.trim() };
    out += &format!("<div>{}</div>", esc(title));
    for l in lines {
        let lead = l.len() - l.trim_start_matches([' ', '\t']).len();
        let pad = l[..lead].replace('\t', "    ").replace(' ', "&nbsp;");
        let rest = esc(&l[lead..]);
        if lead == 0 && rest.is_empty() {
            out += "<div><br></div>";
        } else {
            out += &format!("<div>{pad}{rest}</div>");
        }
    }
    out + "</body></html>"
}

pub fn hash(n: &Note) -> u64 {
    fnv(&html(n))
}

fn rfc2822(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().format("%a, %d %b %Y %H:%M:%S +0000").to_string()
}

fn wrap76(s: &str) -> String {
    s.as_bytes().chunks(76).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("\r\n")
}

pub fn message(n: &Note, uuid: &str) -> String {
    let title = if n.title.trim().is_empty() { "Untitled" } else { n.title.trim() };
    let date = rfc2822(n.updated);
    format!(
        "From: Penguin Notes <notes@penguin-notes>\r\n\
         Date: {date}\r\n\
         X-Uniform-Type-Identifier: com.apple.mail-note\r\n\
         X-Universally-Unique-Identifier: {uuid}\r\n\
         X-Mail-Created-Date: {date}\r\n\
         X-Penguin-Id: {}\r\n\
         Subject: =?utf-8?B?{}?=\r\n\
         Mime-Version: 1.0\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{}\r\n",
        n.id,
        B64.encode(title),
        wrap76(&B64.encode(html(n)))
    )
}

fn new_uuid(seed: &str) -> String {
    let a = fnv(&format!("{seed}{}", model::now_ms()));
    let b = fnv(&format!("{}{seed}{a}", std::process::id()));
    let h = format!("{a:016X}{b:016X}");
    format!("{}-{}-4{}-A{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32])
}

/// Headers, unfolded, names lower-cased; and the body.
fn split_msg(raw: &[u8]) -> (Vec<(String, String)>, Vec<u8>) {
    let text = raw;
    let (head, body) = match find(text, b"\r\n\r\n") {
        Some(i) => (&text[..i], text[i + 4..].to_vec()),
        None => match find(text, b"\n\n") {
            Some(i) => (&text[..i], text[i + 2..].to_vec()),
            None => (text, vec![]),
        },
    };
    let head = String::from_utf8_lossy(head).replace("\r\n", "\n");
    let mut hs: Vec<(String, String)> = vec![];
    for line in head.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = hs.last_mut() {
                last.1 += " ";
                last.1 += line.trim();
            }
        } else if let Some((k, v)) = line.split_once(':') {
            hs.push((k.trim().to_lowercase(), v.trim().to_string()));
        }
    }
    (hs, body)
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

fn header<'a>(hs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    hs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn param(v: &str, name: &str) -> Option<String> {
    v.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        (k.trim().eq_ignore_ascii_case(name)).then(|| v.trim().trim_matches('"').to_string())
    })
}

fn qp(s: &[u8]) -> Vec<u8> {
    let mut out = vec![];
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'=' {
            if s.get(i + 1) == Some(&b'\r') && s.get(i + 2) == Some(&b'\n') {
                i += 3;
                continue;
            }
            if s.get(i + 1) == Some(&b'\n') {
                i += 2;
                continue;
            }
            if let Some(b) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

fn decode_body(hs: &[(String, String)], body: &[u8]) -> Vec<u8> {
    match header(hs, "content-transfer-encoding").unwrap_or("").to_lowercase().as_str() {
        "base64" => {
            let clean: Vec<u8> = body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            B64.decode(clean).unwrap_or_default()
        }
        "quoted-printable" => qp(body),
        _ => body.to_vec(),
    }
}

/// (is_html, text) of the readable part, looking inside multipart messages.
fn content(raw: &[u8]) -> (bool, String) {
    let (hs, body) = split_msg(raw);
    let ct = header(&hs, "content-type").unwrap_or("text/plain").to_string();
    let lower = ct.to_lowercase();
    if lower.starts_with("multipart/") {
        let Some(b) = param(&ct, "boundary") else {
            return (false, String::new());
        };
        let sep = format!("--{b}");
        let text = String::from_utf8_lossy(&body).to_string();
        let parts: Vec<(bool, String)> = text.split(sep.as_str()).skip(1).filter(|p| !p.starts_with("--")).map(|p| content(p.trim_start_matches(['\r', '\n']).as_bytes())).collect();
        return parts.iter().find(|(h, _)| *h).or(parts.first()).cloned().unwrap_or((false, String::new()));
    }
    let data = String::from_utf8_lossy(&decode_body(&hs, &body)).to_string();
    (lower.starts_with("text/html"), data)
}

fn entities(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out += &rest[..i];
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
            n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out + rest
}

/// Apple Notes HTML -> plain lines.
pub fn html_text(h: &str) -> String {
    let mut s = h.replace(['\r', '\n'], "");
    for empty in ["<div><br></div>", "<div><br/></div>", "<div><br /></div>"] {
        s = s.replace(empty, "<div></div>");
    }
    let mut out = String::new();
    let mut rest = s.as_str();
    let mut in_ol: Vec<usize> = vec![];
    while let Some(i) = rest.find('<') {
        out += &rest[..i];
        let Some(end) = rest[i..].find('>') else {
            rest = &rest[i..i];
            break;
        };
        let tag = rest[i + 1..i + end].trim().to_lowercase();
        let name = tag.trim_start_matches('/').split([' ', '/']).next().unwrap_or("");
        let closing = tag.starts_with('/');
        match (name, closing) {
            ("br", _) => out.push('\n'),
            ("div" | "p" | "h1" | "h2" | "h3" | "li" | "tr", true) => {
                if !out.ends_with('\n') || name == "div" || name == "p" {
                    out.push('\n');
                }
            }
            ("ol", false) => in_ol.push(0),
            ("ol", true) => {
                in_ol.pop();
            }
            ("li", false) => match in_ol.last_mut() {
                Some(k) => {
                    *k += 1;
                    out += &format!("{k}. ");
                }
                None => out += "- ",
            },
            ("head" | "style" | "script", false) => {
                let close = format!("</{name}>");
                let after = &rest[i + end + 1..];
                rest = after.to_lowercase().find(&close).map(|j| &after[j + close.len()..]).unwrap_or("");
                continue;
            }
            _ => {}
        }
        rest = &rest[i + end + 1..];
    }
    out += rest;
    entities(&out).replace('\u{a0}', " ").trim_end().to_string()
}

/// (title, body) from a message as stored on the server.
pub fn parse(raw: &[u8]) -> (String, String) {
    let (is_html, text) = content(raw);
    let text = if is_html { html_text(&text) } else { text.replace("\r\n", "\n").trim_end().to_string() };
    let mut lines = text.lines();
    let title = lines.next().unwrap_or("").trim().to_string();
    let body = lines.collect::<Vec<_>>().join("\n");
    let title = if title.is_empty() { "Untitled".to_string() } else { title };
    (title, body.trim_start_matches('\n').to_string())
}

// ---------------------------------------------------------------- IMAP client

struct Resp {
    text: String,
    lits: Vec<Vec<u8>>,
}

type Tls = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

struct Imap {
    r: BufReader<Box<dyn ReadWrite>>,
    n: u32,
}

trait ReadWrite: Read + Write + Send {}
impl<T: Read + Write + Send> ReadWrite for T {}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Imap {
    fn connect(host: &str) -> Result<Self, String> {
        let (name, port) = match host.rsplit_once(':') {
            Some((h, p)) if p.parse::<u16>().is_ok() => (h.to_string(), p.parse().unwrap()),
            _ => (host.to_string(), 993),
        };
        let tcp = std::net::ToSocketAddrs::to_socket_addrs(&(name.as_str(), port))
            .map_err(|_| format!("can't find {name} (is Tailscale on?)"))?
            .find_map(|a| TcpStream::connect_timeout(&a, Duration::from_secs(10)).ok())
            .ok_or_else(|| format!("can't reach {name} (is Tailscale on?)"))?;
        tcp.set_read_timeout(Some(Duration::from_secs(30))).ok();
        tcp.set_write_timeout(Some(Duration::from_secs(30))).ok();
        let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let sn = rustls::pki_types::ServerName::try_from(name.clone()).map_err(|_| format!("bad server name {name}"))?;
        let conn = rustls::ClientConnection::new(Arc::new(cfg), sn).map_err(|e| e.to_string())?;
        let tls: Tls = rustls::StreamOwned::new(conn, tcp);
        let mut me = Imap { r: BufReader::new(Box::new(tls)), n: 0 };
        let hello = me.read().map_err(|e| format!("secure connection to {name} failed: {e}"))?;
        if !hello.text.starts_with("* OK") {
            return Err(format!("unexpected greeting: {}", hello.text));
        }
        Ok(me)
    }

    fn line(&mut self) -> Result<String, String> {
        let mut buf = vec![];
        self.r.read_until(b'\n', &mut buf).map_err(|e| e.to_string())?;
        if buf.is_empty() {
            return Err("connection closed".into());
        }
        Ok(String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string())
    }

    /// One response line, with any {n} literals read in.
    fn read(&mut self) -> Result<Resp, String> {
        let mut text = String::new();
        let mut lits = vec![];
        loop {
            let l = self.line()?;
            text += &l;
            let Some(n) = l.strip_suffix('}').and_then(|s| s.rsplit_once('{')).and_then(|(_, n)| n.parse::<usize>().ok()) else {
                return Ok(Resp { text, lits });
            };
            let mut lit = vec![0; n];
            self.r.read_exact(&mut lit).map_err(|e| e.to_string())?;
            lits.push(lit);
        }
    }

    fn send(&mut self, data: &[u8]) -> Result<(), String> {
        let w = self.r.get_mut();
        w.write_all(data).and_then(|_| w.flush()).map_err(|e| e.to_string())
    }

    /// Runs a command; returns the untagged responses and the tagged OK line.
    fn cmd(&mut self, c: &str) -> Result<(Vec<Resp>, String), String> {
        self.n += 1;
        let tag = format!("a{}", self.n);
        self.send(format!("{tag} {c}\r\n").as_bytes())?;
        self.finish(&tag, c)
    }

    fn finish(&mut self, tag: &str, c: &str) -> Result<(Vec<Resp>, String), String> {
        let mut got = vec![];
        loop {
            let r = self.read()?;
            if let Some(rest) = r.text.strip_prefix(&format!("{tag} ")) {
                if rest.starts_with("OK") {
                    return Ok((got, rest.to_string()));
                }
                let what = c.split(' ').next().unwrap_or(c);
                return Err(format!("{what} failed: {rest}"));
            }
            got.push(r);
        }
    }

    fn append(&mut self, folder: &str, msg: &str) -> Result<Option<u32>, String> {
        self.n += 1;
        let tag = format!("a{}", self.n);
        self.send(format!("{tag} APPEND {} (\\Seen) {{{}}}\r\n", quote(folder), msg.len()).as_bytes())?;
        let go = self.line()?;
        if !go.starts_with('+') {
            return Err(format!("APPEND refused: {go}"));
        }
        self.send(msg.as_bytes())?;
        self.send(b"\r\n")?;
        let (_, ok) = self.finish(&tag, "APPEND")?;
        // OK [APPENDUID <validity> <uid>]
        Ok(ok.split_once("APPENDUID ").and_then(|(_, r)| r.split([' ', ']']).nth(1)?.parse().ok()))
    }

    fn remove(&mut self, uid: u32) -> Result<(), String> {
        self.cmd(&format!("UID STORE {uid} +FLAGS.SILENT (\\Deleted)"))?;
        self.cmd(&format!("UID EXPUNGE {uid}"))?;
        Ok(())
    }
}

fn num_after(s: &str, key: &str) -> Option<u32> {
    let i = s.find(key)? + key.len();
    s[i..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/// A note message on the server: uuid -> (uid, penguin id). The newest copy wins.
fn list(im: &mut Imap) -> Result<HashMap<String, (u32, Option<String>)>, String> {
    let (rs, _) = im.cmd("UID FETCH 1:* (UID FLAGS BODY.PEEK[HEADER.FIELDS (X-UNIVERSALLY-UNIQUE-IDENTIFIER X-PENGUIN-ID)])")?;
    let mut out: HashMap<String, (u32, Option<String>)> = HashMap::new();
    for r in rs {
        if !r.text.contains(" FETCH ") || r.text.contains("\\Deleted") {
            continue;
        }
        let Some(uid) = num_after(&r.text, "UID ") else { continue };
        let Some(h) = r.lits.first() else { continue };
        let (hs, _) = split_msg(&[h.as_slice(), b"\r\n\r\n"].concat());
        // A message without a UUID (made by hand) gets one from its uid.
        let uuid = header(&hs, "x-universally-unique-identifier").map(str::to_string).unwrap_or_else(|| format!("uid-{uid}"));
        let pid = header(&hs, "x-penguin-id").map(str::to_string);
        if out.get(&uuid).is_none_or(|(u, _)| *u < uid) {
            out.insert(uuid, (uid, pid));
        }
    }
    Ok(out)
}

/// One full sync. Runs on a worker thread.
pub fn sync(c: &Creds, notes: &[Note], state: State) -> Result<Outcome, String> {
    sync_in(c, FOLDER, notes, state)
}

pub fn sync_in(c: &Creds, folder: &str, notes: &[Note], mut st: State) -> Result<Outcome, String> {
    if c.host.trim().is_empty() || c.user.trim().is_empty() || c.pass.is_empty() {
        return Err("enter the server, user name and password".into());
    }
    let mut im = Imap::connect(c.host.trim())?;
    im.cmd(&format!("LOGIN {} {}", quote(c.user.trim()), quote(&c.pass)))
        .map_err(|_| "user name or password was rejected".to_string())?;
    let sel = match im.cmd(&format!("SELECT {}", quote(folder))) {
        Ok(r) => r,
        Err(_) => {
            im.cmd(&format!("CREATE {}", quote(folder)))?;
            im.cmd(&format!("SUBSCRIBE {}", quote(folder))).ok();
            im.cmd(&format!("SELECT {}", quote(folder)))?
        }
    };
    let validity = sel.0.iter().find_map(|r| num_after(&r.text, "UIDVALIDITY ")).unwrap_or(0);
    if validity != st.validity {
        // Mailbox rebuilt: uids mean nothing now, uuids still do.
        for e in st.notes.values_mut() {
            e.uid = 0;
        }
        st.validity = validity;
    }
    let exists = sel.0.iter().any(|r| r.text.ends_with(" EXISTS") && !r.text.starts_with("* 0 "));
    let server = if exists { list(&mut im)? } else { HashMap::new() };

    let mut ops = vec![];
    let by_uuid: HashMap<String, String> = st.notes.iter().map(|(id, e)| (e.uuid.clone(), id.clone())).collect();
    // From the phone.
    for (uuid, (uid, pid)) in &server {
        let id = by_uuid.get(uuid).cloned().or_else(|| pid.clone().filter(|p| notes.iter().any(|n| &n.id == p)));
        let local = id.as_ref().and_then(|id| notes.iter().find(|n| &n.id == id));
        if let Some(e) = id.as_ref().and_then(|id| st.notes.get(id)) {
            if e.uid == *uid {
                continue;
            }
        }
        let (rs, _) = im.cmd(&format!("UID FETCH {uid} BODY.PEEK[]"))?;
        let Some(raw) = rs.into_iter().find_map(|r| r.lits.into_iter().next()) else { continue };
        let (title, mut body) = parse(&raw);
        let id = id.unwrap_or_else(model::new_id);
        // Keep the "# Title" heading the note had here.
        if let Some(n) = local {
            if n.body.lines().next().is_some_and(|l| l.trim() == format!("# {}", n.title.trim())) {
                body = if body.is_empty() { format!("# {title}") } else { format!("# {title}\n\n{body}") };
            }
        }
        let after = Note { id: id.clone(), title: title.clone(), body: body.clone(), tags: String::new(), updated: 0, pinned: false };
        st.notes.insert(id.clone(), Entry { uuid: uuid.clone(), uid: *uid, hash: hash(&after) });
        ops.push(Op::Put { id, title, body, base: local.map(|n| n.updated) });
    }
    // Gone from the server: deleted on the phone.
    let on_server: Vec<&String> = server.keys().collect();
    let gone: Vec<String> = st.notes.iter().filter(|(_, e)| e.uid != 0 && !on_server.contains(&&e.uuid)).map(|(id, _)| id.clone()).collect();
    for id in gone {
        let e = st.notes.remove(&id).unwrap();
        if let Some(n) = notes.iter().find(|n| n.id == id) {
            if hash(n) == e.hash {
                ops.push(Op::Delete { id, base: n.updated });
            }
            // Edited here since: it goes back up below.
        }
    }
    // To the phone.
    let changed_by_phone: Vec<String> = ops.iter().filter_map(|o| if let Op::Put { id, .. } = o { Some(id.clone()) } else { None }).collect();
    for n in notes {
        if changed_by_phone.contains(&n.id) {
            continue;
        }
        let h = hash(n);
        let prev = st.notes.get(&n.id).cloned();
        if prev.as_ref().is_some_and(|e| e.hash == h && e.uid != 0) {
            continue;
        }
        let uuid = prev.as_ref().map(|e| e.uuid.clone()).unwrap_or_else(|| new_uuid(&n.id));
        let uid = im.append(folder, &message(n, &uuid))?.unwrap_or(0);
        if let Some(old) = prev.filter(|e| e.uid != 0 && e.uid != uid) {
            im.remove(old.uid)?;
        }
        st.notes.insert(n.id.clone(), Entry { uuid, uid, hash: h });
    }
    // Deleted here.
    let gone: Vec<String> = st.notes.keys().filter(|id| !notes.iter().any(|n| &n.id == *id) && !changed_by_phone.contains(id)).cloned().collect();
    for id in gone {
        let e = st.notes.remove(&id).unwrap();
        if e.uid != 0 {
            im.remove(e.uid)?;
        }
    }
    im.cmd("LOGOUT").ok();
    st.last_ok = model::now_ms();
    Ok(Outcome { ops, state: st })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, title: &str, body: &str) -> Note {
        Note { id: id.into(), title: title.into(), body: body.into(), tags: "a".into(), updated: 1, pinned: false }
    }

    #[test]
    fn round_trips_through_html() {
        let n = note("n1", "Plan <A&B>", "# Plan <A&B>\n\n- [ ] milk\n  - nested\n\n**bold** ملاحظة");
        let raw = message(&n, "U1");
        assert!(raw.contains("X-Uniform-Type-Identifier: com.apple.mail-note"));
        let (title, body) = parse(raw.as_bytes());
        assert_eq!(title, "Plan <A&B>");
        assert_eq!(body, "- [ ] milk\n  - nested\n\n**bold** ملاحظة");
    }

    #[test]
    fn reads_what_the_iphone_writes() {
        let raw = "Subject: Groceries\r\nX-Universally-Unique-Identifier: 1A2B\r\nContent-Type: text/html;\r\n\tcharset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n<html><head><style>p{}</style></head><body><div>Groceries</div><div><br></div><div>milk &amp; eggs=\r\n</div><ul><li>bread</li><li>tea</li></ul><div>caf=C3=A9&nbsp;ok</div></body></html>";
        let (title, body) = parse(raw.as_bytes());
        assert_eq!(title, "Groceries");
        assert_eq!(body, "milk & eggs\n- bread\n- tea\ncafé ok");
    }

    #[test]
    fn multipart_picks_html() {
        let raw = "Content-Type: multipart/alternative; boundary=\"XX\"\r\n\r\n--XX\r\nContent-Type: text/plain\r\n\r\nplain\r\n--XX\r\nContent-Type: text/html\r\n\r\n<div>T</div><div>x</div>\r\n--XX--\r\n";
        assert_eq!(parse(raw.as_bytes()), ("T".into(), "x".into()));
    }

    /// Against a real server, in a folder Apple Notes never shows:
    /// PN_IMAP_HOST=… PN_IMAP_USER=… PN_IMAP_PASS=… cargo test imap_live -- --ignored
    #[test]
    #[ignore]
    fn imap_live() {
        let c = Creds {
            host: std::env::var("PN_IMAP_HOST").unwrap(),
            user: std::env::var("PN_IMAP_USER").unwrap(),
            pass: std::env::var("PN_IMAP_PASS").unwrap(),
            enabled: true,
        };
        let f = "PenguinTest";
        let mut notes = vec![note("n1", "One", "first"), note("n2", "Two", "second")];
        let out = sync_in(&c, f, &notes, State::default()).unwrap();
        assert!(out.ops.is_empty());
        let st = out.state;
        assert_eq!(st.notes.len(), 2);

        // Nothing changed: nothing happens.
        let out = sync_in(&c, f, &notes, st).unwrap();
        assert!(out.ops.is_empty());
        let st = out.state;

        // The phone edits One: a new message with the same uuid, the old one deleted.
        let e = st.notes["n1"].clone();
        let mut im = Imap::connect(&c.host).unwrap();
        im.cmd(&format!("LOGIN {} {}", quote(&c.user), quote(&c.pass))).unwrap();
        im.cmd(&format!("SELECT {f}")).unwrap();
        let edited = format!("X-Universally-Unique-Identifier: {}\r\nContent-Type: text/html\r\n\r\n<div>One!</div><div>edited on phone</div>", e.uuid);
        im.append(f, &edited).unwrap();
        im.remove(e.uid).unwrap();
        // ... and makes a new note.
        im.append(f, "X-Universally-Unique-Identifier: PHONE-1\r\nContent-Type: text/html\r\n\r\n<div>From phone</div><div>hi</div>").unwrap();
        // ... and deletes Two.
        im.remove(st.notes["n2"].uid).unwrap();

        let out = sync_in(&c, f, &notes, st).unwrap();
        let mut ops = out.ops.clone();
        ops.sort_by_key(|o| format!("{o:?}"));
        assert!(ops.contains(&Op::Put { id: "n1".into(), title: "One!".into(), body: "edited on phone".into(), base: Some(1) }), "{ops:?}");
        assert!(ops.contains(&Op::Delete { id: "n2".into(), base: 1 }), "{ops:?}");
        let new = ops.iter().find_map(|o| match o { Op::Put { id, title, base: None, .. } if title == "From phone" => Some(id.clone()), _ => None }).expect("new note");
        // Apply like the app does, then edit and delete here.
        notes = vec![note("n1", "One!", "edited on phone"), note(&new, "From phone", "hi")];
        notes[0].body = "edited here".into();
        notes.pop();
        let out = sync_in(&c, f, &notes, out.state).unwrap();
        assert!(out.ops.is_empty(), "{:?}", out.ops);
        assert_eq!(out.state.notes.len(), 1);
        // A fresh session sees what the sync left behind.
        im.cmd("LOGOUT").ok();
        let mut im = Imap::connect(&c.host).unwrap();
        im.cmd(&format!("LOGIN {} {}", quote(&c.user), quote(&c.pass))).unwrap();
        im.cmd(&format!("SELECT {f}")).unwrap();
        let left = list(&mut im).unwrap();
        assert_eq!(left.len(), 1, "{left:?}");
        let uid = left.values().next().unwrap().0;
        let (rs, _) = im.cmd(&format!("UID FETCH {uid} BODY.PEEK[]")).unwrap();
        let raw = rs.into_iter().find_map(|r| r.lits.into_iter().next()).unwrap();
        assert_eq!(parse(&raw), ("One!".into(), "edited here".into()));
        im.cmd("CLOSE").ok();
        im.cmd(&format!("DELETE {f}")).unwrap();
    }
}
