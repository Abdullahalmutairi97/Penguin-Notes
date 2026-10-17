//! Two-way mirror of every note as a Markdown file in a folder (Obsidian-compatible),
//! so a sync tool (Syncthing, iCloud Drive, …) can carry them to the phone.
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{self, Note};

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct Entry {
    pub file: String,
    pub hash: u64,
    /// When we last wrote or read the file (note.updated at that time).
    pub synced: i64,
}

#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct State {
    pub dir: String,
    pub files: HashMap<String, Entry>,
}

fn hash(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn safe_name(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| {
            if "/\\:*?\"<>|#^[]".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s: String = s.chars().take(80).collect();
    if s.is_empty() { "Untitled".into() } else { s }
}

pub fn render(n: &Note) -> String {
    let tags = n.tag_list();
    let mut fm = format!("---\nid: {}\n", n.id);
    if !tags.is_empty() {
        fm += &format!("tags: [{}]\n", tags.join(", "));
    }
    if n.pinned {
        fm += "pinned: true\n";
    }
    fm + "---\n" + &n.body
}

/// Split front matter from the body: (id, tags, pinned, body).
fn parse(text: &str) -> (Option<String>, Option<String>, bool, String) {
    let text = text.replace("\r\n", "\n");
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, None, false, text);
    };
    let Some(end) = rest.find("\n---") else {
        return (None, None, false, text);
    };
    let (fm, body) = (
        &rest[..end],
        rest[end + 4..]
            .strip_prefix('\n')
            .unwrap_or(&rest[end + 4..]),
    );
    let (mut id, mut tags, mut pinned) = (None, None, false);
    let mut list_tags: Vec<String> = vec![];
    let mut in_tags = false;
    for line in fm.lines() {
        if in_tags {
            if let Some(t) = line.trim().strip_prefix("- ") {
                list_tags.push(t.trim().to_string());
                continue;
            }
            in_tags = false;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "id" => id = Some(v.to_string()).filter(|s| !s.is_empty()),
            "pinned" => pinned = v == "true",
            "tags" if v.is_empty() => in_tags = true,
            "tags" => {
                tags = Some(
                    v.trim_start_matches('[')
                        .trim_end_matches(']')
                        .split(',')
                        .map(|t| t.trim().trim_matches('"'))
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            }
            _ => {}
        }
    }
    if !list_tags.is_empty() {
        tags = Some(list_tags.join(", "));
    }
    (id, tags, pinned, body.to_string())
}

fn md_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    rd.filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "md") && p.is_file())
        .collect()
}

/// Files changed in the folder -> notes. Returns true if any note changed.
pub fn import(notes: &mut Vec<Note>, st: &mut State) -> bool {
    let dir = PathBuf::from(&st.dir);
    if !dir.is_dir() {
        return false;
    }
    let files = md_files(&dir);
    let mut changed = false;
    let mut seen: Vec<String> = vec![];
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string();
        let file = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let (id, tags, pinned, body) = parse(&text);
        let h = hash(&text);
        let known = id
            .as_ref()
            .and_then(|id| st.files.get(id))
            .filter(|e| e.file == file);
        if known.is_some_and(|e| e.hash == h) {
            seen.push(id.unwrap());
            continue;
        }
        // New or edited outside the app.
        match id
            .as_ref()
            .and_then(|id| notes.iter().position(|n| &n.id == id))
        {
            Some(i) => {
                let n = &mut notes[i];
                n.title = name;
                n.body = body;
                n.tags = tags.unwrap_or_default();
                n.pinned = pinned;
                n.updated = model::now_ms();
                st.files.insert(
                    n.id.clone(),
                    Entry {
                        file,
                        hash: h,
                        synced: n.updated,
                    },
                );
                seen.push(n.id.clone());
            }
            None => {
                let n = Note {
                    id: id.unwrap_or_else(model::new_id),
                    title: name,
                    body,
                    tags: tags.unwrap_or_default(),
                    updated: model::now_ms(),
                    pinned,
                };
                // Hash 0 forces a rewrite so the file gains its front matter.
                st.files.insert(
                    n.id.clone(),
                    Entry {
                        file,
                        hash: 0,
                        synced: n.updated,
                    },
                );
                seen.push(n.id.clone());
                notes.insert(0, n);
            }
        }
        changed = true;
    }
    // Files that vanished: deleted on the phone. Skip if the folder looks unmounted/empty.
    if !files.is_empty() {
        let gone: Vec<String> = st
            .files
            .keys()
            .filter(|id| !seen.contains(id))
            .cloned()
            .collect();
        for id in gone {
            let e = st.files.remove(&id).unwrap();
            // Only delete notes not edited here since the last sync.
            if notes.iter().any(|n| n.id == id && n.updated <= e.synced) {
                notes.retain(|n| n.id != id);
                changed = true;
            }
        }
    }
    changed
}

