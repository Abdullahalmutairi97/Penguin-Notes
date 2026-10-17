//! Calendar: every date has its own day page (to-dos and free notes), kept apart from the notes.
//! Month view with an agenda for the selected day, and the day page itself.
use std::sync::Arc;

use chrono::{Datelike, Local, NaiveDate};
use eframe::egui::{
    self, Align2, Color32, FontId, Galley, Key, Painter, Rect, RichText, Sense, Stroke, StrokeKind,
    Vec2, text::LayoutJob,
};

use crate::icloud::Outside;
use crate::md;
use crate::model::{self, Note};
use crate::theme::{self, Palette, bold, caps};

pub enum Hit {
    /// Open this note in the editor.
    Open(String),
    SyncNow,
}

/// Where the calendar is: the month shown, the day picked in it, and the open day page.
pub struct Cal {
    pub month: NaiveDate,
    pub sel: NaiveDate,
    pub page: Option<NaiveDate>,
    new_time: String,
    new_text: String,
}

impl Cal {
    pub fn new(today: NaiveDate) -> Self {
        Self {
            month: today.with_day(1).unwrap(),
            sel: today,
            page: None,
            new_time: String::new(),
            new_text: String::new(),
        }
    }

    pub fn select(&mut self, d: NaiveDate) {
        self.sel = d;
        self.month = d.with_day(1).unwrap();
    }

    pub fn open_page(&mut self, d: NaiveDate) {
        self.select(d);
        if self.page != Some(d) {
            self.new_time.clear();
            self.new_text.clear();
        }
        self.page = Some(d);
    }
}

pub fn daily_title(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

fn edited_on(n: &Note) -> NaiveDate {
    chrono::DateTime::from_timestamp_millis(n.updated)
        .unwrap_or_default()
        .with_timezone(&Local)
        .date_naive()
}

fn shift(month: NaiveDate, by: i32) -> NaiveDate {
    let m = month.year() * 12 + month.month0() as i32 + by;
    NaiveDate::from_ymd_opt(m.div_euclid(12), m.rem_euclid(12) as u32 + 1, 1).unwrap()
}

/// A leading "7:00" / "15:30" is the time of a to-do.
fn split_time(t: &str) -> (&str, &str) {
    let (head, rest) = t.split_once(' ').unwrap_or((t, ""));
    if minutes(head).is_some() && !rest.is_empty() {
        (head, rest.trim_start())
    } else {
        ("", t)
    }
}

fn minutes(t: &str) -> Option<u32> {
    let (h, m) = t.split_once(':')?;
    let ok = (1..=2).contains(&h.len())
        && m.len() == 2
        && h.chars().chain(m.chars()).all(|c| c.is_ascii_digit());
    if !ok {
        return None;
    }
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// "9", "930", "9:30", "21.30" -> "09:30"-style time, or None.
fn tidy_time(t: &str) -> Option<String> {
    let t = t.trim().replace('.', ":");
    if !t.chars().all(|c| c.is_ascii_digit() || c == ':') {
        return None;
    }
    let t = if t.contains(':') {
        t
    } else if t.len() <= 2 {
        format!("{t}:00")
    } else if t.len() <= 4 {
        format!("{}:{}", &t[..t.len() - 2], &t[t.len() - 2..])
    } else {
        return None;
    };
    let m = minutes(&t)?;
    Some(format!("{:02}:{:02}", m / 60, m % 60))
}

// ---- day pages

pub fn page<'a>(days: &'a [Note], d: NaiveDate) -> Option<&'a Note> {
    days.iter().find(|n| n.title == daily_title(d))
}

struct Todo {
    line: usize,
    done: bool,
    time: String,
    text: String,
}

/// The page's to-dos, timed ones first in time order, then the rest as written.
fn todos(body: &str) -> Vec<Todo> {
    let mut v: Vec<Todo> = md::checklist(body)
        .into_iter()
        .map(|(line, _, done)| {
            let raw = body.split('\n').nth(line).unwrap_or("").get(6..).unwrap_or("").trim();
            let (time, text) = split_time(raw);
            Todo { line, done, time: time.into(), text: text.into() }
        })
        .collect();
    v.sort_by_key(|t| minutes(&t.time).unwrap_or(u32::MAX));
    v
}

/// Everything on the page that is not a to-do.
fn free_text(body: &str) -> String {
    let lines: Vec<usize> = md::checklist(body).iter().map(|c| c.0).collect();
    let rest: Vec<&str> = body
        .split('\n')
        .enumerate()
        .filter(|(k, _)| !lines.contains(k))
        .map(|(_, l)| l)
        .collect();
    rest.join("\n").trim_start().to_string()
}

/// To-dos (in the order written) on top, then a blank line and the free notes.
fn compose(body: &str, free: &str) -> String {
    let lines: Vec<&str> = body.split('\n').collect();
    let todo: Vec<&str> = md::checklist(body).iter().map(|c| lines[c.0]).collect();
    match (todo.is_empty(), free.trim().is_empty()) {
        (_, true) => todo.join("\n"),
        (true, false) => free.to_string(),
        (false, false) => format!("{}\n\n{free}", todo.join("\n")),
    }
}

