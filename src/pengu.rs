//! Pengu, the note assistant: system prompt, and the action block it can reply with
//! to create notes, add to notes, and put things on the calendar.
use chrono::{Local, NaiveDate};
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
        }
    }
}

/// How to reverse one applied action: `None` = the note was created, so delete it.
pub type Undo = (String, Option<String>);

const FENCE: &str = "```pengu";

pub fn system_prompt(notes: &[Note], days: &[Note], current: Option<&Note>) -> String {
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
  {{"action": "schedule", "date": "YYYY-MM-DD", "time": "HH:MM or empty", "text": "what happens"}}
]
```

Rules:
- When the user dumps loose words or thoughts, sort them into a clean, well-structured note with create_note (headings, bullet lists, "- [ ] " checkboxes for to-dos).
- Anything with a day or deadline also gets a schedule action. Convert relative dates ("tomorrow", "next friday") to YYYY-MM-DD using today's date. Never schedule without a concrete date.
- Use append only with a title from the list of existing notes.
- Outside the block, reply in one or two short, friendly sentences saying what you did. Do not repeat the note contents.
- Only include the block when there is something to change. For plain questions just answer.

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
    let body = rest.find("```").map_or(rest, |e| &rest[..e]);
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

/// Apply actions: notes go to `notes`, scheduled items to the day pages in `days`.
/// Returns undo records, the id of the note to show, and the first day scheduled.
pub fn apply(
    notes: &mut Vec<Note>,
    days: &mut Vec<Note>,
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
    fn apply_and_undo() {
        let mut notes = vec![note("Welcome", "# Welcome\n\n- [x] a")];
        let mut days = vec![];
        let before = notes[0].body.clone();
        let (undo, show, day) = apply(&mut notes, &mut days, &parse(REPLY));
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
        apply(&mut notes, &mut days, &acts);
        assert!(notes.is_empty());
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].body, "- [ ] a\n- [ ] b");
    }
}