/// Notes -> files.
pub fn export(notes: &[Note], st: &mut State) -> Result<(), String> {
    let dir = PathBuf::from(&st.dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    let mut taken: Vec<String> = vec![];
    for n in notes {
        let base = safe_name(&n.title);
        let mut file = format!("{base}.md");
        let mut k = 2;
        while taken.iter().any(|t| t.eq_ignore_ascii_case(&file)) {
            file = format!("{base} ({k}).md");
            k += 1;
        }
        taken.push(file.clone());
        let text = render(n);
        let h = hash(&text);
        let prev = st.files.get(&n.id).cloned();
        if prev.as_ref().is_some_and(|e| e.file == file && e.hash == h) {
            continue;
        }
        if let Some(old) = prev.as_ref().filter(|e| e.file != file) {
            let _ = std::fs::remove_file(dir.join(&old.file));
        }
        std::fs::write(dir.join(&file), &text).map_err(|e| format!("can't write {file}: {e}"))?;
        st.files.insert(
            n.id.clone(),
            Entry {
                file,
                hash: h,
                synced: n.updated,
            },
        );
    }
    // Notes deleted here.
    let gone: Vec<String> = st
        .files
        .keys()
        .filter(|id| !notes.iter().any(|n| &n.id == *id))
        .cloned()
        .collect();
    for id in gone {
        if let Some(e) = st.files.remove(&id) {
            let p = dir.join(&e.file);
            if std::fs::read_to_string(&p).is_ok_and(|t| hash(&t) == e.hash) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, title: &str, body: &str) -> Note {
        Note {
            id: id.into(),
            title: title.into(),
            body: body.into(),
            tags: "a, b".into(),
            updated: 1,
            pinned: false,
        }
    }

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("pn-mirror-{}", model::new_id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn round_trip_and_phone_edits() {
        let dir = tmp();
        let mut st = State {
            dir: dir.to_string_lossy().into(),
            ..Default::default()
        };
        let mut notes = vec![
            note("n1", "Groceries / Week", "- [ ] milk"),
            note("n2", "Ideas", "x"),
        ];
        export(&notes, &mut st).unwrap();
        let g = dir.join("Groceries Week.md");
        assert_eq!(
            std::fs::read_to_string(&g).unwrap(),
            "---\nid: n1\ntags: [a, b]\n---\n- [ ] milk"
        );
        assert!(!import(&mut notes, &mut st), "nothing changed");

        // Edited on the phone.
        std::fs::write(&g, "---\nid: n1\ntags:\n  - food\n---\n- [x] milk").unwrap();
        assert!(import(&mut notes, &mut st));
        assert_eq!(
            (notes[0].body.as_str(), notes[0].tags.as_str()),
            ("- [x] milk", "food")
        );

        // New file from the phone (no front matter) becomes a note and gains an id.
        std::fs::write(dir.join("From phone.md"), "hello").unwrap();
        assert!(import(&mut notes, &mut st));
        assert_eq!(notes[0].title, "From phone");
        export(&notes, &mut st).unwrap();
        assert!(
            std::fs::read_to_string(dir.join("From phone.md"))
                .unwrap()
                .starts_with("---\nid: ")
        );

        // Deleted on the phone.
        std::fs::remove_file(dir.join("Ideas.md")).unwrap();
        assert!(import(&mut notes, &mut st));
        assert!(!notes.iter().any(|n| n.id == "n2"));

        // Renamed and deleted here.
        notes.iter_mut().find(|n| n.id == "n1").unwrap().title = "Shopping".into();
        notes.retain(|n| n.title != "From phone");
        export(&notes, &mut st).unwrap();
        let mut names: Vec<String> = md_files(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into())
            .collect();
        names.sort();
        assert_eq!(names, vec!["Shopping.md"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn empty_folder_never_deletes_notes() {
        let dir = tmp();
        let mut st = State {
            dir: dir.to_string_lossy().into(),
            ..Default::default()
        };
        let mut notes = vec![note("n1", "A", "x")];
        export(&notes, &mut st).unwrap();
        std::fs::remove_file(dir.join("A.md")).unwrap();
        import(&mut notes, &mut st);
        assert_eq!(notes.len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