/// Store `body` as the page for `d`: made when it gets content, removed when emptied.
fn set_page(days: &mut Vec<Note>, d: NaiveDate, body: String) {
    let i = days.iter().position(|n| n.title == daily_title(d));
    match (i, body.trim().is_empty()) {
        (Some(i), true) => {
            days.remove(i);
        }
        (Some(i), false) => {
            days[i].body = body;
            days[i].updated = model::now_ms();
        }
        (None, true) => {}
        (None, false) => days.push(Note {
            id: model::new_id(),
            title: daily_title(d),
            body,
            tags: String::new(),
            updated: model::now_ms(),
            pinned: false,
        }),
    }
}

fn body_of(days: &[Note], d: NaiveDate) -> String {
    page(days, d).map(|n| n.body.clone()).unwrap_or_default()
}

fn toggle(days: &mut Vec<Note>, d: NaiveDate, line: usize) {
    let b = md::toggle_line(&body_of(days, d), line);
    set_page(days, d, b);
}

fn remove_line(days: &mut Vec<Note>, d: NaiveDate, line: usize) {
    let body = body_of(days, d);
    let ls: Vec<&str> = body
        .split('\n')
        .enumerate()
        .filter(|(k, _)| *k != line)
        .map(|(_, l)| l)
        .collect();
    let joined = ls.join("\n");
    let b = compose(&joined, &free_text(&joined));
    set_page(days, d, b);
}

fn add_todo(days: &mut Vec<Note>, d: NaiveDate, time: &str, text: &str) {
    let item = match tidy_time(time) {
        Some(t) => format!("- [ ] {t} {}", text.trim()),
        None => format!("- [ ] {}", text.trim()),
    };
    let body = body_of(days, d);
    let free = free_text(&body);
    let todo = compose(&body, "");
    let todo = if todo.is_empty() { item } else { format!("{todo}\n{item}") };
    set_page(days, d, compose(&todo, &free));
}

// ---- what is on a day

enum Kind<'a> {
    Task { line: usize, done: bool },
    Phone(&'a str),
    Memo,
    Edited,
}

struct Entry<'a> {
    time: String,
    text: String,
    id: &'a str,
    kind: Kind<'a>,
}

/// Everything on `date`: the page's to-dos, phone events, the page's notes, then notes edited that day.
fn day<'a>(date: NaiveDate, notes: &'a [Note], days: &'a [Note], outside: &'a [Outside]) -> Vec<Entry<'a>> {
    let pg = page(days, date);
    let mut out = Vec::new();
    if let Some(n) = pg {
        for t in todos(&n.body) {
            out.push(Entry { time: t.time, text: t.text, id: "", kind: Kind::Task { line: t.line, done: t.done } });
        }
    }
    let mut phone: Vec<&Outside> = outside.iter().filter(|o| o.date == date).collect();
    phone.sort_by_key(|o| minutes(&o.time).unwrap_or(0));
    for o in phone {
        out.push(Entry { time: o.time.clone(), text: o.text.clone(), id: "", kind: Kind::Phone(&o.calendar) });
    }
    if let Some(n) = pg {
        let free = free_text(&n.body);
        let first = free
            .lines()
            .map(|l| l.trim_start_matches(['#', '-', '*', ' ']))
            .find(|l| !l.trim().is_empty());
        if let Some(first) = first {
            out.push(Entry { time: String::new(), text: first.to_string(), id: "", kind: Kind::Memo });
        }
    }
    for n in notes.iter().filter(|n| edited_on(n) == date) {
        let text = if n.title.is_empty() { "Untitled".into() } else { n.title.clone() };
        out.push(Entry { time: String::new(), text, id: &n.id, kind: Kind::Edited });
    }
    out
}

fn one_line(painter: &Painter, text: &str, font: FontId, col: Color32, w: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.into(), font, col);
    job.wrap = egui::text::TextWrapping::truncate_at_width(w.max(1.0));
    painter.layout_job(job)
}

const PHONE: Color32 = Color32::from_rgb(0x3b, 0x5b, 0xa9);

fn rel_day(d: NaiveDate, today: NaiveDate) -> String {
    match (d - today).num_days() {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        _ => d.format("%A").to_string(),
    }
}

fn timed(time: &str, text: &str) -> String {
    if time.is_empty() { text.to_string() } else { format!("{time}  {text}") }
}

/// `outside`: read-only events from the phone's other calendars. `sync`: status line, None = sync off.
/// Edits to day pages go straight into `days`; the bool says whether any were made.
pub fn draw(
    ui: &mut egui::Ui,
    p: &Palette,
    cal: &mut Cal,
    notes: &[Note],
    days: &mut Vec<Note>,
    outside: &[Outside],
    sync: Option<&str>,
) -> (Option<Hit>, bool) {
    let before: Vec<(String, String)> = days.iter().map(|d| (d.title.clone(), d.body.clone())).collect();
    let hit = match cal.page {
        Some(d) => day_page(ui, p, cal, d, notes, days, outside, sync),
        None => month_view(ui, p, cal, notes, days, outside, sync),
    };
    let changed = days.len() != before.len()
        || days.iter().zip(&before).any(|(d, (t, b))| d.title != *t || d.body != *b);
    (hit, changed)
}

