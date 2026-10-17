//! Minimal iCalendar (RFC 5545) for calendar sync: daily-note to-dos <-> VEVENTs.
use chrono::{Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

/// One calendar entry as Penguin Notes sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub uid: String,
    pub date: NaiveDate,
    /// Local "HH:MM", or empty for all-day.
    pub time: String,
    pub text: String,
    pub done: bool,
}

const DONE: &str = "✓ ";

impl Event {
    /// The daily-note checklist line for this event.
    pub fn line(&self) -> String {
        let check = if self.done { "x" } else { " " };
        if self.time.is_empty() {
            format!("- [{check}] {}", self.text)
        } else {
            format!("- [{check}] {} {}", self.time, self.text)
        }
    }

    /// Parse a daily-note checklist line ("- [ ] 15:00 Dentist").
    pub fn from_line(date: NaiveDate, line: &str) -> Option<Event> {
        let rest = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))?;
        let done = match rest.get(..4)?.to_lowercase().as_str() {
            "[ ] " => false,
            "[x] " => true,
            _ => return None,
        };
        let rest = rest[4..].trim();
        let (time, text) = match rest.split_once(' ') {
            Some((t, x)) if NaiveTime::parse_from_str(t, "%H:%M").is_ok() => {
                (t.to_string(), x.trim().to_string())
            }
            _ => (String::new(), rest.to_string()),
        };
        (!text.is_empty()).then(|| Event {
            uid: String::new(),
            date,
            time,
            text,
            done,
        })
    }

    pub fn to_ics(&self) -> String {
        let esc = |s: &str| {
            s.replace('\\', "\\\\")
                .replace(';', "\\;")
                .replace(',', "\\,")
                .replace('\n', "\\n")
        };
        let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
        let (start, end) = match NaiveTime::parse_from_str(&self.time, "%H:%M") {
            Ok(t) => {
                let local = Local
                    .from_local_datetime(&self.date.and_time(t))
                    .earliest()
                    .unwrap_or_else(|| Local::now());
                let s = local.with_timezone(&Utc);
                let e = s + chrono::Duration::hours(1);
                (
                    format!("DTSTART:{}", s.format("%Y%m%dT%H%M%SZ")),
                    format!("DTEND:{}", e.format("%Y%m%dT%H%M%SZ")),
                )
            }
            Err(_) => (
                format!("DTSTART;VALUE=DATE:{}", self.date.format("%Y%m%d")),
                format!(
                    "DTEND;VALUE=DATE:{}",
                    (self.date + chrono::Duration::days(1)).format("%Y%m%d")
                ),
            ),
        };
        let summary = if self.done {
            format!("{DONE}{}", self.text)
        } else {
            self.text.clone()
        };
        [
            "BEGIN:VCALENDAR",
            "VERSION:2.0",
            "PRODID:-//Penguin Notes//EN",
            "BEGIN:VEVENT",
            &format!("UID:{}", self.uid),
            &format!("DTSTAMP:{stamp}"),
            &start,
            &end,
            &format!("SUMMARY:{}", esc(&summary)),
            "END:VEVENT",
            "END:VCALENDAR",
            "",
        ]
        .join("\r\n")
    }
}

/// Unfold continuation lines and split into (name-with-params, value).
fn props(ics: &str) -> Vec<(String, String)> {
    let mut lines: Vec<String> = Vec::new();
    for raw in ics.split('\n') {
        let raw = raw.trim_end_matches('\r');
        if let (Some(last), true) = (
            lines.last_mut(),
            raw.starts_with(' ') || raw.starts_with('\t'),
        ) {
            last.push_str(&raw[1..]);
        } else {
            lines.push(raw.to_string());
        }
    }
    lines
        .into_iter()
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

fn unescape(s: &str) -> String {
    s.replace("\\n", "\n")
        .replace("\\N", "\n")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
}

/// Turn a DTSTART into a local date and "HH:MM" ("" = all-day).
fn when(key: &str, val: &str) -> Option<(NaiveDate, String)> {
    let val = val.trim();
    if key.contains("VALUE=DATE") && !key.contains("VALUE=DATE-TIME") || val.len() == 8 {
        return NaiveDate::parse_from_str(val, "%Y%m%d")
            .ok()
            .map(|d| (d, String::new()));
    }
    if let Some(v) = val.strip_suffix('Z') {
        let dt = NaiveDateTime::parse_from_str(v, "%Y%m%dT%H%M%S").ok()?;
        let local = Utc.from_utc_datetime(&dt).with_timezone(&Local);
        return Some((local.date_naive(), local.format("%H:%M").to_string()));
    }
    // TZID=... or floating: show the wall-clock time as written.
    let dt = NaiveDateTime::parse_from_str(val, "%Y%m%dT%H%M%S").ok()?;
    Some((dt.date(), dt.format("%H:%M").to_string()))
}

/// Every VEVENT in a calendar object (expanded recurrences come back as several).
pub fn parse(ics: &str) -> Vec<Event> {
    let mut out = Vec::new();
    let mut cur: Option<(String, Option<(NaiveDate, String)>, String)> = None;
    for (k, v) in props(ics) {
        let name = k.split(';').next().unwrap_or("").to_ascii_uppercase();
        match (name.as_str(), v.as_str()) {
            ("BEGIN", "VEVENT") => cur = Some((String::new(), None, String::new())),
            ("END", "VEVENT") => {
                if let Some((uid, Some((date, time)), summary)) = cur.take() {
                    let summary = unescape(&summary);
                    let (done, text) = match summary.strip_prefix(DONE) {
                        Some(t) => (true, t.to_string()),
                        None => (false, summary),
                    };
                    out.push(Event {
                        uid,
                        date,
                        time,
                        text: text.replace('\n', " "),
                        done,
                    });
                }
            }
            ("UID", _) => {
                if let Some(c) = cur.as_mut() {
                    c.0 = v.trim().to_string();
                }
            }
            ("DTSTART", _) => {
                if let Some(c) = cur.as_mut() {
                    c.1 = when(&k, &v);
                }
            }
            ("SUMMARY", _) => {
                if let Some(c) = cur.as_mut() {
                    c.2 = v.trim().to_string();
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn line_round_trip() {
        let e = Event::from_line(d("2026-10-20"), "- [x] 15:00 Dentist, bring card").unwrap();
        assert_eq!(
            (e.time.as_str(), e.text.as_str(), e.done),
            ("15:00", "Dentist, bring card", true)
        );
        assert_eq!(e.line(), "- [x] 15:00 Dentist, bring card");
        assert!(Event::from_line(d("2026-10-20"), "- plain bullet").is_none());
    }

    #[test]
    fn ics_round_trip_timed_and_all_day() {
        for line in ["- [ ] 15:00 Dentist, bring card", "- [x] Pay rent; on time"] {
            let mut e = Event::from_line(d("2026-10-20"), line).unwrap();
            e.uid = "u1".into();
            let back = parse(&e.to_ics());
            assert_eq!(back, vec![e]);
        }
    }

    #[test]
    fn parses_folded_tzid_and_multiple() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART;TZID=Asia/Riyadh:20261021T090000\r\nSUMMARY:Long meet\r\n ing\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART;VALUE=DATE:20261028\r\nSUMMARY:Next\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let ev = parse(ics);
        assert_eq!(ev.len(), 2);
        assert_eq!(
            (ev[0].time.as_str(), ev[0].text.as_str()),
            ("09:00", "Long meeting")
        );
        assert_eq!((ev[1].date, ev[1].time.as_str()), (d("2026-10-28"), ""));
    }
}
