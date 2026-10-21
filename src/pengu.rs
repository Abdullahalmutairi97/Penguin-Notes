//! Pengu, the note assistant: system prompt, and the action block it can reply with
//! to create notes, add to notes, and put things on (or take them off) the calendar.
use chrono::{Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::calendar::daily_title;
use crate::model::{self, Note};

#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    Create {
        title: String,
        tags: String,
        body: String,
    },
    Append {
        title: String,
        text: String,
    },
    Schedule {
        date: NaiveDate,
        time: String,
        text: String,
    },
    /// Take an item off a day; it waits in the trash for a day.
    Unschedule {
        date: NaiveDate,
        text: String,
    },
    /// Put an item from the trash back on its day.
    Restore {
        date: NaiveDate,
        text: String,
    },
}

/// A calendar item Pengu removed, kept hidden so it can be brought back.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Deleted {
    pub date: NaiveDate,
    pub line: String,
    pub at: i64,
}

/// How long removed items stay restorable.
pub const KEEP_MS: i64 = 24 * 60 * 60 * 1000;

pub fn load_trash() -> Vec<Deleted> {
    let mut t: Vec<Deleted> = std::fs::read_to_string(model::data_dir().join("pengu-trash.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    purge(&mut t);
    t
}

pub fn save_trash(t: &[Deleted]) {
    if let Ok(s) = serde_json::to_string(t) {
        model::write_atomic("pengu-trash.json", &s);
    }
}

/// Forget what was removed more than a day ago. Returns true if anything went.
pub fn purge(t: &mut Vec<Deleted>) -> bool {
    let n = t.len();
    let now = model::now_ms();
    t.retain(|d| now - d.at < KEEP_MS);
    t.len() != n
}

/// The words of a to-do line: "- [ ] 15:00 Dentist" -> "15:00 dentist".
fn words(line: &str) -> String {
    let l = line.trim();
    let l = ["- [ ] ", "- [x] ", "- [X] ", "- "].iter().find_map(|p| l.strip_prefix(p)).unwrap_or(l);
    l.trim().to_lowercase()
}

/// Which of `lines` Pengu means by `text`: the same words, or else the one line containing them.
fn pick<'a>(lines: impl Iterator<Item = &'a str> + Clone, text: &str) -> Option<usize> {
    let want = words(text);
    if want.is_empty() {
        return None;
    }
    lines.clone().position(|l| words(l) == want).or_else(|| {
        let hits: Vec<usize> = lines.enumerate().filter(|(_, l)| words(l).contains(&want)).map(|(i, _)| i).collect();
        (hits.len() == 1).then(|| hits[0])
    })
}

impl Act {
    pub fn describe(&self) -> String {
        match self {
            Act::Create { title, .. } => format!("Created note “{title}”"),
            Act::Append { title, .. } => format!("Added to “{title}”"),
            Act::Schedule { date, time, text } => {
                let when = date.format("%a %b %-d").to_string();
                if time.is_empty() {
                    format!("{when}: {text}")
                } else {
                    format!("{when} {time}: {text}")
                }
            }
            Act::Unschedule { date, text } => format!("Removed from {}: {text}", date.format("%a %b %-d")),
            Act::Restore { date, text } => format!("Put back on {}: {text}", date.format("%a %b %-d")),
        }
    }
}

/// How to reverse one applied action: `None` = the note was created, so delete it.
pub type Undo = (String, Option<String>);

const FENCE: &str = "```pengu";

pub fn system_prompt(notes: &[Note], days: &[Note], current: Option<&Note>, trash: &[Deleted]) -> String {
    let today = Local::now().date_naive();
    let titles: Vec<String> = notes
        .iter()
        .take(60)
        .map(|n| {
            format!(
                "- {}",
                if n.title.is_empty() {
                    "Untitled"
                } else {
                    &n.title
                }
            )
        })
        .collect();
    let mut s = format!(
        r#"You are Pengu, a friendly penguin note assistant inside the Penguin Notes app.
Today is {today_long} ({today}).

You can change the user's notes and calendar. When the user asks you to write, sort, organize, save, remember, add, plan or schedule something, do it by ending your reply with ONE fenced block like this:

{FENCE}
[
  {{"action": "create_note", "title": "Short title", "tags": "tag1, tag2", "body": "markdown"}},
  {{"action": "append", "title": "Existing note title", "text": "markdown to add at the end"}},
  {{"action": "schedule", "date": "YYYY-MM-DD", "time": "HH:MM or empty", "text": "what happens"}},
  {{"action": "unschedule", "date": "YYYY-MM-DD", "text": "the item as it appears on that day"}},
  {{"action": "restore", "date": "YYYY-MM-DD", "text": "a recently removed item"}}
]
```

Rules:
- When the user dumps loose words or thoughts, sort them into a clean, well-structured note with create_note (headings, bullet lists, "- [ ] " checkboxes for to-dos).
- Anything with a day or deadline also gets a schedule action. Convert relative dates ("tomorrow", "next friday") to YYYY-MM-DD using today's date. Never schedule without a concrete date.
- Use append only with a title from the list of existing notes.
- unschedule removes an item from the calendar. Only remove what the user asks you to remove (cancelled, moved, not needed). To move an item, unschedule it and schedule it on the new date. Removed items are kept for 24 hours, and restore puts one back.
- Outside the block, reply in one or two short, friendly sentences saying what you did. Do not repeat the note contents.
- Only include the block when there is something to change. For plain questions just answer.
- Reply in the language the user wrote their last message in (English or Arabic).

Note markup: # headings, - lists, - [ ] checkboxes, | tables |, ==highlight==, [[Note title]] links, and charts as fenced blocks: ```chart bar|line|pie with lines "Label: number" and optional "title: text".

Existing notes:
{titles}"#,
        today_long = today.format("%A, %B %-d, %Y"),
        titles = if titles.is_empty() {
            "(none)".into()
        } else {
            titles.join("\n")
        },
    );
    // What is on the calendar from a week ago to a month ahead, so Pengu can plan around it.
    let mut plan: Vec<(NaiveDate, &str)> = days
        .iter()
        .filter_map(|d| Some((model::day_date(d)?, d.body.as_str())))
        .filter(|(d, _)| (-7..=31).contains(&(*d - today).num_days()))
        .collect();
    plan.sort_by_key(|p| p.0);
    let plan: Vec<String> = plan
        .iter()
        .filter(|(_, b)| !b.trim().is_empty())
        .map(|(d, b)| format!("{}:\n{}", d.format("%Y-%m-%d (%a)"), b.trim()))
        .collect();
    if !plan.is_empty() {
        s += &format!("\n\nThe user's calendar (each date has its own day page):\n{}", plan.join("\n"));
    }
    // What Pengu removed in the last day, so it can put things back.
    if !trash.is_empty() {
        let list: Vec<String> = trash.iter().map(|d| format!("- {}: {}", d.date.format("%Y-%m-%d"), d.line.trim())).collect();
        s += &format!("\n\nRecently removed from the calendar (can be restored for 24 hours):\n{}", list.join("\n"));
    }
    if let Some(n) = current {
        let t = if n.title.is_empty() {
            "Untitled"
        } else {
            &n.title
        };
        s += &format!("\n\nThe note the user has open, \"{t}\":\n{}", n.body);
    }
    s
}

/// The reply text with the action block removed (also while it is still streaming in).
pub fn visible(reply: &str) -> &str {
    reply.find(FENCE).map_or(reply, |i| &reply[..i]).trim_end()
}

fn field(v: &Value, k: &str) -> String {
    v.get(k)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

pub fn parse(reply: &str) -> Vec<Act> {
    let Some(start) = reply.find(FENCE) else {
        return vec![];
    };
    let rest = &reply[start + FENCE.len()..];
    // The block ends at a fence on its own line. Fences inside the JSON (a ```chart in a
    // note body) are inside strings, where line breaks are written as \n, so they never start a line.
    let body = rest.find("\n```").map_or(rest, |e| &rest[..e]);
    let Ok(v) = serde_json::from_str::<Value>(body.trim()) else {
        return vec![];
    };
    let items = match v {
        Value::Array(a) => a,
        o @ Value::Object(_) => vec![o],
        _ => vec![],
    };
    items
        .iter()
        .filter_map(|it| match field(it, "action").as_str() {
            "create_note" | "create" => {
                let title = field(it, "title");
                (!title.is_empty()).then(|| Act::Create {
                    title,
                    tags: field(it, "tags"),
                    body: field(it, "body"),
                })
            }
            "append" => {
                let (title, text) = (field(it, "title"), field(it, "text"));
                (!title.is_empty() && !text.is_empty()).then_some(Act::Append { title, text })
            }
            "schedule" => {
                let date = NaiveDate::parse_from_str(&field(it, "date"), "%Y-%m-%d").ok()?;
                let text = field(it, "text");
                (!text.is_empty()).then(|| Act::Schedule {
                    date,
                    time: field(it, "time"),
                    text,
                })
            }
            "unschedule" | "remove" | "delete" | "restore" => {
                let date = NaiveDate::parse_from_str(&field(it, "date"), "%Y-%m-%d").ok()?;
                let text = field(it, "text");
                if text.is_empty() {
                    return None;
                }
                Some(if field(it, "action") == "restore" {
                    Act::Restore { date, text }
                } else {
                    Act::Unschedule { date, text }
                })
            }
            _ => None,
        })
        .collect()
}

fn find(notes: &[Note], title: &str) -> Option<usize> {
    notes
        .iter()
        .position(|n| n.title.trim().eq_ignore_ascii_case(title.trim()))
}

pub fn add_text(body: &mut String, text: &str) {
    let t = body.trim_end().len();
    body.truncate(t);
    if !body.is_empty() {
        body.push_str(
            if text.starts_with("- ") && body.lines().last().is_some_and(|l| l.starts_with("- ")) {
                "\n"
            } else {
                "\n\n"
            },
        );
    }
    body.push_str(text);
}

/// Apply actions: notes go to `notes`, scheduled items to the day pages in `days`, removed
/// items to `trash`. Returns undo records, the id of the note to show, and the first day touched.
/// (Removals and restores are undone through the trash; see `undo_trash`.)
pub fn apply(
    notes: &mut Vec<Note>,
    days: &mut Vec<Note>,
    trash: &mut Vec<Deleted>,
    acts: &[Act],
) -> (Vec<Undo>, Option<String>, Option<NaiveDate>) {
    let mut undo: Vec<Undo> = Vec::new();
    let mut show = None;
    let mut first_day = None;
    let remember = |undo: &mut Vec<Undo>, n: &Note| {
        if !undo.iter().any(|(id, _)| *id == n.id) {
            undo.push((n.id.clone(), Some(n.body.clone())));
        }
    };
    for a in acts {
        match a {
            Act::Unschedule { date, text } => {
                if let Some(line) = take_line(days, *date, text) {
                    trash.push(Deleted { date: *date, line, at: model::now_ms() });
                    first_day = first_day.or(Some(*date));
                }
                continue;
            }
            Act::Restore { date, text } => {
                let on_day: Vec<usize> = (0..trash.len()).filter(|&i| trash[i].date == *date).collect();
                if let Some(k) = pick(on_day.iter().map(|&i| trash[i].line.as_str()), text) {
                    let d = trash.remove(on_day[k]);
                    add_to_daily(days, d.date, &d.line);
                    first_day = first_day.or(Some(*date));
                }
                continue;
            }
            _ => {}
        }
        let (title, text, fresh_body, tags) = match a {
            Act::Create { title, tags, body } => {
                (title.clone(), body.clone(), body.clone(), tags.clone())
            }
            Act::Append { title, text } => {
                (title.clone(), text.clone(), text.clone(), String::new())
            }
            Act::Schedule { date, time, text } => {
                let item = if time.is_empty() {
                    format!("- [ ] {text}")
                } else {
                    format!("- [ ] {time} {text}")
                };
                first_day = first_day.or(Some(*date));
                (daily_title(*date), item.clone(), item, String::new())
            }
            Act::Unschedule { .. } | Act::Restore { .. } => unreachable!(),
        };
        let list: &mut Vec<Note> = if matches!(a, Act::Schedule { .. }) { &mut *days } else { &mut *notes };
        let i = match find(list, &title) {
            Some(i) => {
                remember(&mut undo, &list[i]);
                add_text(&mut list[i].body, &text);
                if !tags.is_empty() && list[i].tags.trim().is_empty() {
                    list[i].tags = tags;
                }
                i
            }
            None => {
                let n = Note {
                    id: model::new_id(),
                    title,
                    body: fresh_body,
                    tags,
                    updated: 0,
                    pinned: false,
                };
                undo.push((n.id.clone(), None));
                list.insert(0, n);
                0
            }
        };
        list[i].updated = model::now_ms();
        if !matches!(a, Act::Schedule { .. }) {
            show = Some(list[i].id.clone());
        }
    }
    (undo, show, first_day)
}

/// Append a checklist line to the day page for `date`, creating the page if needed.
pub fn add_to_daily(notes: &mut Vec<Note>, date: NaiveDate, line: &str) {
    match find(notes, &daily_title(date)) {
        Some(i) => {
            add_text(&mut notes[i].body, line);
            notes[i].updated = model::now_ms();
        }
        None => notes.insert(
            0,
            Note {
                id: model::new_id(),
                title: daily_title(date),
                body: line.to_string(),
                tags: String::new(),
                updated: model::now_ms(),
                pinned: false,
            },
        ),
    }
}

/// Take the item Pengu means off the day page for `date`; returns the line as it was.
fn take_line(days: &mut Vec<Note>, date: NaiveDate, text: &str) -> Option<String> {
    let i = find(days, &daily_title(date))?;
    let k = pick(days[i].body.split('\n'), text)?;
    let line = days[i].body.split('\n').nth(k)?.trim_end().to_string();
    remove_from_daily_pages(days, date, &line);
    Some(line)
}

/// Undo what one reply did to the trash: `before` is the trash as it was before the reply.
/// What it removed goes back on its day; what it restored goes back in the trash.
pub fn undo_trash(days: &mut Vec<Note>, trash: &mut Vec<Deleted>, before: &[Deleted]) {
    let removed: Vec<Deleted> = trash.iter().filter(|d| !before.contains(d)).cloned().collect();
    let restored: Vec<Deleted> = before.iter().filter(|d| !trash.contains(d)).cloned().collect();
    for d in &removed {
        let have = find(days, &daily_title(d.date)).is_some_and(|i| days[i].body.lines().any(|l| l.trim_end() == d.line));
        if !have {
            add_to_daily(days, d.date, &d.line);
        }
    }
    trash.retain(|d| !removed.contains(d));
    for d in restored {
        remove_from_daily_pages(days, d.date, &d.line);
        trash.push(d);
    }
}

/// Remove the first line equal to `line` from the day page for `date`; an emptied page goes away.
pub fn remove_from_daily(notes: &mut [Note], date: NaiveDate, line: &str) {
    if let Some(i) = find(notes, &daily_title(date)) {
        let n = &mut notes[i];
        let mut lines: Vec<&str> = n.body.split('\n').collect();
        if let Some(k) = lines.iter().position(|l| l.trim_end() == line) {
            lines.remove(k);
            n.body = lines.join("\n").trim().to_string();
            n.updated = model::now_ms();
        }
    }
}

pub fn remove_from_daily_pages(days: &mut Vec<Note>, date: NaiveDate, line: &str) {
    remove_from_daily(days, date, line);
    days.retain(|d| !d.body.trim().is_empty());
}

pub fn revert(notes: &mut Vec<Note>, days: &mut Vec<Note>, undo: &[Undo]) {
    for (id, prior) in undo.iter().rev() {
        for list in [&mut *notes, &mut *days] {
            match prior {
                None => list.retain(|n| &n.id != id),
                Some(body) => {
                    if let Some(n) = list.iter_mut().find(|n| &n.id == id) {
                        n.body = body.clone();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(title: &str, body: &str) -> Note {
        Note {
            id: format!("id-{title}"),
            title: title.into(),
            body: body.into(),
            tags: String::new(),
            updated: 0,
            pinned: false,
        }
    }

    const REPLY: &str = "Done! I sorted it.\n```pengu\n[{\"action\":\"create_note\",\"title\":\"Groceries\",\"tags\":\"shopping\",\"body\":\"- [ ] milk\"},\n {\"action\":\"schedule\",\"date\":\"2026-10-20\",\"time\":\"15:00\",\"text\":\"Dentist\"},\n {\"action\":\"append\",\"title\":\"welcome\",\"text\":\"- [ ] new\"},\n {\"action\":\"schedule\",\"date\":\"next friday\",\"text\":\"bad\"}]\n```";

    #[test]
    fn parses_and_hides_block() {
        assert_eq!(visible(REPLY), "Done! I sorted it.");
        assert_eq!(visible("Thinking ```pen"), "Thinking ```pen");
        let acts = parse(REPLY);
        assert_eq!(acts.len(), 3, "invalid date is dropped");
        assert!(matches!(&acts[1], Act::Schedule { time, .. } if time == "15:00"));
    }

    #[test]
    fn note_with_a_chart_inside() {
        // What Pengu really sent: a ```chart fence inside the note body.
        let reply = "Doing great! Here's a test note.\n\n```pengu\n[\n  {\"action\": \"create_note\", \"title\": \"Test Note\", \"tags\": \"test\", \"body\": \"# Test Note\\n\\n```chart bar\\nNotes: 3\\n```\\n\\nAll set\"}\n]\n```";
        let acts = parse(reply);
        assert_eq!(acts.len(), 1, "{reply}");
        assert!(matches!(&acts[0], Act::Create { title, body, .. } if title == "Test Note" && body.contains("```chart bar\nNotes: 3\n```")));
        assert_eq!(visible(reply), "Doing great! Here's a test note.");
    }

    /// PN_REPLY=file-with-a-real-reply cargo test real_reply -- --ignored
    #[test]
    #[ignore]
    fn real_reply() {
        let r = std::fs::read_to_string(std::env::var("PN_REPLY").unwrap()).unwrap();
        let acts = parse(&r);
        println!("{acts:?}");
        assert!(!acts.is_empty());
    }

    #[test]
    fn apply_and_undo() {
        let mut notes = vec![note("Welcome", "# Welcome\n\n- [x] a")];
        let mut days = vec![];
        let before = notes[0].body.clone();
        let (undo, show, day) = apply(&mut notes, &mut days, &mut vec![], &parse(REPLY));
        assert_eq!(notes.len(), 2, "the schedule goes to a day page, not a note");
        assert_eq!(day, NaiveDate::from_ymd_opt(2026, 10, 20));
        let daily = days.iter().find(|n| n.title == "2026-10-20").unwrap();
        assert_eq!(daily.body, "- [ ] 15:00 Dentist");
        assert!(
            notes
                .iter()
                .find(|n| n.title == "Welcome")
                .unwrap()
                .body
                .ends_with("- [x] a\n- [ ] new")
        );
        assert_eq!(show.as_deref(), Some("id-Welcome"));
        revert(&mut notes, &mut days, &undo);
        assert_eq!(notes.len(), 1);
        assert!(days.is_empty());
        assert_eq!(notes[0].body, before);
    }

    #[test]
    fn unschedule_restore_and_undo() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
        let mut notes = vec![];
        let mut days = vec![note("2026-10-21", "- [ ] 15:00 Dentist\n- [ ] Gym\n- [x] Gym bag")];
        let mut trash = vec![];
        let acts = parse("ok\n```pengu\n[{\"action\":\"unschedule\",\"date\":\"2026-10-21\",\"text\":\"dentist\"}]\n```");
        assert_eq!(acts, vec![Act::Unschedule { date: d, text: "dentist".into() }]);
        let (_, _, day) = apply(&mut notes, &mut days, &mut trash, &acts);
        assert_eq!(day, Some(d));
        assert_eq!(days[0].body, "- [ ] Gym\n- [x] Gym bag");
        assert_eq!(trash.len(), 1);
        assert_eq!(trash[0].line, "- [ ] 15:00 Dentist");
        // Pengu sees it, and can put it back.
        assert!(system_prompt(&notes, &days, None, &trash).contains("2026-10-21: - [ ] 15:00 Dentist"));
        let before = trash.clone();
        apply(&mut notes, &mut days, &mut trash, &[Act::Restore { date: d, text: "15:00 Dentist".into() }]);
        assert!(trash.is_empty());
        assert!(days[0].body.contains("- [ ] 15:00 Dentist"));
        // Undoing the restore sends it back to the trash.
        undo_trash(&mut days, &mut trash, &before);
        assert_eq!(trash, before);
        assert!(!days[0].body.contains("Dentist"));
        // Undoing the removal puts it back on the day, once.
        undo_trash(&mut days, &mut trash, &[]);
        assert!(trash.is_empty());
        assert_eq!(days[0].body.matches("Dentist").count(), 1);
    }

    #[test]
    fn unschedule_only_what_is_meant() {
        let mut days = vec![note("2026-10-21", "- [ ] Gym\n- [x] Gym bag")];
        let mut trash = vec![];
        let d = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
        // "gym" is exactly one item; "gy" could be either, so nothing is removed.
        apply(&mut vec![], &mut days, &mut trash, &[Act::Unschedule { date: d, text: "gy".into() }]);
        assert!(trash.is_empty());
        apply(&mut vec![], &mut days, &mut trash, &[Act::Unschedule { date: d, text: "Gym".into() }]);
        assert_eq!(days[0].body, "- [x] Gym bag");
        // The last item takes its empty day page with it; undo brings both back.
        apply(&mut vec![], &mut days, &mut trash, &[Act::Unschedule { date: d, text: "gym bag".into() }]);
        assert!(days.is_empty());
        let before = trash[..1].to_vec();
        undo_trash(&mut days, &mut trash, &before);
        assert_eq!(days[0].body, "- [x] Gym bag");
    }

    #[test]
    fn trash_forgets_after_a_day() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
        let now = model::now_ms();
        let mut t = vec![
            Deleted { date: d, line: "- [ ] old".into(), at: now - KEEP_MS - 1 },
            Deleted { date: d, line: "- [ ] new".into(), at: now - 1000 },
        ];
        assert!(purge(&mut t));
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].line, "- [ ] new");
    }

    #[test]
    fn schedule_twice_same_day_appends() {
        let mut notes = vec![];
        let d = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
        let acts = vec![
            Act::Schedule {
                date: d,
                time: String::new(),
                text: "a".into(),
            },
            Act::Schedule {
                date: d,
                time: String::new(),
                text: "b".into(),
            },
        ];
        let mut days = vec![];
        apply(&mut notes, &mut days, &mut vec![], &acts);
        assert!(notes.is_empty());
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].body, "- [ ] a\n- [ ] b");
    }
}