/// Header line: the controls, then the hint / sync status on the right if it fits.
/// Returns true when "Sync now" was clicked.
fn header(ui: &mut egui::Ui, p: &Palette, hint: &str, sync: Option<&str>, controls: impl FnOnce(&mut egui::Ui)) -> bool {
    let mut sync_now = false;
    egui::Frame::new()
        .inner_margin(egui::Margin { left: 20, right: 20, top: 12, bottom: 4 })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                controls(ui);
                let hint = sync.unwrap_or(hint);
                let w = ui.painter().layout_no_wrap(hint.into(), FontId::monospace(11.0), p.ink_3).size().x
                    + if sync.is_some() { 110.0 } else { 0.0 };
                // Only show the hint / sync status when it fits beside the controls.
                if ui.available_width() > w + 16.0 {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if sync.is_some() && theme::pill(ui, p, "Sync now", false).clicked() {
                            sync_now = true;
                        }
                        ui.label(RichText::new(hint).monospace().size(11.0).color(p.ink_3));
                    });
                } else if sync.is_some() && theme::pill(ui, p, "Sync now", false).clicked() {
                    sync_now = true;
                }
            });
        });
    sync_now
}

/// Give a text field a name screen readers (and tests) can find it by.
fn name(r: &egui::Response, label: &str) {
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, label));
}

fn no_focus(ui: &egui::Ui) -> bool {
    ui.memory(|m| m.focused().is_none())
}

