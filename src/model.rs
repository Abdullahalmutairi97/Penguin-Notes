use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: String,
    pub updated: i64,
    #[serde(default)]
    pub pinned: bool,
}

impl Note {
    pub fn tag_list(&self) -> Vec<String> {
        self.tags
            .split(',')
            .map(|t| t.trim().to_lowercase())
            .filter(|t| !t.is_empty())
            .collect()
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AiCfg {
    pub preset: String,
    pub url: String,
    pub model: String,
    pub key: String,
}

impl Default for AiCfg {
    fn default() -> Self {
        Self {
            preset: "Ollama".into(),
            url: "http://localhost:11434/v1".into(),
            model: String::new(),
            key: String::new(),
        }
    }
}

/// (name, endpoint, default model)
pub const PRESETS: [(&str, &str, &str); 6] = [
    ("DeepSeek", "https://api.deepseek.com", "deepseek-flash"),
    ("OpenAI", "https://api.openai.com/v1", ""),
    ("OpenRouter", "https://openrouter.ai/api/v1", ""),
    ("Ollama", "http://localhost:11434/v1", ""),
    ("LM Studio", "http://localhost:1234/v1", ""),
    ("Custom", "", ""),
];

#[derive(Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub dark: bool,
    #[serde(default)]
    pub ai: AiCfg,
    #[serde(default = "yes")]
    pub pengu_edits: bool,
    /// Pengu's quick-action pills and toggles are expanded (collapsed with the arrow)
    #[serde(default = "yes")]
    pub actions_open: bool,
    #[serde(default = "yes")]
    pub sidebar_open: bool,
    /// The top bar and formatting toolbar are shown (folded into a thin strip otherwise)
    #[serde(default = "yes")]
    pub topbar_open: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark: false,
            ai: AiCfg::default(),
            pengu_edits: true,
            actions_open: true,
            sidebar_open: true,
            topbar_open: true,
        }
    }
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    format!("n{}{}", now_ms(), N.fetch_add(1, Ordering::Relaxed))
}