fn month_view(
    ui: &mut egui::Ui,
    p: &Palette,
    cal: &mut Cal,
    notes: &[Note],
    days: &mut Vec<Note>,
    outside: &[Outside],
    sync: Option<&str>,
) -> Option<Hit> {
    let mut hit = None;
    let today = Local::now().date_naive();
    let month = cal.month;
    // the agenda needs room; below that a click opens the day page
    let agenda = ui.available_width() >= 820.0;
    let mut go: Option<NaiveDate> = None;
    let mut pick: Option<NaiveDate> = None;
    let mut open: Option<NaiveDate> = None;
    let hint = if agenda { "double-click a day to open its page" } else { "click a day to open its page" };
    if header(ui, p, hint, sync, |ui| {
        ui.label(RichText::new(month.format("%B %Y").to_string()).size(24.0).family(bold()));
        ui.add_space(10.0);
        if theme::pill(ui, p, "‹", false).on_hover_text("Previous month (PageUp)").clicked() {
            go = Some(shift(month, -1));
        }
        if theme::pill(ui, p, "Today", false).clicked() {
            pick = Some(today);
        }
        if theme::pill(ui, p, "›", false).on_hover_text("Next month (PageDown)").clicked() {
            go = Some(shift(month, 1));
        }
    }) {
        hit = Some(Hit::SyncNow);
    }
    if no_focus(ui) {
        if ui.input(|i| i.key_pressed(Key::PageUp)) {
            go = Some(shift(month, -1));
        } else if ui.input(|i| i.key_pressed(Key::PageDown)) {
            go = Some(shift(month, 1));
        } else if agenda && ui.input(|i| i.key_pressed(Key::Enter)) {
            open = Some(cal.sel);
        }
    }

    let full = ui.available_rect_before_wrap().shrink2(Vec2::new(20.0, 8.0));
    let agenda_w = 280.0;
    let area = if agenda {
        Rect::from_min_max(full.min, egui::pos2(full.right() - agenda_w - 14.0, full.bottom()))
    } else {
        full
    };
    let head_h = 24.0;
    let cw = area.width() / 7.0;
    // Weeks start on Sunday; show however many rows this month needs.
    let first = month.with_day(1).unwrap();
    let lead = first.weekday().num_days_from_sunday() as i64;
    let n_days = (shift(month, 1) - first).num_days();
    let rows = ((lead + n_days) as f32 / 7.0).ceil() as usize;
    let ch = ((area.height() - head_h) / rows as f32).max(60.0);
    let painter = ui.painter().clone();

    for (k, d) in ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].iter().enumerate() {
        let c = egui::pos2(area.left() + cw * (k as f32 + 0.5), area.top() + head_h / 2.0);
        let col = if k == 0 || k == 6 { p.accent2 } else { p.ink_3 };
        painter.text(c, Align2::CENTER_CENTER, d.to_uppercase(), FontId::monospace(10.5), col);
    }

    for cell in 0..rows * 7 {
        let date = first + chrono::Duration::days(cell as i64 - lead);
        let (r, c) = (cell / 7, cell % 7);
        let rect = Rect::from_min_size(
            egui::pos2(area.left() + cw * c as f32, area.top() + head_h + ch * r as f32),
            Vec2::new(cw, ch),
        )
        .shrink(3.0);
        let in_month = date.month() == month.month();
        let weekend = c == 0 || c == 6;
        let has_page = page(days, date).is_some();
        let entries = day(date, notes, days, outside);

        let resp = ui
            .interact(rect, ui.id().with(("day", date)), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, date.format("%-d %B").to_string())
        });
        let fill = if has_page {
            p.tint
        } else if !in_month {
            p.paper
        } else if weekend {
            p.paper2.lerp_to_gamma(p.paper, 0.5)
        } else {
            p.paper2
        };
        let picked = agenda && date == cal.sel;
        let (sw, stroke) = if resp.hovered() {
            (1.5, p.ink_3)
        } else if picked {
            (2.0, p.ink)
        } else if date == today {
            (1.8, p.accent)
        } else {
            (1.2, p.line)
        };
        painter.rect(rect, 8, fill, Stroke::new(sw, stroke), StrokeKind::Inside);
        if picked && resp.hovered() {
            painter.rect_stroke(rect, 8, Stroke::new(2.0, p.ink), StrokeKind::Inside);
        }

        let num_pos = rect.left_top() + Vec2::new(14.0, 13.0);
        if date == today {
            painter.circle_filled(num_pos, 11.0, p.accent);
        }
        let num_col = if date == today {
            Color32::WHITE
        } else if in_month {
            p.ink
        } else {
            p.ink_3
        };
        painter.text(num_pos, Align2::CENTER_CENTER, date.day().to_string(), FontId::new(14.0, bold()), num_col);

        let tasks: Vec<bool> = entries
            .iter()
            .filter_map(|e| match e.kind {
                Kind::Task { done, .. } => Some(done),
                _ => None,
            })
            .collect();
        if !tasks.is_empty() {
            let done = tasks.iter().filter(|d| **d).count();
            let col = if done == tasks.len() { p.accent } else { p.ink_2 };
            painter.text(
                rect.right_top() + Vec2::new(-8.0, 13.0),
                Align2::RIGHT_CENTER,
                format!("☑ {done}/{}", tasks.len()),
                FontId::monospace(10.5),
                col,
            );
        }

        // each row: marker, bold time, text cut with "…"; hover shows it in full
        let mut y = rect.top() + 30.0;
        let line_h = 17.0;
        let room = ((rect.bottom() - y - 2.0) / line_h).floor().max(0.0) as usize;
        let shown = if entries.len() > room { room.saturating_sub(1) } else { entries.len() };
        let clip = painter.with_clip_rect(rect);
        for (k, e) in entries.iter().take(shown).enumerate() {
            let row = Rect::from_min_size(egui::pos2(rect.left() + 6.0, y), Vec2::new(rect.width() - 12.0, line_h - 2.0));
            // edited notes open on click; everything else belongs to the day
            let clickable = matches!(e.kind, Kind::Edited) || !agenda;
            let sense = if clickable { Sense::click() } else { Sense::hover() };
            let full_text = timed(&e.time, &e.text);
            let tr = ui
                .interact(row, ui.id().with(("cal-entry", date, k)), sense)
                .on_hover_text(match e.kind {
                    Kind::Phone(cal) => format!("{full_text}\n{cal} calendar on your iPhone"),
                    Kind::Edited => format!("{full_text}\nnote edited this day · click to open"),
                    Kind::Memo => format!("{full_text}\nfrom the day page"),
                    Kind::Task { .. } => full_text.clone(),
                });
            if clickable && tr.hovered() {
                painter.rect_filled(row, 4, p.line);
            }
            let mark = egui::pos2(row.left() + 5.0, row.center().y);
            let dim = !in_month;
            let col = match e.kind {
                Kind::Task { done, .. } => {
                    let b = Rect::from_center_size(mark, Vec2::splat(8.0));
                    painter.rect(
                        b,
                        2,
                        if done { p.accent } else { p.paper },
                        Stroke::new(1.2, if done { p.accent } else { p.ink_2 }),
                        StrokeKind::Inside,
                    );
                    if done || dim { p.ink_3 } else { p.ink }
                }
                Kind::Phone(_) => {
                    painter.rect_filled(Rect::from_center_size(mark, egui::vec2(3.0, 11.0)), 1, PHONE);
                    if dim { p.ink_3 } else { p.ink_2 }
                }
                Kind::Memo => {
                    painter.rect_filled(Rect::from_center_size(mark, egui::vec2(7.0, 1.6)), 0, p.accent2);
                    painter.rect_filled(Rect::from_center_size(mark + Vec2::new(0.0, 3.0), egui::vec2(7.0, 1.6)), 0, p.accent2);
                    painter.rect_filled(Rect::from_center_size(mark - Vec2::new(0.0, 3.0), egui::vec2(7.0, 1.6)), 0, p.accent2);
                    if dim { p.ink_3 } else { p.ink_2 }
                }
                Kind::Edited => {
                    painter.circle_filled(mark, 2.0, p.ink_3);
                    p.ink_3
                }
            };
            let mut x = row.left() + 12.0;
            if !e.time.is_empty() {
                let tg = one_line(&painter, &e.time, FontId::new(11.5, bold()), p.accent, row.right() - x);
                let tw = tg.size().x;
                clip.galley(egui::pos2(x, row.center().y - tg.size().y / 2.0), tg, p.accent);
                x += tw + 4.0;
            }
            let size = if matches!(e.kind, Kind::Edited) { 12.0 } else { 13.0 };
            let g = one_line(&painter, &e.text, FontId::proportional(size), col, row.right() - x);
            let at = egui::pos2(x, row.center().y - g.size().y / 2.0);
            if matches!(e.kind, Kind::Task { done: true, .. }) {
                let w = g.size().x;
                clip.line_segment(
                    [at + Vec2::new(0.0, g.size().y / 2.0), at + Vec2::new(w, g.size().y / 2.0)],
                    Stroke::new(1.0, p.ink_3),
                );
            }
            clip.galley(at, g, col);
            if tr.clicked() {
                if matches!(e.kind, Kind::Edited) {
                    hit = Some(Hit::Open(e.id.to_string()));
                } else {
                    open = Some(date);
                }
            }
            y += line_h;
        }
        if entries.len() > shown {
            painter.text(
                egui::pos2(rect.left() + 18.0, y + line_h / 2.0 - 1.0),
                Align2::LEFT_CENTER,
                format!("+{} more", entries.len() - shown),
                FontId::monospace(10.5),
                p.ink_3,
            );
        }
        if resp.double_clicked() {
            open = Some(date);
        } else if resp.clicked() && hit.is_none() && open.is_none() {
            if agenda {
                pick = Some(date);
            } else {
                open = Some(date);
            }
        }
    }
    ui.allocate_rect(full, Sense::hover());

    if agenda {
        let rect = Rect::from_min_max(egui::pos2(full.right() - agenda_w, full.top()), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            let mut nav = Nav { hit: &mut hit, pick: &mut pick, open: &mut open };
            agenda_panel(ui, p, cal.sel, today, notes, days, outside, &mut nav);
        });
    }
    if let Some(m) = go {
        cal.month = m;
    }
    if let Some(d) = pick {
        cal.select(d);
    }
    if let Some(d) = open {
        cal.open_page(d);
    }
    hit
}

/// What a click in the agenda asks for.
struct Nav<'a> {
    hit: &'a mut Option<Hit>,
    pick: &'a mut Option<NaiveDate>,
    open: &'a mut Option<NaiveDate>,
}

fn section(ui: &mut egui::Ui, p: &Palette, t: &str) {
    ui.add_space(8.0);
    ui.label(caps(t, p, 10.0));
    ui.add_space(2.0);
}

fn phone_rows(ui: &mut egui::Ui, p: &Palette, entries: &[Entry]) {
    for e in entries {
        let Kind::Phone(cal) = e.kind else { continue };
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(3.0, 15.0), Sense::hover());
            ui.painter().rect_filled(r, 1, PHONE);
            ui.add(egui::Label::new(RichText::new(timed(&e.time, &e.text)).size(14.0).color(p.ink_2)).wrap())
                .on_hover_text(format!("{cal} calendar"));
        });
    }
}

/// Edited-note rows; returns the id of a clicked one.
fn edited_rows(ui: &mut egui::Ui, p: &Palette, entries: &[Entry]) -> Option<String> {
    let mut out = None;
    for e in entries.iter().filter(|e| matches!(e.kind, Kind::Edited)) {
        let r = ui
            .add(egui::Label::new(RichText::new(&e.text).size(13.5).color(p.ink_2)).truncate().sense(Sense::click()))
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Open note");
        if r.clicked() {
            out = Some(e.id.to_string());
        }
    }
    out
}