pub fn data_dir() -> PathBuf {
    // Tests must never touch the real notes or settings.
    if cfg!(test) {
        return std::env::temp_dir().join(format!("penguin-notes-test-{}", std::process::id()));
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("penguin-notes")
}

pub fn write_atomic(name: &str, data: &str) {
    let dir = data_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let tmp = dir.join(format!("{name}.tmp"));
    if std::fs::write(&tmp, data).is_ok() {
        // Notes and the API key are private: owner read/write only.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        let _ = std::fs::rename(&tmp, dir.join(name));
    }
}

pub fn load_notes() -> Vec<Note> {
    let notes: Vec<Note> = std::fs::read_to_string(data_dir().join("notes.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if notes.is_empty() {
        welcome_notes()
    } else {
        notes
    }
}

/// A notes file exactly as it is on disk (no welcome notes for an empty one).
pub fn read_list(name: &str) -> Option<Vec<Note>> {
    serde_json::from_str(&std::fs::read_to_string(data_dir().join(name)).ok()?).ok()
}

pub fn mtime(name: &str) -> Option<std::time::SystemTime> {
    std::fs::metadata(data_dir().join(name)).ok()?.modified().ok()
}

/// Bring changes another process saved (`disk`) into `mine`. `since` is when the two last
/// agreed: a note only one side has is new there if it changed after that, else the other
/// side deleted it. For notes both have, the later edit wins. Returns true if `mine` changed.
pub fn merge(mine: &mut Vec<Note>, disk: Vec<Note>, since: i64) -> bool {
    let mut changed = false;
    mine.retain(|n| {
        let keep = n.updated > since || disk.iter().any(|d| d.id == n.id);
        changed |= !keep;
        keep
    });
    for d in disk {
        match mine.iter_mut().find(|n| n.id == d.id) {
            Some(n) if d.updated > n.updated => {
                *n = d;
                changed = true;
            }
            Some(_) => {}
            None if d.updated > since => {
                mine.insert(0, d);
                changed = true;
            }
            None => {}
        }
    }
    changed
}

pub fn save_notes(notes: &[Note]) {
    if let Ok(s) = serde_json::to_string(notes) {
        write_atomic("notes.json", &s);
    }
}

/// Day pages: one per calendar date, titled "YYYY-MM-DD", kept apart from the notes.
pub fn load_days() -> Vec<Note> {
    std::fs::read_to_string(data_dir().join("days.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_days(days: &[Note]) {
    if let Ok(s) = serde_json::to_string(days) {
        write_atomic("days.json", &s);
    }
}

pub fn day_date(n: &Note) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(n.title.trim(), "%Y-%m-%d").ok()
}

/// Move old daily notes ("2026-10-16") out of the notes into day pages, dropping the
/// "# Friday, October 16, 2026" heading they were made with. Returns true if anything moved.
pub fn move_daily_notes(notes: &mut Vec<Note>, days: &mut Vec<Note>) -> bool {
    let before = notes.len();
    let mut keep = Vec::with_capacity(before);
    for mut n in std::mem::take(notes) {
        let Some(date) = day_date(&n) else {
            keep.push(n);
            continue;
        };
        n.title = date.format("%Y-%m-%d").to_string();
        let first = n.body.lines().next().unwrap_or("");
        if first.starts_with("# ") && !first.starts_with("# [") {
            n.body = n.body.lines().skip(1).collect::<Vec<_>>().join("\n").trim_start().to_string();
        }
        match days.iter_mut().find(|d| d.title == n.title) {
            Some(d) => {
                // Only what the page doesn't have yet, so moving the same note twice adds nothing.
                let have: Vec<String> = d.body.lines().map(|l| l.trim().to_string()).collect();
                let new: Vec<&str> =
                    n.body.lines().filter(|l| l.trim().is_empty() || !have.iter().any(|h| h == l.trim())).collect();
                let new = new.join("\n");
                if !new.trim().is_empty() {
                    let body = std::mem::take(&mut d.body);
                    d.body = [body.trim_end(), new.trim()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join("\n\n");
                }
                d.updated = d.updated.max(n.updated);
            }
            None => days.push(n),
        }
    }
    *notes = keep;
    notes.len() != before
}

pub fn load_settings() -> Settings {
    std::fs::read_to_string(data_dir().join("settings.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(s: &Settings) {
    if let Ok(j) = serde_json::to_string(s) {
        write_atomic("settings.json", &j);
    }
}

pub fn export_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Downloads"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(data_dir)
}

#[cfg(test)]
pub fn load_notes_default() -> Vec<Note> {
    welcome_notes()
}

fn welcome_notes() -> Vec<Note> {
    vec![
        Note {
            id: "n1".into(),
            title: "Welcome".into(),
            tags: "intro".into(),
            updated: now_ms(),
            pinned: false,
            body: "# Welcome\n\nNotes save automatically on this machine.\n\n- [x] Write a note\n- [ ] Connect a local model (Local AI button)\n\nLink notes with [[Sales chart]]. Use ==highlight==, ~~strike~~, `code` and [links](https://example.com).\n\n| Tool | Use |\n| --- | --- |\n| Chart | bar, line, pie |\n| Wikilink | link notes |".into(),
        },
        Note {
            id: "n2".into(),
            title: "Sales chart".into(),
            tags: "intro, data".into(),
            updated: now_ms() - 1000,
            pinned: false,
            body: "# Sales chart\n\nBack to [[Welcome]].\n\n```chart bar\ntitle: Units per month\nJan: 12\nFeb: 19\nMar: 8\nApr: 24\n```\n\n```chart pie\nWeb: 55\nStore: 30\nPartner: 15\n```".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(id: &str, title: &str, body: &str) -> Note {
        Note { id: id.into(), title: title.into(), body: body.into(), tags: String::new(), updated: 5, pinned: false }
    }

    #[test]
    fn merges_what_another_process_saved() {
        let since = 100;
        let mk = |id: &str, body: &str, updated: i64| Note { updated, ..n(id, id, body) };
        let mut mine = vec![mk("a", "mine", 150), mk("b", "old", 50), mk("c", "made here", 120), mk("gone", "x", 50)];
        let disk = vec![
            mk("a", "theirs", 140),       // older than my edit: mine stays
            mk("b", "pengu added", 130),  // edited there
            mk("new", "from pengu", 130), // made there
            mk("deleted-here", "x", 40),  // I deleted it before `since`
        ];
        assert!(merge(&mut mine, disk, since));
        let get = |id: &str| mine.iter().find(|n| n.id == id).map(|n| n.body.clone());
        assert_eq!(get("a").as_deref(), Some("mine"));
        assert_eq!(get("b").as_deref(), Some("pengu added"));
        assert_eq!(get("c").as_deref(), Some("made here"), "made here after `since`");
        assert_eq!(get("new").as_deref(), Some("from pengu"));
        assert_eq!(get("gone"), None, "deleted there");
        assert_eq!(get("deleted-here"), None);
    }

    #[test]
    fn daily_notes_move_to_day_pages() {
        let mut notes = vec![
            n("a", "Goals", "# Goals"),
            n("b", "2026-10-16", "# Friday, October 16, 2026\n\n- [ ] 15:00 Dentist\n\nbring card"),
            n("c", "2026-10-17", "- [ ] Gym"),
        ];
        let mut days = vec![n("d", "2026-10-17", "- [ ] Swim")];
        assert!(move_daily_notes(&mut notes, &mut days));
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].title, "Goals");
        let fri = days.iter().find(|d| d.title == "2026-10-16").unwrap();
        assert_eq!(fri.body, "- [ ] 15:00 Dentist\n\nbring card");
        assert_eq!(fri.id, "b", "keeps its id");
        assert_eq!(days.iter().find(|d| d.title == "2026-10-17").unwrap().body, "- [ ] Swim\n\n- [ ] Gym");
        assert!(!move_daily_notes(&mut notes, &mut days));
        // an old app saving the same daily note again must not double the to-dos
        notes.push(n("b2", "2026-10-16", "# Friday, October 16, 2026\n\n- [ ] 15:00 Dentist\n- [ ] new one"));
        move_daily_notes(&mut notes, &mut days);
        let fri = days.iter().find(|d| d.title == "2026-10-16").unwrap();
        assert_eq!(fri.body, "- [ ] 15:00 Dentist\n\nbring card\n\n- [ ] new one");
    }
}