/// The selected day in short, then the next two weeks' open to-dos and phone events.
#[allow(clippy::too_many_arguments)]
fn agenda_panel(
    ui: &mut egui::Ui,
    p: &Palette,
    sel: NaiveDate,
    today: NaiveDate,
    notes: &[Note],
    days: &mut Vec<Note>,
    outside: &[Outside],
    nav: &mut Nav,
) {
    let mut toggled = None;
    egui::Frame::new()
        .fill(p.paper2)
        .stroke(Stroke::new(1.2, p.line))
        .corner_radius(10)
        .inner_margin(14)
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            let has_page = page(days, sel).is_some();
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(RichText::new(rel_day(sel, today)).size(21.0).family(bold()));
                    ui.label(RichText::new(sel.format("%A, %B %-d").to_string()).size(13.0).color(p.ink_3));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    if theme::pill(ui, p, "Open day", !has_page).on_hover_text("Open this day's page (Enter)").clicked() {
                        *nav.open = Some(sel);
                    }
                });
            });
            ui.add_space(4.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let entries = day(sel, notes, days, outside);
                let tasks: Vec<&Entry> = entries.iter().filter(|e| matches!(e.kind, Kind::Task { .. })).collect();
                if !tasks.is_empty() {
                    let done = tasks.iter().filter(|e| matches!(e.kind, Kind::Task { done: true, .. })).count();
                    section(ui, p, &format!("To-dos · {done}/{}", tasks.len()));
                    for e in tasks {
                        let Kind::Task { line, done } = e.kind else { continue };
                        let mut d = done;
                        let mut t = RichText::new(timed(&e.time, &e.text)).size(14.0).color(if done { p.ink_3 } else { p.ink });
                        if done {
                            t = t.strikethrough();
                        }
                        if ui.checkbox(&mut d, t).changed() {
                            toggled = Some(line);
                        }
                    }
                }
                if entries.iter().any(|e| matches!(e.kind, Kind::Phone(_))) {
                    section(ui, p, "iPhone calendars");
                    phone_rows(ui, p, &entries);
                }
                if let Some(n) = page(days, sel) {
                    let free = free_text(&n.body);
                    if !free.trim().is_empty() {
                        section(ui, p, "Day notes");
                        let short: String = free.trim_end().lines().take(4).collect::<Vec<_>>().join("\n");
                        let r = ui
                            .add(egui::Label::new(RichText::new(short).size(13.5).color(p.ink_2)).wrap().sense(Sense::click()))
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Open the day page");
                        if r.clicked() {
                            *nav.open = Some(sel);
                        }
                    }
                }
                if entries.iter().any(|e| matches!(e.kind, Kind::Edited)) {
                    section(ui, p, "Notes edited");
                    if let Some(id) = edited_rows(ui, p, &entries) {
                        *nav.hit = Some(Hit::Open(id));
                    }
                }
                if entries.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Nothing planned.").size(14.0).color(p.ink_3));
                }
                coming_up(ui, p, sel, notes, days, outside, nav.pick);
            });
        });
    if let Some(line) = toggled {
        toggle(days, sel, line);
    }
}

/// Open to-dos and phone events of the 14 days after `from`; a click picks that day.
fn coming_up(
    ui: &mut egui::Ui,
    p: &Palette,
    from: NaiveDate,
    notes: &[Note],
    days: &[Note],
    outside: &[Outside],
    pick: &mut Option<NaiveDate>,
) {
    let mut next: Vec<(NaiveDate, String)> = Vec::new();
    for k in 1..=14 {
        let d = from + chrono::Duration::days(k);
        for e in day(d, notes, days, outside) {
            if matches!(e.kind, Kind::Task { done: false, .. } | Kind::Phone(_)) {
                next.push((d, timed(&e.time, &e.text)));
            }
        }
    }
    if next.is_empty() {
        return;
    }
    ui.add_space(8.0);
    section(ui, p, "Coming up");
    for (d, t) in next.iter().take(10) {
        let r = ui
            .horizontal(|ui| {
                ui.add_sized(
                    [52.0, 16.0],
                    egui::Label::new(RichText::new(d.format("%a %-d").to_string()).monospace().size(10.5).color(p.accent2)),
                );
                ui.add(egui::Label::new(RichText::new(t).size(13.5)).truncate());
            })
            .response
            .interact(Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(d.format("%A, %B %-d").to_string());
        if r.clicked() {
            *pick = Some(*d);
        }
    }
    if next.len() > 10 {
        ui.label(RichText::new(format!("+{} more", next.len() - 10)).monospace().size(10.5).color(p.ink_3));
    }
}

/// One date's own page: its to-dos (tick, remove, add with a time) and free notes, and on the
/// side the week around it, phone events, notes edited that day and what is coming up.
#[allow(clippy::too_many_arguments)]
fn day_page(
    ui: &mut egui::Ui,
    p: &Palette,
    cal: &mut Cal,
    d: NaiveDate,
    notes: &[Note],
    days: &mut Vec<Note>,
    outside: &[Outside],
    sync: Option<&str>,
) -> Option<Hit> {
    let mut hit = None;
    let today = Local::now().date_naive();
    let mut back = false;
    let mut go: Option<NaiveDate> = None;
    if header(ui, p, "PageUp/PageDown: other days · Esc: month", sync, |ui| {
        if theme::pill(ui, p, "‹ Month", false).on_hover_text("Back to the month (Esc)").clicked() {
            back = true;
        }
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(RichText::new(d.format("%A, %B %-d").to_string()).size(24.0).family(bold()));
            let sub = if d == today { "Today".to_string() } else { format!("{} · {}", rel_day(d, today), d.year()) };
            ui.label(RichText::new(sub).monospace().size(11.0).color(p.accent2));
        });
        ui.add_space(10.0);
        if theme::pill(ui, p, "‹", false).on_hover_text("Previous day (PageUp)").clicked() {
            go = Some(d - chrono::Duration::days(1));
        }
        if theme::pill(ui, p, "Today", d == today).clicked() {
            go = Some(today);
        }
        if theme::pill(ui, p, "›", false).on_hover_text("Next day (PageDown)").clicked() {
            go = Some(d + chrono::Duration::days(1));
        }
    }) {
        hit = Some(Hit::SyncNow);
    }
    if ui.input(|i| i.key_pressed(Key::Escape)) && no_focus(ui) {
        back = true;
    }
    if no_focus(ui) {
        if ui.input(|i| i.key_pressed(Key::PageUp)) {
            go = Some(d - chrono::Duration::days(1));
        } else if ui.input(|i| i.key_pressed(Key::PageDown)) {
            go = Some(d + chrono::Duration::days(1));
        }
    }

    let full = ui.available_rect_before_wrap().shrink2(Vec2::new(20.0, 8.0));
    let side_w = if full.width() >= 760.0 { 290.0 } else { 0.0 };
    let main_right = if side_w > 0.0 { full.right() - side_w - 14.0 } else { full.right() };
    let main = Rect::from_min_max(full.min, egui::pos2(main_right, full.bottom()));
    ui.allocate_rect(full, Sense::hover());

    let body = body_of(days, d);
    let mut new_body: Option<String> = None;
    let mut toggled = None;
    let mut removed = None;
    let mut add = false;

    ui.scope_builder(egui::UiBuilder::new().max_rect(main), |ui| {
        egui::Frame::new()
            .fill(p.paper2)
            .stroke(Stroke::new(1.2, p.line))
            .corner_radius(10)
            .inner_margin(18)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                egui::ScrollArea::vertical().id_salt("day-main").auto_shrink([false, false]).show(ui, |ui| {
                    let list = todos(&body);
                    let done = list.iter().filter(|t| t.done).count();
                    let head = if list.is_empty() { "To-dos".to_string() } else { format!("To-dos · {done}/{}", list.len()) };
                    ui.label(caps(&head, p, 11.0));
                    ui.add_space(6.0);
                    for t in &list {
                        ui.horizontal(|ui| {
                            let mut v = t.done;
                            let mut rt = RichText::new(timed(&t.time, &t.text)).size(16.0).color(if t.done { p.ink_3 } else { p.ink });
                            if t.done {
                                rt = rt.strikethrough();
                            }
                            if ui.checkbox(&mut v, rt).changed() {
                                toggled = Some(t.line);
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let x = ui
                                    .add(egui::Button::new(RichText::new("×").size(17.0).color(p.ink_3)).frame(false))
                                    .on_hover_text("Remove");
                                let name = format!("Remove “{}”", t.text);
                                x.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
                                if x.clicked() {
                                    removed = Some(t.line);
                                }
                            });
                        });
                    }
                    if list.is_empty() {
                        ui.label(RichText::new("Nothing to do yet.").size(14.0).color(p.ink_3));
                    }
                    ui.add_space(8.0);
                    // add a to-do: optional time, then the text; Enter adds
                    ui.horizontal(|ui| {
                        let tr = ui.add(
                            egui::TextEdit::singleline(&mut cal.new_time)
                                .id(egui::Id::new(("day-new-time", d)))
                                .hint_text("time")
                                .desired_width(52.0),
                        );
                        name(&tr, "To-do time");
                        let w = (ui.available_width() - 70.0).max(80.0);
                        let xr = ui.add(
                            egui::TextEdit::singleline(&mut cal.new_text)
                                .id(egui::Id::new(("day-new-todo", d)))
                                .hint_text("Add a to-do…")
                                .desired_width(w),
                        );
                        name(&xr, "New to-do");
                        let enter = (tr.lost_focus() || xr.lost_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                        let ready = !cal.new_text.trim().is_empty();
                        if (theme::pill(ui, p, "Add", ready).clicked() || enter) && ready {
                            add = true;
                            if enter {
                                xr.request_focus();
                            }
                        }
                    });
                    if !cal.new_time.trim().is_empty() && tidy_time(&cal.new_time).is_none() {
                        ui.label(RichText::new("Times look like 9:30 or 15:00 (or leave it empty)").size(12.0).color(p.accent2));
                    }

                    ui.add_space(18.0);
                    ui.label(caps("Notes for the day", p, 11.0));
                    ui.add_space(6.0);
                    let mut free = free_text(&body);
                    let out = ui.add(
                        egui::TextEdit::multiline(&mut free)
                            .id(egui::Id::new(("day-notes", d)))
                            .hint_text("Anything about this day…")
                            .font(FontId::proportional(15.0))
                            .desired_width(f32::INFINITY)
                            .desired_rows(8)
                            .margin(egui::Margin::symmetric(10, 8)),
                    );
                    name(&out, "Day notes");
                    if out.changed() {
                        new_body = Some(compose(&body, &free));
                    }
                });
            });
    });

    if side_w > 0.0 {
        let rect = Rect::from_min_max(egui::pos2(full.right() - side_w, full.top()), full.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
            egui::Frame::new()
                .fill(p.paper2)
                .stroke(Stroke::new(1.2, p.line))
                .corner_radius(10)
                .inner_margin(14)
                .show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    egui::ScrollArea::vertical().id_salt("day-side").auto_shrink([false, false]).show(ui, |ui| {
                        // the week around this day
                        ui.label(caps("This week", p, 10.0));
                        ui.add_space(4.0);
                        let start = d - chrono::Duration::days(d.weekday().num_days_from_sunday() as i64);
                        for k in 0..7 {
                            let w = start + chrono::Duration::days(k);
                            let es = day(w, notes, days, outside);
                            let open = es.iter().filter(|e| matches!(e.kind, Kind::Task { done: false, .. })).count();
                            let phone = es.iter().filter(|e| matches!(e.kind, Kind::Phone(_))).count();
                            let mut info = Vec::new();
                            if open > 0 {
                                info.push(format!("{open} to-do{}", if open == 1 { "" } else { "s" }));
                            }
                            if phone > 0 {
                                info.push(format!("{phone} event{}", if phone == 1 { "" } else { "s" }));
                            }
                            if info.is_empty() && page(days, w).is_some() {
                                info.push("notes".into());
                            }
                            let r = ui
                                .horizontal(|ui| {
                                    let col = if w == d { p.accent } else if w == today { p.accent2 } else { p.ink };
                                    let mut t = RichText::new(w.format("%a %-d").to_string()).monospace().size(12.0).color(col);
                                    if w == d {
                                        t = t.strong();
                                    }
                                    ui.add_sized([60.0, 18.0], egui::Label::new(t));
                                    ui.label(RichText::new(info.join(" · ")).size(13.0).color(p.ink_3));
                                })
                                .response
                                .interact(Sense::click())
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(w.format("%A, %B %-d").to_string());
                            if r.clicked() {
                                go = Some(w);
                            }
                        }

                        let es = day(d, notes, days, outside);
                        if es.iter().any(|e| matches!(e.kind, Kind::Phone(_))) {
                            ui.add_space(6.0);
                            section(ui, p, "iPhone calendars");
                            phone_rows(ui, p, &es);
                        }
                        if es.iter().any(|e| matches!(e.kind, Kind::Edited)) {
                            ui.add_space(6.0);
                            section(ui, p, "Notes edited this day");
                            if let Some(id) = edited_rows(ui, p, &es) {
                                hit = Some(Hit::Open(id));
                            }
                        }
                        let mut pick = None;
                        coming_up(ui, p, d, notes, days, outside, &mut pick);
                        if pick.is_some() {
                            go = pick;
                        }
                    });
                });
        });
    }

    if let Some(b) = new_body {
        set_page(days, d, b);
    }
    if let Some(line) = toggled {
        toggle(days, d, line);
    }
    if let Some(line) = removed {
        remove_line(days, d, line);
    }
    if add {
        add_todo(days, d, &cal.new_time, &cal.new_text);
        cal.new_text.clear();
        cal.new_time.clear();
    }
    if back {
        cal.page = None;
    } else if let Some(g) = go {
        cal.open_page(g);
    }
    hit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn month_shift_wraps_years() {
        let d = NaiveDate::from_ymd_opt(2026, 12, 1).unwrap();
        assert_eq!(shift(d, 1), NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
        assert_eq!(shift(d, -12), NaiveDate::from_ymd_opt(2025, 12, 1).unwrap());
    }

    #[test]
    fn times_split_off_todos() {
        assert_eq!(split_time("15:00 Dentist"), ("15:00", "Dentist"));
        assert_eq!(split_time("7:30 Gym"), ("7:30", "Gym"));
        assert_eq!(split_time("Buy 2:30 tickets"), ("", "Buy 2:30 tickets"));
        assert_eq!(split_time("15:00"), ("", "15:00"));
        assert_eq!(tidy_time("9"), Some("09:00".into()));
        assert_eq!(tidy_time("930"), Some("09:30".into()));
        assert_eq!(tidy_time("21.15"), Some("21:15".into()));
        assert_eq!(tidy_time("25:00"), None);
        assert_eq!(tidy_time("noon"), None);
    }

    #[test]
    fn day_pages_come_and_go_with_content() {
        let d = NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
        let mut days = vec![];
        add_todo(&mut days, d, "15", "Dentist");
        add_todo(&mut days, d, "", "Milk");
        add_todo(&mut days, d, "7:00", "Gym");
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].body, "- [ ] 15:00 Dentist\n- [ ] Milk\n- [ ] 07:00 Gym");
        let order: Vec<String> = todos(&days[0].body).into_iter().map(|t| t.text).collect();
        assert_eq!(order, ["Gym", "Dentist", "Milk"], "timed first, by time");

        let b = compose(&days[0].body, "bring the card\n");
        set_page(&mut days, d, b);
        assert_eq!(free_text(&days[0].body), "bring the card\n", "typing keeps its newline");
        add_todo(&mut days, d, "", "Call mum");
        assert_eq!(
            days[0].body,
            "- [ ] 15:00 Dentist\n- [ ] Milk\n- [ ] 07:00 Gym\n- [ ] Call mum\n\nbring the card\n"
        );

        toggle(&mut days, d, 1);
        assert!(days[0].body.contains("- [x] Milk"));
        for _ in 0..4 {
            remove_line(&mut days, d, 0);
        }
        assert_eq!(days[0].body, "bring the card\n");
        let b = compose(&days[0].body, "");
        set_page(&mut days, d, b);
        assert!(days.is_empty(), "an emptied page goes away");
    }
}
