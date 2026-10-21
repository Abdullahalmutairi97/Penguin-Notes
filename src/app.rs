use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, Key, Layout as ELayout, Modifiers, RichText, Sense, Stroke, UiBuilder,
    Vec2,
};

mod sync;

use crate::ai::{self, Ev};
use crate::calendar;
use crate::edit;
use crate::md::{self, Action, Block};
use crate::model::{self, AiCfg, Note, PRESETS, Settings};
use crate::pengu;
use crate::theme::{self, Deco, Palette, bold, caps, pill, pill_ex, primary};
use crate::voice;

#[derive(PartialEq, Clone, Copy, Debug)]
enum View {
    Edit,
    Split,
    Preview,
}

#[derive(PartialEq, Clone, Copy)]
enum Conn {
    None,
    Testing,
    Ok,
    Fail,
}

#[derive(PartialEq, Clone, Copy)]
enum ActState {
    Pending,
    Applied,
    Undone,
}

struct Msg {
    role: &'static str,
    content: String,
    label: String,
    acts: Vec<pengu::Act>,
    undo: Vec<pengu::Undo>,
    /// The trash before this reply was applied, if it removed or restored calendar items.
    trash_before: Option<Vec<pengu::Deleted>>,
    state: ActState,
}

impl Msg {
    fn new(role: &'static str, content: String, label: String) -> Self {
        Self {
            role,
            content,
            label,
            acts: vec![],
            undo: vec![],
            trash_before: None,
            state: ActState::Pending,
        }
    }
}

type Op = Box<dyn Fn(&str, usize, usize) -> edit::Edit>;

pub struct App {
    notes: Vec<Note>,
    /// one page per calendar date, kept apart from the notes
    days: Vec<Note>,
    active: Option<String>,
    query: String,
    view: View,
    dark: bool,
    applied_dark: Option<bool>,
    saved: String,
    calendar: bool,
    cal: calendar::Cal,
    /// last note removed with the trash icon: (list position, note, when) for Undo
    trashed: Option<(usize, Note, Instant)>,
    ai_open: bool,
    settings_open: bool,
    confirm_delete: bool,
    ai: AiCfg,
    models: Vec<String>,
    conn: Conn,
    conn_msg: String,
    msgs: Vec<Msg>,
    draft: String,
    busy: bool,
    err: String,
    use_ctx: bool,
    pengu_edits: bool,
    actions_open: bool,
    sidebar_open: bool,
    topbar_open: bool,
    voice_lang: String,
    /// Last known editor selection (char indices); `None` = end of note.
    sel: Option<(usize, usize)>,
    pending_sel: Option<(usize, usize)>,
    refocus: bool,
    blocks: (String, Vec<Block>),
    chat_rx: Option<Receiver<Ev>>,
    model_rx: Option<Receiver<Ev>>,
    cancel: Arc<AtomicBool>,
    dirty: bool,
    last_save: Instant,
    sync: sync::SyncRt,
    voice: voice::Voice,
    /// Calendar items Pengu removed, restorable for a day.
    trash: Vec<pengu::Deleted>,
    /// The notes and day files as last saved or read here, to notice the widget's changes.
    seen: (Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    /// When the files on disk last matched what is in memory.
    agreed: i64,
    checked_disk: Instant,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self::with_days(model::load_notes(), model::load_days(), model::load_settings())
    }

    #[cfg(test)]
    fn from_parts(notes: Vec<Note>, s: Settings) -> Self {
        Self::with_days(notes, vec![], s)
    }

    fn with_days(mut notes: Vec<Note>, mut days: Vec<Note>, s: Settings) -> Self {
        // Old "YYYY-MM-DD" daily notes become day pages; save that right away.
        let moved = model::move_daily_notes(&mut notes, &mut days);
        if moved {
            model::save_days(&days);
            model::save_notes(&notes);
        }
        Self {
            active: notes.first().map(|n| n.id.clone()),
            notes,
            days,
            query: String::new(),
            view: View::Edit,
            dark: s.dark,
            applied_dark: None,
            saved: String::new(),
            calendar: false,
            cal: calendar::Cal::new(chrono::Local::now().date_naive()),
            trashed: None,
            ai_open: false,
            settings_open: false,
            confirm_delete: false,
            ai: s.ai,
            models: vec![],
            conn: Conn::None,
            conn_msg: String::new(),
            msgs: vec![],
            draft: String::new(),
            busy: false,
            err: String::new(),
            use_ctx: true,
            pengu_edits: s.pengu_edits,
            actions_open: s.actions_open,
            sidebar_open: s.sidebar_open,
            topbar_open: s.topbar_open,
            voice_lang: if cfg!(test) { "en".into() } else { voice::lang() },
            sel: None,
            pending_sel: None,
            refocus: false,
            blocks: (String::new(), vec![]),
            chat_rx: None,
            model_rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            dirty: false,
            last_save: Instant::now(),
            sync: sync::SyncRt::load(),
            voice: voice::Voice::default(),
            trash: if cfg!(test) { vec![] } else { pengu::load_trash() },
            seen: (model::mtime("notes.json"), model::mtime("days.json")),
            agreed: model::now_ms(),
            checked_disk: Instant::now(),
        }
    }

    fn idx(&self) -> Option<usize> {
        let id = self.active.as_ref()?;
        self.notes.iter().position(|n| &n.id == id)
    }

    fn open(&mut self, id: String) {
        if self.active.as_deref() != Some(id.as_str()) {
            self.sel = None;
            self.pending_sel = None;
        }
        self.active = Some(id);
        self.calendar = false;
    }

    fn touch(&mut self, i: usize) {
        self.notes[i].updated = model::now_ms();
        self.dirty = true;
        self.saved = format!("Saved {}", chrono::Local::now().format("%H:%M"));
    }

    fn save_settings(&self) {
        model::save_settings(&Settings {
            dark: self.dark,
            ai: self.ai.clone(),
            pengu_edits: self.pengu_edits,
            actions_open: self.actions_open,
            sidebar_open: self.sidebar_open,
            topbar_open: self.topbar_open,
        });
    }

    fn add_note(&mut self, title: &str, body: &str) {
        let n = Note {
            id: model::new_id(),
            title: title.into(),
            body: body.into(),
            tags: String::new(),
            updated: model::now_ms(),
            pinned: false,
        };
        let id = n.id.clone();
        self.notes.insert(0, n);
        self.open(id);
        self.query.clear();
        self.view = View::Edit;
        self.refocus = true;
        self.dirty = true;
    }

    fn open_by_title(&mut self, t: &str) {
        if let Some(n) = self
            .notes
            .iter()
            .find(|n| n.title.to_lowercase() == t.to_lowercase())
        {
            let id = n.id.clone();
            self.open(id);
        } else {
            self.add_note(t, &format!("# {t}\n\n"));
        }
    }

    /// Trash-icon delete: no dialog, but Undo brings it back for a few seconds.
    fn trash_note(&mut self, id: &str) {
        let Some(i) = self.notes.iter().position(|n| n.id == id) else { return };
        let n = self.notes.remove(i);
        if self.active.as_deref() == Some(id) {
            match self.notes.first().map(|n| n.id.clone()) {
                Some(f) => self.open(f),
                None => self.active = None,
            }
        }
        self.trashed = Some((i, n, Instant::now()));
        self.dirty = true;
    }

    fn undo_trash(&mut self) {
        if let Some((i, n, _)) = self.trashed.take() {
            let id = n.id.clone();
            self.notes.insert(i.min(self.notes.len()), n);
            self.open(id);
            self.dirty = true;
        }
    }

    fn delete_active(&mut self) {
        if let Some(i) = self.idx() {
            self.notes.remove(i);
            if let Some(id) = self.notes.first().map(|n| n.id.clone()) {
                self.open(id);
            } else {
                self.active = None;
            }
            self.dirty = true;
        }
    }

    /// Apply a toolbar edit at the editor selection, then restore the selection.
    fn apply(&mut self, f: impl FnOnce(&str, usize, usize) -> edit::Edit) {
        let Some(i) = self.idx() else { return };
        let end = self.notes[i].body.chars().count();
        let (s, e) = match (self.view, self.sel) {
            (View::Preview, _) | (_, None) => (end, end),
            (_, Some(r)) => r,
        };
        let r = f(&self.notes[i].body, s, e);
        self.notes[i].body = r.text;
        self.touch(i);
        if self.view == View::Preview {
            self.view = View::Edit;
        }
        self.pending_sel = Some(r.sel);
        self.sel = Some(r.sel);
        self.refocus = true;
    }

    fn selection_text(&self) -> String {
        let Some(i) = self.idx() else {
            return String::new();
        };
        let body = &self.notes[i].body;
        match self.sel {
            Some((s, e)) if self.view != View::Preview && e > s => {
                body.chars().skip(s).take(e - s).collect()
            }
            _ => body.clone(),
        }
    }

    fn send(&mut self, ctx: &egui::Context, content: String, label: String) {
        if self.busy {
            return;
        }
        if self.ai.model.trim().is_empty() {
            self.err = "Pick a model in Pengu settings first.".into();
            self.settings_open = true;
            return;
        }
        let current = self.idx().filter(|_| self.use_ctx).map(|i| &self.notes[i]);
        if pengu::purge(&mut self.trash) {
            pengu::save_trash(&self.trash);
        }
        let sys = pengu::system_prompt(&self.notes, &self.days, current, &self.trash);
        self.msgs.push(Msg::new("user", content, label));
        let mut m = vec![("system".to_string(), sys)];
        m.extend(
            self.msgs
                .iter()
                .map(|x| (x.role.to_string(), x.content.clone())),
        );
        self.msgs
            .push(Msg::new("assistant", String::new(), String::new()));
        self.busy = true;
        self.err.clear();
        self.draft.clear();
        self.cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        self.chat_rx = Some(rx);
        ai::chat(self.ai.clone(), m, tx, self.cancel.clone(), ctx.clone());
    }

    /// Apply the actions in Pengu's reply `k` to the notes.
    fn pengu_apply(&mut self, k: usize) {
        let before = self.trash.clone();
        let (undo, show, day) = pengu::apply(&mut self.notes, &mut self.days, &mut self.trash, &self.msgs[k].acts);
        self.msgs[k].undo = undo;
        if self.trash != before {
            self.msgs[k].trash_before = Some(before);
            pengu::save_trash(&self.trash);
        }
        self.msgs[k].state = ActState::Applied;
        self.dirty = true;
        self.saved = format!("Pengu saved {}", chrono::Local::now().format("%H:%M"));
        // A new note opens; dates alone show their day (in the month if that is open).
        match (show, day) {
            (Some(id), _) if !self.calendar => self.open(id),
            (_, Some(d)) if self.calendar && self.cal.page.is_none() => self.cal.select(d),
            (_, Some(d)) => {
                self.cal.open_page(d);
                self.calendar = true;
            }
            _ => {}
        }
    }

    fn pengu_undo(&mut self, k: usize) {
        pengu::revert(&mut self.notes, &mut self.days, &self.msgs[k].undo);
        if let Some(before) = self.msgs[k].trash_before.take() {
            pengu::undo_trash(&mut self.days, &mut self.trash, &before);
            pengu::save_trash(&self.trash);
        }
        self.msgs[k].undo.clear();
        self.msgs[k].state = ActState::Undone;
        self.dirty = true;
        if self.idx().is_none() {
            self.active = self.notes.first().map(|n| n.id.clone());
        }
    }

    fn stop_chat(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.chat_rx = None;
        self.finish_chat();
    }

    fn finish_chat(&mut self) {
        self.busy = false;
        if self
            .msgs
            .last()
            .is_some_and(|m| m.role == "assistant" && m.content.is_empty())
        {
            self.msgs.pop();
        }
    }

    fn test_connection(&mut self, ctx: &egui::Context) {
        self.conn = Conn::Testing;
        self.conn_msg = "Connecting…".into();
        self.models.clear();
        let (tx, rx) = channel();
        self.model_rx = Some(rx);
        ai::list_models(self.ai.clone(), tx, ctx.clone());
    }

    fn poll(&mut self) {
        let mut done = false;
        if let Some(rx) = &self.chat_rx {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    Ev::Chunk(c) => {
                        if let Some(m) = self.msgs.last_mut() {
                            m.content += &c;
                        }
                    }
                    Ev::Done => done = true,
                    Ev::Err(e) => {
                        self.err = e;
                        done = true;
                    }
                    Ev::Models(_) => {}
                }
            }
        }
        if done {
            self.chat_rx = None;
            self.finish_chat();
            let k = self.msgs.len().wrapping_sub(1);
            if self.err.is_empty() && self.msgs.last().is_some_and(|m| m.role == "assistant") {
                self.msgs[k].acts = pengu::parse(&self.msgs[k].content);
                if !self.msgs[k].acts.is_empty() && self.pengu_edits {
                    self.pengu_apply(k);
                }
            }
        }
        let Some(Ok(Ev::Models(r))) = self.model_rx.as_ref().map(|rx| rx.try_recv()) else {
            return;
        };
        self.model_rx = None;
        match r {
            Ok(list) => {
                self.conn = Conn::Ok;
                self.conn_msg = format!("Connected · {} models", list.len());
                if let (true, Some(f)) = (self.ai.model.is_empty(), list.first()) {
                    self.ai.model = f.clone();
                    self.save_settings();
                }
                self.models = list;
            }
            Err(e) => {
                self.conn = Conn::Fail;
                self.conn_msg = format!("Failed: {e}");
            }
        }
    }

    /// Merge in what the desktop widget's Pengu saved while this window was open.
    fn pick_up_outside_changes(&mut self) {
        self.checked_disk = Instant::now();
        // Tests run side by side over one temp data dir; they would pick up each other's saves.
        if cfg!(test) {
            return;
        }
        // The widget may have switched the Talk language.
        self.voice_lang = voice::lang();
        let now = (model::mtime("notes.json"), model::mtime("days.json"));
        if now == self.seen {
            return;
        }
        let mut changed = false;
        if now.0 != self.seen.0 {
            if let Some(disk) = model::read_list("notes.json") {
                changed |= model::merge(&mut self.notes, disk, self.agreed);
            }
        }
        if now.1 != self.seen.1 {
            if let Some(disk) = model::read_list("days.json") {
                changed |= model::merge(&mut self.days, disk, self.agreed);
            }
        }
        self.trash = pengu::load_trash();
        self.seen = now;
        if changed {
            // Saving writes the merged result back; `agreed` moves on then.
            self.dirty = true;
            if self.idx().is_none() {
                self.active = self.notes.first().map(|n| n.id.clone());
            }
        }
    }

    fn maybe_save(&mut self, ctx: &egui::Context) {
        if self.checked_disk.elapsed() > Duration::from_secs(1) {
            self.pick_up_outside_changes();
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        if !self.dirty {
            return;
        }
        if self.last_save.elapsed() > Duration::from_millis(500) {
            self.pick_up_outside_changes();
            model::save_notes(&self.notes);
            model::save_days(&self.days);
            self.seen = (model::mtime("notes.json"), model::mtime("days.json"));
            self.agreed = model::now_ms();
            self.dirty = false;
            self.last_save = Instant::now();
        } else {
            ctx.request_repaint_after(Duration::from_millis(550));
        }
    }

    fn status_color(&self, p: &Palette) -> Color32 {
        match self.conn {
            Conn::Ok => Color32::from_rgb(0x2f, 0x9e, 0x5a),
            Conn::Fail => p.accent,
            Conn::Testing => p.accent2,
            Conn::None => p.ink_3,
        }
    }

    fn export(&mut self) {
        let Some(i) = self.idx() else { return };
        let n = &self.notes[i];
        let name: String = n
            .title
            .chars()
            .filter(|c| c.is_alphanumeric() || " -_".contains(*c))
            .collect();
        let path = model::export_dir().join(format!(
            "{}.md",
            if name.trim().is_empty() {
                "note"
            } else {
                name.trim()
            }
        ));
        self.saved = match std::fs::write(&path, &n.body) {
            Ok(_) => format!("Exported to {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
    }
}

fn ellipse(
    p: &egui::Painter,
    o: egui::Pos2,
    (cx, cy): (f32, f32),
    (rx, ry): (f32, f32),
    c: Color32,
) {
    let pts: Vec<_> = (0..40)
        .map(|k| {
            let a = k as f32 / 40.0 * std::f32::consts::TAU;
            o + Vec2::new(cx + rx * a.cos(), cy + ry * a.sin())
        })
        .collect();
    p.add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
}

fn logo(ui: &mut egui::Ui) {
    let (rc, _) = ui.allocate_exact_size(Vec2::new(32.0, 33.0), Sense::hover());
    let p = ui.painter_at(rc);
    let o = rc.min;
    let c = |h: u32| Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8);
    let (navy, white, orange) = (c(0x1d2f40), c(0xfbfdff), c(0xf08a24));
    // On a dark background the navy body vanishes, so give it a soft light rim.
    if ui.visuals().dark_mode {
        let rim = c(0x7f9bb5);
        ellipse(&p, o, (16.0, 17.0), (12.8, 14.3), rim);
        ellipse(&p, o, (4.5, 21.0), (4.0, 7.7), rim);
        ellipse(&p, o, (27.5, 21.0), (4.0, 7.7), rim);
    }
    ellipse(&p, o, (16.0, 17.0), (11.5, 13.0), navy);
    ellipse(&p, o, (4.5, 21.0), (2.8, 6.5), navy);
    ellipse(&p, o, (27.5, 21.0), (2.8, 6.5), navy);
    ellipse(&p, o, (16.0, 19.5), (7.2, 9.5), white);
    ellipse(&p, o, (12.6, 12.0), (3.3, 3.5), white);
    ellipse(&p, o, (19.4, 12.0), (3.3, 3.5), white);
    p.circle_filled(o + Vec2::new(13.0, 12.2), 1.3, navy);
    p.circle_filled(o + Vec2::new(19.0, 12.2), 1.3, navy);
    p.add(egui::Shape::convex_polygon(
        vec![
            o + Vec2::new(13.4, 15.2),
            o + Vec2::new(18.6, 15.2),
            o + Vec2::new(16.0, 19.2),
        ],
        orange,
        Stroke::NONE,
    ));
    ellipse(&p, o, (11.5, 30.6), (4.2, 1.7), orange);
    ellipse(&p, o, (20.5, 30.6), (4.2, 1.7), orange);
}

/// First meaningful line of a note, minus markup, skipping a heading that repeats the title.
fn snippet(n: &Note) -> String {
    let clean = |l: &str| {
        l.chars()
            .filter(|c| !"#*`>[]|=~".contains(*c))
            .collect::<String>()
            .trim()
            .trim_start_matches("- ")
            .to_string()
    };
    n.body
        .lines()
        .map(clean)
        .find(|l| {
            !l.is_empty()
                && !l.eq_ignore_ascii_case(n.title.trim())
                && !l.chars().all(|c| c == '-' || c == ' ')
        })
        .unwrap_or_else(|| "Empty note".into())
}

fn menu_item(ui: &mut egui::Ui, text: impl Into<egui::WidgetText>) -> bool {
    ui.add(
        egui::Button::new(text)
            .frame_when_inactive(false)
            .min_size(Vec2::new(180.0, 28.0)),
    )
    .clicked()
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self) {
        model::save_notes(&self.notes);
        model::save_days(&self.days);
        self.save_settings();
    }
}

impl App {
    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        // Fonts only take effect next frame, so the first frame just installs them.
        if self.applied_dark.is_none() {
            theme::install_fonts(ctx);
            theme::apply(ctx, self.dark);
            self.applied_dark = Some(self.dark);
            ctx.request_repaint();
            return;
        }
        if self.applied_dark != Some(self.dark) {
            theme::apply(ctx, self.dark);
            self.applied_dark = Some(self.dark);
        }
        let p = theme::palette(self.dark);
        self.poll();
        self.sync_tick(ctx);
        if self.busy || self.conn == Conn::Testing {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        self.shortcuts(ctx);

        if self.sidebar_open {
            self.sidebar(ui, &p);
        }
        if self.ai_open {
            self.ai_panel(ui, &p);
        }
        let show_note = self.idx().is_some() && !self.calendar;
        if self.topbar_open {
            self.topbar(ui, &p);
            if show_note {
                self.toolbar(ui, &p);
            }
        } else {
            self.folded_topbar(ui, &p);
        }
        if show_note {
            self.footer(ui, &p);
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(p.paper))
            .show(ui, |ui| {
                if self.calendar {
                    let status = self.sync.status();
                    let (hit, changed) = calendar::draw(
                        ui,
                        &p,
                        &mut self.cal,
                        &self.notes,
                        &mut self.days,
                        &self.sync.state.outside,
                        status.as_deref(),
                    );
                    if changed {
                        self.dirty = true;
                        self.saved = format!("Saved {}", chrono::Local::now().format("%H:%M"));
                    }
                    match hit {
                        Some(calendar::Hit::SyncNow) => self.sync.sync_now(),
                        Some(calendar::Hit::Open(id)) => self.open(id),
                        None => {}
                    }
                } else if show_note {
                    self.editor(ui, &p);
                } else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() / 2.0 - 50.0);
                        ui.label(
                            RichText::new("No note selected")
                                .size(22.0)
                                .color(p.ink_3)
                                .family(bold()),
                        );
                        ui.add_space(6.0);
                        if primary(ui, &p, "+ New note").clicked() {
                            self.add_note("", "");
                        }
                    });
                }
            });
        self.settings_modal(ctx, &p);
        self.sync_modal(ctx, &p);
        self.delete_modal(ctx, &p);
        self.maybe_save(ctx);
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let cmd = Modifiers::COMMAND;
        if ctx.input_mut(|i| i.consume_key(cmd, Key::N)) {
            self.add_note("", "");
        }
        if ctx.input_mut(|i| i.consume_key(cmd, Key::F)) {
            if !self.sidebar_open {
                self.sidebar_open = true;
                self.save_settings();
            }
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("search")));
        }
        if ctx.input_mut(|i| i.consume_key(cmd | Modifiers::SHIFT, Key::Space)) {
            match self.voice {
                voice::Voice::Idle => {
                    self.ai_open = true;
                    self.start_talking(ctx);
                }
                voice::Voice::Recording { .. } => self.voice.stop(ctx, &self.voice_lang),
                voice::Voice::Downloading { .. } | voice::Voice::Transcribing { .. } => {}
            }
        }
        if ctx.input_mut(|i| i.consume_key(cmd | Modifiers::SHIFT, Key::Backslash)) {
            self.toggle_topbar();
        }
        if ctx.input_mut(|i| i.consume_key(cmd, Key::Backslash)) {
            self.toggle_sidebar();
        }
        if self.idx().is_some() && !self.calendar {
            if ctx.input_mut(|i| i.consume_key(cmd, Key::B)) {
                self.apply(|v, s, e| edit::wrap(v, s, e, "**", "**"));
            }
            if ctx.input_mut(|i| i.consume_key(cmd, Key::I)) {
                self.apply(|v, s, e| edit::wrap(v, s, e, "*", "*"));
            }
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        egui::Panel::left("list")
            .exact_size(if ui.ctx().content_rect().width() < 1100.0 {
                220.0
            } else {
                270.0
            })
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(p.paper2)
                    .inner_margin(egui::Margin {
                        left: 16,
                        right: 14,
                        top: 14,
                        bottom: 10,
                    }),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    logo(ui);
                    ui.label(RichText::new("Notes").size(24.0).family(bold()));
                    ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                        if pill(ui, p, "+ New", true)
                            .on_hover_text("New note (Ctrl+N)")
                            .clicked()
                        {
                            self.add_note("", "");
                        }
                    });
                });
                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(egui::Id::new("search"))
                        .hint_text("Search notes or #tags…")
                        .desired_width(f32::INFINITY)
                        .margin(egui::Margin::symmetric(10, 6)),
                );
                ui.add_space(4.0);
                let q = self.query.to_lowercase();
                let mut order: Vec<usize> = (0..self.notes.len())
                    .filter(|&i| {
                        let n = &self.notes[i];
                        let tags = n
                            .tags
                            .split(',')
                            .map(|t| format!("#{}", t.trim()))
                            .collect::<Vec<_>>()
                            .join(" ");
                        q.is_empty()
                            || format!("{} {} {}", n.title, n.body, tags)
                                .to_lowercase()
                                .contains(&q)
                    })
                    .collect();
                order.sort_by_key(|&i| {
                    (
                        std::cmp::Reverse(self.notes[i].pinned),
                        std::cmp::Reverse(self.notes[i].updated),
                    )
                });
                let count = self.notes.len();
                ui.horizontal(|ui| {
                    ui.label(caps(
                        &format!("{count} {}", if count == 1 { "note" } else { "notes" }),
                        p,
                        10.5,
                    ));
                    if self.sync.notes_on() {
                        ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                            let busy = self.sync.notes_busy();
                            let failed = self.sync.notes_status().starts_with("iPhone sync failed");
                            let t = ui.input(|i| i.time) as f32;
                            if busy {
                                ui.ctx().request_repaint();
                            }
                            let r = icon_button(ui, p, "Refresh notes", |painter, r, col| {
                                let col = if failed { p.accent } else { col };
                                refresh_icon(painter, r.center(), col, if busy { t * 6.0 } else { 0.0 })
                            })
                            .on_hover_text(self.sync.notes_status());
                            if r.clicked() && !busy {
                                self.sync.notes_sync_now();
                            }
                        });
                    }
                });
                ui.add_space(2.0);
                let mut open = None;
                let mut trash: Option<String> = None;
                if self.trashed.as_ref().is_some_and(|t| t.2.elapsed() > Duration::from_secs(10)) {
                    self.trashed = None;
                }
                if let Some((_, n, at)) = &self.trashed {
                    ui.ctx().request_repaint_after(Duration::from_secs(10).saturating_sub(at.elapsed()));
                    let title = if n.title.is_empty() { "Untitled".to_string() } else { n.title.clone() };
                    let mut undo = false;
                    egui::Panel::bottom("trash-undo")
                        .frame(egui::Frame::new().inner_margin(egui::Margin { left: 0, right: 0, top: 8, bottom: 0 }))
                        .show_separator_line(false)
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .fill(p.ink)
                                .corner_radius(8)
                                .inner_margin(egui::Margin::symmetric(10, 6))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                                            let u = ui
                                                .add(egui::Label::new(RichText::new("Undo").size(14.0).family(bold()).color(p.accent2)).sense(Sense::click()))
                                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                                            undo = u.clicked();
                                            ui.with_layout(ELayout::left_to_right(Align::Center), |ui| {
                                                ui.add(egui::Label::new(RichText::new(format!("Deleted “{title}”")).size(13.5).color(p.paper)).truncate());
                                            });
                                        });
                                    });
                                });
                        });
                    if undo {
                        self.undo_trash();
                    }
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        for &i in &order {
                            let n = &self.notes[i];
                            let on = self.active.as_deref() == Some(n.id.as_str());
                            let fr = egui::Frame::new()
                                .fill(if on { p.tint } else { p.paper })
                                .stroke(Stroke::new(1.5, if on { p.accent } else { p.line }))
                                .corner_radius(8)
                                .inner_margin(egui::Margin::symmetric(12, 8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    let title = if n.title.is_empty() {
                                        "Untitled"
                                    } else {
                                        &n.title
                                    };
                                    let pin = if n.pinned { "★ " } else { "" };
                                    // the title stops short of the trash icon's corner
                                    ui.scope(|ui| {
                                        ui.set_max_width(ui.available_width() - 20.0);
                                        ui.add(
                                            egui::Label::new(theme::ar_job(
                                                &format!("{pin}{title}"),
                                                egui::FontId::new(16.0, bold()),
                                                p.ink,
                                            ))
                                            .truncate(),
                                        );
                                    });
                                    ui.add(
                                        egui::Label::new(theme::ar_job(
                                            &snippet(n),
                                            egui::FontId::proportional(13.0),
                                            p.ink_2,
                                        ))
                                        .truncate(),
                                    );
                                    ui.add_space(2.0);
                                    let date = chrono::DateTime::from_timestamp_millis(n.updated)
                                        .unwrap_or_default()
                                        .with_timezone(&chrono::Local)
                                        .format("%b %-d · %H:%M");
                                    let tags: String =
                                        n.tag_list().iter().map(|t| format!("  #{t}")).collect();
                                    let tasks = md::checklist(&n.body);
                                    let tasks = if tasks.is_empty() {
                                        String::new()
                                    } else {
                                        format!(
                                            "  ☑ {}/{}",
                                            tasks.iter().filter(|t| t.2).count(),
                                            tasks.len()
                                        )
                                    };
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!("{date}{tasks}{tags}"))
                                                .monospace()
                                                .size(10.5)
                                                .color(p.ink_3),
                                        )
                                        .truncate(),
                                    );
                                });
                            let r = ui
                                .interact(
                                    fr.response.rect,
                                    ui.id().with(("card", &n.id)),
                                    Sense::click(),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if r.hovered() && !on {
                                ui.painter().rect_stroke(
                                    fr.response.rect,
                                    8,
                                    Stroke::new(1.5, p.ink_3),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            // trash icon, top right, on hover or on the open note (registered after the card, so it wins the click)
                            let tr = egui::Rect::from_center_size(
                                fr.response.rect.right_top() + Vec2::new(-17.0, 17.0),
                                Vec2::splat(22.0),
                            );
                            let t = ui
                                .interact(tr, ui.id().with(("trash", &n.id)), Sense::click())
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text("Delete note");
                            t.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Trash “{}”", n.title)));
                            if r.hovered() || t.hovered() || on {
                                let col = if t.hovered() { p.accent } else { p.ink_3 };
                                if t.hovered() {
                                    ui.painter().rect_filled(tr, 5, p.line);
                                }
                                trash_icon(ui.painter(), tr.center(), col);
                            }
                            if t.clicked() {
                                trash = Some(n.id.clone());
                            } else if r.clicked() {
                                open = Some(n.id.clone());
                            }
                        }
                        if order.is_empty() {
                            ui.add_space(8.0);
                            ui.label(RichText::new("No notes found.").color(p.ink_3));
                        }
                    });
                if let Some(id) = open {
                    self.open(id);
                }
                if let Some(id) = trash {
                    self.trash_note(&id);
                }
            });
    }

    fn toggle_sidebar(&mut self) {
        self.sidebar_open = !self.sidebar_open;
        self.save_settings();
    }

    fn toggle_topbar(&mut self) {
        self.topbar_open = !self.topbar_open;
        self.save_settings();
    }

    fn sidebar_button(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let (label, tip) = if self.sidebar_open {
            ("Hide sidebar", "Hide sidebar (Ctrl+\\)")
        } else {
            ("Show sidebar", "Show sidebar (Ctrl+\\)")
        };
        let open = self.sidebar_open;
        if icon_button(ui, p, label, |painter, r, col| sidebar_icon(painter, r.center(), col, open))
            .on_hover_text(tip)
            .clicked()
        {
            self.toggle_sidebar();
        }
    }

    fn topbar_button(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let up = self.topbar_open;
        let (label, tip) = if up {
            ("Hide top bar", "Hide top bar (Ctrl+Shift+\\)")
        } else {
            ("Show top bar", "Show top bar (Ctrl+Shift+\\)")
        };
        if icon_button(ui, p, label, |painter, r, col| chevron(painter, r.center(), col, up))
            .on_hover_text(tip)
            .clicked()
        {
            self.toggle_topbar();
        }
    }

    /// The top bar folded away: a thin strip with the two toggles.
    fn folded_topbar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        egui::Panel::top("top-folded")
            .frame(
                egui::Frame::new()
                    .fill(p.paper)
                    .inner_margin(egui::Margin::symmetric(10, 2)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().interact_size.y = 0.0;
                    self.sidebar_button(ui, p);
                    ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                        self.topbar_button(ui, p);
                    });
                });
            });
    }

    fn topbar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let pinned = self.idx().is_some_and(|i| self.notes[i].pinned);
        let pin_label = if pinned { "★ Pinned" } else { "☆ Pin" };
        egui::Panel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(p.paper)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| {
                let gap = ui.spacing().item_spacing.x;
                let views = ["Edit", "Split", "Preview"];
                let seg_w: f32 = views
                    .iter()
                    .map(|v| theme::pill_width(ui, v, &Deco::None))
                    .sum();
                let act_w: f32 = [
                    (pin_label, Deco::None),
                    ("Calendar", Deco::None),
                    ("Pengu", Deco::Dot(p.ink)),
                    ("More", Deco::Chevron),
                ]
                .iter()
                .map(|(t, d)| theme::pill_width(ui, t, d) + gap)
                .sum::<f32>()
                    + 2.0 * (ICON_BTN + gap);
                let fits = ui.available_width() >= seg_w + act_w + 24.0;
                let views_ui = |this: &mut Self, ui: &mut egui::Ui| {
                    let sel = (!this.calendar).then_some(this.view as usize);
                    if let Some(k) = theme::segmented(ui, p, &views, sel) {
                        this.view = [View::Edit, View::Split, View::Preview][k];
                        this.calendar = false;
                    }
                };
                if fits {
                    // One row, actions right-aligned (added right-to-left, so reversed).
                    ui.horizontal(|ui| {
                        self.sidebar_button(ui, p);
                        views_ui(self, ui);
                        ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                            self.topbar_button(ui, p);
                            for k in (0..4).rev() {
                                self.top_action(ui, p, k, pin_label, pinned);
                            }
                        });
                    });
                } else {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        self.sidebar_button(ui, p);
                        views_ui(self, ui);
                        ui.end_row();
                        for k in 0..4 {
                            self.top_action(ui, p, k, pin_label, pinned);
                        }
                        self.topbar_button(ui, p);
                    });
                }
            });
    }

    fn top_action(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        k: usize,
        pin_label: &str,
        pinned: bool,
    ) {
        match k {
            0 => {
                let r = pill(ui, p, pin_label, pinned);
                if let (true, Some(i)) = (r.clicked(), self.idx()) {
                    self.notes[i].pinned = !pinned;
                    self.touch(i);
                }
            }
            1 => {
                if pill(ui, p, "Calendar", self.calendar).clicked() {
                    self.calendar = !self.calendar;
                }
            }
            2 => {
                if pill_ex(
                    ui,
                    p,
                    "Pengu",
                    self.ai_open,
                    Deco::Dot(self.status_color(p)),
                )
                .clicked()
                {
                    self.ai_open = !self.ai_open;
                }
            }
            _ => {
                let more = pill_ex(ui, p, "More", false, Deco::Chevron);
                egui::Popup::menu(&more).show(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    if menu_item(ui, "Export .md") {
                        self.export();
                    }
                    if menu_item(ui, "Pengu settings…") {
                        self.settings_open = true;
                    }
                    if menu_item(ui, "iPhone sync…") {
                        self.sync.open = true;
                    }
                    if menu_item(
                        ui,
                        format!("{} theme", if self.dark { "Light" } else { "Dark" }),
                    ) {
                        self.dark = !self.dark;
                        self.save_settings();
                    }
                    ui.separator();
                    if menu_item(ui, RichText::new("Delete note").color(p.accent)) {
                        self.confirm_delete = self.idx().is_some();
                    }
                });
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, p: &Palette) {
        fn w(a: &'static str, b: &'static str) -> Op {
            Box::new(move |v, s, e| edit::wrap(v, s, e, a, b))
        }
        fn pre(a: &'static str) -> Op {
            Box::new(move |v, s, e| edit::prefix(v, s, e, a))
        }
        fn ins(t: &'static str) -> Op {
            Box::new(move |v, s, e| edit::insert(v, s, e, t))
        }
        let date: Op = Box::new(|v, s, e| {
            edit::insert(
                v,
                s,
                e,
                &chrono::Local::now().format("%b %-d, %Y").to_string(),
            )
        });
        let groups: Vec<Vec<(&str, &str, Op)>> = vec![
            vec![
                ("H", "Heading", pre("## ")),
                ("B", "Bold (Ctrl+B)", w("**", "**")),
                ("I", "Italic (Ctrl+I)", w("*", "*")),
                ("S", "Strikethrough", w("~~", "~~")),
                ("Hi", "Highlight", w("==", "==")),
            ],
            vec![
                ("•", "Bullet list", pre("- ")),
                ("1.", "Numbered list", pre("1. ")),
                ("[ ]", "Checklist item", pre("- [ ] ")),
                ("\"", "Quote", pre("> ")),
            ],
            vec![
                ("</>", "Inline code", w("`", "`")),
                ("Link", "Web link", w("[", "](https://)")),
                ("[[ ]]", "Link to another note", w("[[", "]]")),
            ],
        ];
        let inserts: Vec<(&str, Op)> = vec![
            (
                "Table",
                ins("\n| Column A | Column B |\n| --- | --- |\n| 1 | 2 |\n"),
            ),
            ("Code block", ins("\n```\ncode\n```\n")),
            ("Today's date", date),
            ("Divider", ins("\n---\n")),
            (
                "Bar chart",
                ins("\n```chart bar\ntitle: Title\nA: 12\nB: 19\nC: 8\n```\n"),
            ),
            (
                "Line chart",
                ins("\n```chart line\ntitle: Title\nMon: 3\nTue: 7\nWed: 5\nThu: 9\n```\n"),
            ),
            (
                "Pie chart",
                ins("\n```chart pie\ntitle: Title\nA: 50\nB: 30\nC: 20\n```\n"),
            ),
        ];
        let mut chosen: Option<Op> = None;
        egui::Panel::top("tools")
            .frame(
                egui::Frame::new()
                    .fill(p.paper)
                    .inner_margin(egui::Margin::symmetric(16, 5)),
            )
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(2.0, 4.0);
                    for items in groups {
                        for (label, tip, op) in items {
                            if theme::tool(ui, p, label, tip).clicked() {
                                chosen = Some(op);
                            }
                        }
                        ui.add_space(6.0);
                        ui.add(egui::Separator::default().vertical().spacing(0.0));
                        ui.add_space(6.0);
                    }
                    let more = pill_ex(ui, p, "Insert", false, Deco::Chevron);
                    egui::Popup::menu(&more).show(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for (k, (label, op)) in inserts.into_iter().enumerate() {
                            if k == 4 {
                                ui.separator();
                            }
                            if menu_item(ui, label) {
                                chosen = Some(op);
                            }
                        }
                    });
                });
            });
        if let Some(op) = chosen {
            self.apply(|v, s, e| op(v, s, e));
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let Some(i) = self.idx() else { return };
        let b = &self.notes[i].body;
        let words = b.split_whitespace().count();
        egui::Panel::bottom("foot")
            .frame(
                egui::Frame::new()
                    .fill(p.paper)
                    .inner_margin(egui::Margin::symmetric(24, 5)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let small = |t: String| RichText::new(t).monospace().size(11.0).color(p.ink_3);
                    ui.label(small(format!(
                        "{words} words · {} chars",
                        b.chars().count()
                    )));
                    let tasks = md::checklist(b);
                    if !tasks.is_empty() {
                        let done = tasks.iter().filter(|t| t.2).count();
                        ui.add_space(14.0);
                        let (bar, _) = ui.allocate_exact_size(Vec2::new(90.0, 6.0), Sense::hover());
                        ui.painter().rect_filled(bar, 3, p.line);
                        let w = bar.width() * done as f32 / tasks.len() as f32;
                        ui.painter().rect_filled(
                            egui::Rect::from_min_size(bar.min, Vec2::new(w, bar.height())),
                            3,
                            p.accent,
                        );
                        let all = done == tasks.len();
                        let t =
                            format!("{done}/{} done{}", tasks.len(), if all { " ✓" } else { "" });
                        ui.label(RichText::new(t).monospace().size(11.0).color(if all {
                            p.accent
                        } else {
                            p.ink_2
                        }));
                    }
                    let saved = if self.saved.is_empty() {
                        "Autosave on".to_string()
                    } else {
                        self.saved.clone()
                    };
                    let w = ui
                        .painter()
                        .layout_no_wrap(saved.clone(), egui::FontId::monospace(11.0), p.ink_3)
                        .size()
                        .x;
                    // Drop the save status rather than overlap the counters in a narrow window.
                    if ui.available_width() > w + 16.0 {
                        ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                            ui.label(small(saved))
                        });
                    }
                });
            });
    }

    fn editor(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let Some(i) = self.idx() else { return };
        let split = self.view == View::Split;
        let max = if split { 1240.0 } else { 820.0 };
        let full = ui.available_rect_before_wrap();
        let side = ((full.width() - max) / 2.0).max(0.0) + 28.0;
        let col = egui::Rect::from_min_max(
            full.min + Vec2::new(side, 18.0),
            full.max - Vec2::new(side, 0.0),
        );
        let ed_id = egui::Id::new(("body", &self.notes[i].id));
        let mut changed = false;
        let mut action = Vec::new();

        ui.scope_builder(UiBuilder::new().max_rect(col), |ui| {
            let n = &mut self.notes[i];
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut n.title)
                        .hint_text("Untitled")
                        .layouter(&mut |ui, text, wrap| {
                            let mut job = theme::ar_job(
                                text.as_str(),
                                egui::FontId::new(32.0, bold()),
                                ui.visuals().text_color(),
                            );
                            job.wrap.max_width = wrap;
                            ui.fonts_mut(|f| f.layout_job(job))
                        })
                        .font(egui::FontId::new(32.0, bold()))
                        .frame(egui::Frame::NONE)
                        .desired_width(f32::INFINITY),
                )
                .changed();
            ui.horizontal(|ui| {
                ui.label(RichText::new("#").monospace().size(12.5).color(p.ink_3));
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut n.tags)
                            .hint_text("tags, comma separated")
                            .font(egui::FontId::monospace(12.5))
                            .text_color(p.ink_2)
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY),
                    )
                    .changed();
            });
            ui.add_space(2.0);
            ui.separator();
            ui.add_space(4.0);

            let area = ui.available_rect_before_wrap();
            let (left, right) = match self.view {
                View::Edit => (Some(area), None),
                View::Preview => (None, Some(area)),
                View::Split => {
                    let mid = area.center().x;
                    ui.painter().add(egui::Shape::dashed_line(
                        &[
                            egui::pos2(mid, area.top()),
                            egui::pos2(mid, area.bottom() - 8.0),
                        ],
                        Stroke::new(1.2, p.line),
                        5.0,
                        4.0,
                    ));
                    (
                        Some(area.with_max_x(mid - 14.0)),
                        Some(area.with_min_x(mid + 14.0)),
                    )
                }
            };
            let mut body = std::mem::take(&mut self.notes[i].body);
            let mut toggles = Vec::new();
            if let Some(r) = left {
                if let Some((s, e)) = self.pending_sel.take() {
                    let mut st =
                        egui::text_edit::TextEditState::load(ui.ctx(), ed_id).unwrap_or_default();
                    st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(s),
                        egui::text::CCursor::new(e),
                    )));
                    st.store(ui.ctx(), ed_id);
                }
                ui.scope_builder(UiBuilder::new().max_rect(r), |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("edit", ed_id))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let rows = ((r.height() - 8.0) / 22.0).max(4.0) as usize;
                            let pal = *p;
                            let reveal = ui
                                .memory(|m| m.has_focus(ed_id))
                                .then_some(self.sel)
                                .flatten();
                            let mut layouter =
                                |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
                                    let mut job = crate::hl::highlight(buf.as_str(), &pal, reveal);
                                    job.wrap.max_width = wrap;
                                    ui.painter().layout_job(job)
                                };
                            let out = egui::TextEdit::multiline(&mut body)
                                .layouter(&mut layouter)
                                .id(ed_id)
                                .font(egui::FontId::monospace(14.0))
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY)
                                .desired_rows(rows)
                                .hint_text("Start writing…")
                                .lock_focus(true)
                                .show(ui);
                            changed |= out.response.changed();
                            // Paint clickable checkboxes over the (invisible) `[ ]` / `[x]` text.
                            for (line, at, done) in md::checklist(&body) {
                                let a = out.galley.pos_from_cursor(egui::text::CCursor::new(at));
                                let b =
                                    out.galley.pos_from_cursor(egui::text::CCursor::new(at + 3));
                                let cell = egui::Rect::from_min_max(a.min, b.max)
                                    .translate(out.galley_pos.to_vec2());
                                let bx =
                                    egui::Rect::from_center_size(cell.center(), Vec2::splat(15.0));
                                let resp = ui
                                    .interact(
                                        bx.expand(3.0),
                                        ed_id.with(("task", line)),
                                        Sense::click(),
                                    )
                                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                                let painter = ui.painter();
                                let stroke = if resp.hovered() { p.accent } else { p.ink_2 };
                                painter.rect(
                                    bx,
                                    3,
                                    if done { p.accent } else { p.paper },
                                    Stroke::new(1.5, if done { p.accent } else { stroke }),
                                    egui::StrokeKind::Inside,
                                );
                                if done {
                                    let c = bx.center();
                                    painter.add(egui::Shape::line(
                                        vec![
                                            c + Vec2::new(-4.0, 0.0),
                                            c + Vec2::new(-1.0, 3.5),
                                            c + Vec2::new(4.5, -3.5),
                                        ],
                                        Stroke::new(2.0, Color32::WHITE),
                                    ));
                                }
                                if resp.clicked() {
                                    toggles.push(line);
                                }
                            }
                            if let (Some(c), true) = (out.cursor_range, out.response.has_focus()) {
                                let (a, b) =
                                    (usize::from(c.primary.index), usize::from(c.secondary.index));
                                self.sel = Some((a.min(b), a.max(b)));
                            }
                        });
                });
            }
            if let Some(r) = right {
                ui.scope_builder(UiBuilder::new().max_rect(r), |ui| {
                    if self.blocks.0 != body {
                        self.blocks = (body.clone(), md::parse(&body));
                    }
                    egui::ScrollArea::vertical()
                        .id_salt(("preview", ed_id))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            action = md::render(ui, &self.blocks.1, p);
                            ui.add_space(24.0);
                        });
                });
            }
            for l in toggles {
                body = md::toggle_line(&body, l);
                changed = true;
            }
            self.notes[i].body = body;
        });
        ui.allocate_rect(full, Sense::hover());

        for a in action {
            match a {
                Action::Toggle(l) => {
                    self.notes[i].body = md::toggle_line(&self.notes[i].body, l);
                    changed = true;
                }
                Action::Wiki(t) => self.open_by_title(&t),
                Action::Url(u) => ui.ctx().open_url(egui::OpenUrl::new_tab(u)),
            }
        }
        if self.refocus && self.view != View::Preview {
            if let Some(j) = self.idx() {
                ui.ctx()
                    .memory_mut(|m| m.request_focus(egui::Id::new(("body", &self.notes[j].id))));
            }
        }
        self.refocus = false;
        if changed {
            self.touch(i);
        }
    }

    fn ai_panel(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = &ui.ctx().clone();
        // (label, tooltip, instruction). "Sort" uses what you typed, the rest use the selection or note.
        let actions: [(&str, &str, &str); 6] = [
            (
                "Sort into note",
                "Turn what you typed (or the selection) into a tidy new note",
                "Sort the following into a clean, well-organized new note. Schedule anything that has a date.",
            ),
            (
                "Find dates",
                "Put every date and deadline in this note on the calendar",
                "Find every date, deadline, appointment or event in the following text and schedule each one. Do not create a note.",
            ),
            (
                "Plan my day",
                "Make today's plan from your open tasks",
                "Look at the open note and make a short plan for today: schedule today's most important to-dos.",
            ),
            (
                "Summarize",
                "Bullet summary",
                "Summarize the following text as short markdown bullet points. Output only the summary.",
            ),
            (
                "Improve",
                "Fix grammar and clarity",
                "Improve clarity and grammar of the following text. Keep meaning and markdown formatting. Output only the revised text.",
            ),
            (
                "Tags",
                "Suggest tags",
                "Suggest 3 to 6 short lowercase tags for the following text, comma separated, no # and no other text.",
            ),
        ];
        let pad = |y| egui::Frame::new().inner_margin(egui::Margin::symmetric(14, y));
        egui::Panel::right("ai")
            .exact_size(if ui.ctx().content_rect().width() < 1100.0 { 300.0 } else { 350.0 })
            .resizable(false)
            .frame(egui::Frame::new().fill(p.paper2))
            .show(ui, |ui| {
                egui::Panel::top("ai-head").frame(pad(12)).show(ui, |ui| {
                    // Settings is placed first (right to left) so the model name truncates before reaching it.
                    ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                        if pill(ui, p, "Settings", false).clicked() {
                            self.settings_open = true;
                        }
                        ui.add_space(6.0);
                        ui.with_layout(ELayout::left_to_right(Align::Center), |ui| {
                            logo(ui);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.label(RichText::new("Pengu").size(20.0).family(bold()));
                                let st = match (self.conn, self.ai.model.is_empty()) {
                                    (Conn::Ok, _) => format!("{} · connected", self.ai.model),
                                    (_, false) => self.ai.model.clone(),
                                    _ => "your note assistant".into(),
                                };
                                ui.add(egui::Label::new(RichText::new(st).monospace().size(10.5).color(p.ink_3)).truncate())
                                    .on_hover_text(format!("Model: {}", if self.ai.model.is_empty() { "none" } else { &self.ai.model }));
                            });
                        });
                    });
                });
                let mut clicked: Option<(&str, &str)> = None;
                egui::Panel::top("ai-actions").frame(pad(10)).show(ui, |ui| {
                    // header row: the whole row toggles; the arrow sits on the right
                    let head = ui
                        .horizontal(|ui| {
                            ui.label(caps("Quick actions", p, 10.0));
                            ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                                // painted triangle: the UI fonts have no ▾ / ▸ glyphs
                                let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), egui::Sense::hover());
                                let (c, h) = (r.center(), 4.0);
                                let pts = if self.actions_open {
                                    vec![c + Vec2::new(-h, -h * 0.5), c + Vec2::new(h, -h * 0.5), c + Vec2::new(0.0, h * 0.7)]
                                } else {
                                    vec![c + Vec2::new(-h * 0.5, -h), c + Vec2::new(-h * 0.5, h), c + Vec2::new(h * 0.7, 0.0)]
                                };
                                ui.painter().add(egui::Shape::convex_polygon(pts, p.ink_2, egui::Stroke::NONE));
                            });
                        })
                        .response
                        .interact(egui::Sense::click())
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(if self.actions_open { "Hide quick actions" } else { "Show quick actions" });
                    if head.clicked() {
                        self.actions_open = !self.actions_open;
                        self.save_settings();
                    }
                    if !self.actions_open {
                        return;
                    }
                    ui.add_space(2.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(5.0, 5.0);
                        for (k, (l, t, i)) in actions.into_iter().enumerate() {
                            if pill(ui, p, l, k == 0).on_hover_text(t).clicked() {
                                clicked = Some((l, i));
                            }
                        }
                    });
                    ui.add_space(2.0);
                    if ui.checkbox(&mut self.pengu_edits, RichText::new("Let Pengu edit notes & calendar").size(13.5).color(p.ink_2)).changed() {
                        self.save_settings();
                    }
                    ui.checkbox(&mut self.use_ctx, RichText::new("Share the open note with Pengu").size(13.5).color(p.ink_2));
                });
                let mut to_send: Option<String> = None;
                egui::Panel::bottom("ai-input").frame(pad(10)).show(ui, |ui| {
                    let id = egui::Id::new("ai-draft");
                    // Talk: a finished transcript goes out with whatever was typed.
                    match self.voice.poll() {
                        Some(Ok(said)) if !self.busy => {
                            let typed = self.draft.trim();
                            to_send = Some(if typed.is_empty() { said } else { format!("{typed} {said}") });
                        }
                        Some(Ok(said)) => self.draft = format!("{} {said}", self.draft.trim()).trim().to_string(),
                        Some(Err(e)) => self.err = e,
                        None => {}
                    }
                    if ui.ctx().memory(|m| m.has_focus(id)) && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) && !self.busy && !self.draft.trim().is_empty() {
                        to_send = Some(self.draft.trim().to_string());
                    }
                    ui.add(
                        egui::TextEdit::multiline(&mut self.draft)
                            .id(id)
                            .desired_rows(1)
                            .desired_width(f32::INFINITY)
                            .margin(egui::Margin::symmetric(8, 6)),
                    );
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        if pill(ui, p, "Clear", false).clicked() {
                            self.stop_chat();
                            self.msgs.clear();
                            self.err.clear();
                        }
                        ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                            if primary(ui, p, if self.busy { "Stop" } else { "Send" }).clicked() {
                                if self.busy {
                                    self.stop_chat();
                                } else if !self.draft.trim().is_empty() {
                                    to_send = Some(self.draft.trim().to_string());
                                }
                            }
                            self.talk_button(ui, p);
                        });
                    });
                });
                // (message index, 0 insert / 1 replace / 2 copy / 3 apply / 4 undo)
                let mut post: Option<(usize, u8)> = None;
                egui::CentralPanel::default().frame(pad(10)).show(ui, |ui| {
                    egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                        let n = self.msgs.len();
                        let maxw = ui.available_width() * 0.92;
                        for (k, m) in self.msgs.iter().enumerate() {
                            let user = m.role == "user";
                            let streaming = self.busy && k == n - 1;
                            let (bg, fg) = if user { (p.ink, p.paper) } else { (p.paper, p.ink) };
                            let shown = pengu::visible(&m.content);
                            let text = match (user, m.label.is_empty()) {
                                (true, false) => m.label.as_str(),
                                (true, true) => m.content.as_str(),
                                _ if shown.is_empty() && streaming => "Pengu is thinking…",
                                _ if shown.is_empty() => "Done.",
                                _ => shown,
                            };
                            ui.with_layout(ELayout::top_down(if user { Align::Max } else { Align::Min }), |ui| {
                                egui::Frame::new().fill(bg).stroke(Stroke::new(1.3, p.ink)).corner_radius(10).inner_margin(egui::Margin::symmetric(11, 7)).show(ui, |ui| {
                                    ui.set_max_width(maxw);
                                    ui.add(egui::Label::new(RichText::new(text).size(14.0).color(fg)).wrap());
                                });
                                if !m.acts.is_empty() {
                                    let applied = m.state == ActState::Applied;
                                    egui::Frame::new().fill(if applied { p.tint } else { p.paper2 }).stroke(Stroke::new(1.2, if applied { p.accent } else { p.line })).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                                        ui.set_max_width(maxw);
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        for act in &m.acts {
                                            let mark = match m.state { ActState::Applied => "✓", ActState::Undone => "↺", ActState::Pending => "•" };
                                            ui.horizontal_top(|ui| {
                                                ui.spacing_mut().item_spacing.x = 6.0;
                                                ui.label(RichText::new(mark).monospace().size(12.5).color(p.accent));
                                                ui.add(egui::Label::new(RichText::new(act.describe()).size(13.0).color(p.ink)).wrap());
                                            });
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;
                                        match m.state {
                                            ActState::Applied => {
                                                if pill(ui, p, "Undo", false).clicked() {
                                                    post = Some((k, 4));
                                                }
                                            }
                                            _ => {
                                                if pill(ui, p, if m.state == ActState::Undone { "Redo" } else { "Apply" }, true).clicked() {
                                                    post = Some((k, 3));
                                                }
                                            }
                                        }
                                    });
                                } else if !user && !shown.is_empty() && !streaming {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 4.0;
                                        for (j, l) in ["Insert", "Replace note", "Copy"].iter().enumerate() {
                                            if pill(ui, p, l, false).clicked() {
                                                post = Some((k, j as u8));
                                            }
                                        }
                                    });
                                }
                            });
                            ui.add_space(4.0);
                        }
                        if !self.err.is_empty() {
                            egui::Frame::new().stroke(Stroke::new(1.3, p.accent)).corner_radius(8).inner_margin(egui::Margin::symmetric(11, 7)).show(ui, |ui| {
                                ui.add(egui::Label::new(RichText::new(&self.err).size(13.0).color(p.accent)).wrap());
                            });
                        }
                    });
                });
                if let Some((k, which)) = post {
                    let text = pengu::visible(&self.msgs[k].content).to_string();
                    match which {
                        0 => self.apply(|v, s, e| edit::insert(v, s, e, &text)),
                        1 => {
                            if let Some(i) = self.idx() {
                                self.notes[i].body = text;
                                self.touch(i);
                                if self.view == View::Preview {
                                    self.view = View::Edit;
                                }
                            }
                        }
                        2 => ctx.copy_text(text),
                        3 => self.pengu_apply(k),
                        _ => self.pengu_undo(k),
                    }
                }
                if let Some((l, instr)) = clicked {
                    // "Sort into note" prefers what you typed in the box.
                    let t = if l == "Sort into note" && !self.draft.trim().is_empty() { self.draft.trim().to_string() } else { self.selection_text() };
                    self.send(ctx, format!("{instr}\n\n---\n{t}"), l.to_string());
                }
                if let Some(t) = to_send {
                    self.send(ctx, t, String::new());
                }
            });
    }

    /// Click to talk, click again to send what you said.
    fn talk_button(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        match &self.voice {
            voice::Voice::Idle => {
                if pill_ex(ui, p, "Talk", false, Deco::Dot(p.accent))
                    .on_hover_text("Talk to Pengu (Ctrl+Shift+Space): click, speak, click again to send")
                    .clicked()
                {
                    self.start_talking(&ctx);
                }
            }
            voice::Voice::Recording { since, .. } => {
                let secs = since.elapsed().as_secs();
                ctx.request_repaint_after(Duration::from_millis(250));
                if pill_ex(ui, p, &format!("Done {}:{:02}", secs / 60, secs % 60), true, Deco::Dot(p.accent))
                    .on_hover_text("Listening… click to send")
                    .clicked()
                {
                    self.voice.stop(&ctx, &self.voice_lang);
                }
            }
            voice::Voice::Downloading { got, .. } => {
                let pct = got.load(std::sync::atomic::Ordering::Relaxed) * 100 / voice::MODEL_SIZE;
                ctx.request_repaint_after(Duration::from_millis(250));
                ui.add_enabled(false, egui::Label::new(RichText::new(format!("Getting voice ready… {pct}%")).size(13.0).color(p.ink_2)))
                    .on_disabled_hover_text("Downloading the speech model (190 MB), once. After that, voice works offline.");
            }
            voice::Voice::Transcribing { .. } => {
                ctx.request_repaint_after(Duration::from_millis(250));
                ui.add_enabled(false, egui::Label::new(RichText::new("Writing it down…").size(13.0).color(p.ink_2)));
            }
        }
        // The spoken language, shared with the desktop widget: click to switch.
        let (label, tip) = if self.voice_lang == "ar" {
            ("AR", "Talking in Arabic. Click for English")
        } else {
            ("EN", "Talking in English. Click for Arabic")
        };
        if pill(ui, p, label, false).on_hover_text(tip).clicked() {
            self.voice_lang = if self.voice_lang == "ar" { "en" } else { "ar" }.into();
            if !cfg!(test) {
                voice::set_lang(&self.voice_lang);
            }
        }
    }

    fn start_talking(&mut self, ctx: &egui::Context) {
        self.err.clear();
        if let Err(e) = self.voice.start(ctx) {
            self.err = e;
        }
    }

    fn settings_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.settings_open {
            return;
        }
        let mut changed = false;
        let mut test = false;
        let mut done = false;
        let frame = egui::Frame::window(&ctx.global_style()).inner_margin(18);
        let resp = egui::Modal::new(egui::Id::new("settings")).frame(frame).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Pengu settings").size(21.0).family(bold()));
                ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                    done = pill(ui, p, "Done", false).clicked();
                });
            });
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                for (l, u, model) in PRESETS {
                    if pill(ui, p, l, self.ai.preset == l).clicked() {
                        self.ai.preset = l.into();
                        if l != "Custom" {
                            self.ai.url = u.into();
                            self.ai.model = model.into();
                        }
                        self.models.clear();
                        self.conn = Conn::None;
                        self.conn_msg.clear();
                        changed = true;
                    }
                }
            });
            ui.add_space(4.0);
            let field = |ui: &mut egui::Ui, label: &str, val: &mut String, hint: &str, pw: bool| -> bool {
                ui.add_space(4.0);
                ui.label(caps(label, p, 10.0));
                ui.add(
                    egui::TextEdit::singleline(val)
                        .hint_text(hint)
                        .password(pw)
                        .font(egui::FontId::monospace(12.5))
                        .margin(egui::Margin::symmetric(8, 6))
                        .desired_width(f32::INFINITY),
                )
                .changed()
            };
            if field(ui, "Endpoint (OpenAI-compatible)", &mut self.ai.url, "https://api.deepseek.com", false) {
                self.ai.preset = "Custom".into();
                changed = true;
            }
            changed |= field(ui, "Model (any name the server accepts)", &mut self.ai.model, "e.g. deepseek-flash", false);
            changed |= field(ui, "API key", &mut self.ai.key, "paste your key (not needed for Ollama / LM Studio)", true);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                test = primary(ui, p, "Test connection").clicked();
                ui.add(egui::Label::new(RichText::new(&self.conn_msg).size(13.0).color(p.ink_2)).truncate());
            });
            if !self.models.is_empty() {
                ui.add_space(4.0);
                ui.label(caps("Pick a model", p, 10.0));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                    for m in self.models.clone() {
                        if pill(ui, p, &m, self.ai.model == m).clicked() {
                            self.ai.model = m;
                            changed = true;
                        }
                    }
                });
            }
            ui.add_space(6.0);
            ui.label(RichText::new("Works with any OpenAI-compatible API. Your key is stored only on this computer, readable just by you, and sent only to the endpoint above.").size(12.0).color(p.ink_3));
        });
        if changed {
            self.save_settings();
        }
        if test {
            self.test_connection(ctx);
        }
        if done || resp.should_close() {
            self.settings_open = false;
        }
    }

    fn delete_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.confirm_delete {
            return;
        }
        let title = self
            .idx()
            .map(|i| self.notes[i].title.clone())
            .unwrap_or_default();
        let mut choice = None;
        let frame = egui::Frame::window(&ctx.global_style()).inner_margin(18);
        let resp = egui::Modal::new(egui::Id::new("delete"))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_width(320.0);
                ui.label(RichText::new("Delete this note?").size(20.0).family(bold()));
                ui.label(
                    RichText::new(format!(
                        "“{}” will be removed permanently.",
                        if title.is_empty() { "Untitled" } else { &title }
                    ))
                    .color(p.ink_2),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                        if primary(ui, p, "Delete").clicked() {
                            choice = Some(true);
                        }
                        if pill(ui, p, "Cancel", false).clicked() {
                            choice = Some(false);
                        }
                    });
                });
            });
        if choice == Some(true) {
            self.delete_active();
        }
        if choice.is_some() || resp.should_close() {
            self.confirm_delete = false;
        }
    }
}

/// A small trash can (the UI fonts have no trash glyph): lid with a handle, tapered bin, two ribs.
const ICON_BTN: f32 = 28.0;

/// Square button with a painted icon (the UI fonts lack most symbols).
fn icon_button(
    ui: &mut egui::Ui,
    p: &Palette,
    label: &str,
    paint: impl FnOnce(&egui::Painter, egui::Rect, Color32),
) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(Vec2::splat(ICON_BTN), Sense::click());
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        if r.hovered() {
            ui.painter().rect_filled(rect.shrink(2.0), 7, p.paper2);
        }
        paint(ui.painter(), rect, if r.hovered() { p.ink } else { p.ink_2 });
    }
    r
}

/// A window with a side column; the column is filled while the sidebar is open.
fn sidebar_icon(painter: &egui::Painter, c: egui::Pos2, col: Color32, open: bool) {
    let r = egui::Rect::from_center_size(c, Vec2::new(16.0, 13.0));
    let s = Stroke::new(1.4, col);
    painter.rect_stroke(r, 3, s, egui::StrokeKind::Middle);
    let x = r.left() + 5.5;
    painter.line_segment([egui::pos2(x, r.top()), egui::pos2(x, r.bottom())], s);
    if open {
        painter.rect_filled(egui::Rect::from_min_max(r.min, egui::pos2(x, r.bottom())), 3, col);
    }
}

/// A circular arrow, turned by `angle` (spins while syncing).
fn refresh_icon(painter: &egui::Painter, c: egui::Pos2, col: Color32, angle: f32) {
    let r = 5.5;
    let at = |a: f32, d: f32| c + Vec2::new((a + angle).cos() * d, (a + angle).sin() * d);
    let pts: Vec<egui::Pos2> = (0..=20).map(|k| at(0.5 + k as f32 * 0.25, r)).collect();
    painter.add(egui::Shape::line(pts, Stroke::new(1.5, col)));
    // arrowhead at the start of the arc
    let tip = at(0.5, r);
    let back = at(0.5 + 0.55, r);
    let dir = (tip - back).normalized();
    let side = Vec2::new(-dir.y, dir.x);
    painter.add(egui::Shape::convex_polygon(
        vec![tip + dir * 2.5, tip - dir * 1.5 + side * 2.6, tip - dir * 1.5 - side * 2.6],
        col,
        Stroke::NONE,
    ));
}

/// ^ (fold up) or v (unfold).
fn chevron(painter: &egui::Painter, c: egui::Pos2, col: Color32, up: bool) {
    let (w, h) = (5.0, if up { -2.5 } else { 2.5 });
    painter.add(egui::Shape::line(
        vec![c + Vec2::new(-w, -h), c + Vec2::new(0.0, h), c + Vec2::new(w, -h)],
        Stroke::new(1.6, col),
    ));
}

fn trash_icon(painter: &egui::Painter, c: egui::Pos2, col: Color32) {
    let s = Stroke::new(1.4, col);
    let v = |x: f32, y: f32| c + Vec2::new(x, y);
    painter.line_segment([v(-6.0, -4.0), v(6.0, -4.0)], s);
    painter.line_segment([v(-2.0, -4.0), v(-2.0, -6.0)], s);
    painter.line_segment([v(-2.0, -6.0), v(2.0, -6.0)], s);
    painter.line_segment([v(2.0, -6.0), v(2.0, -4.0)], s);
    painter.add(egui::Shape::closed_line(vec![v(-4.5, -2.5), v(4.5, -2.5), v(3.6, 6.0), v(-3.6, 6.0)], s));
    painter.line_segment([v(-1.4, -0.5), v(-1.1, 4.0)], Stroke::new(1.1, col));
    painter.line_segment([v(1.4, -0.5), v(1.1, 4.0)], Stroke::new(1.1, col));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    fn harness(app: App) -> Harness<'static, App> {
        let mut h = Harness::builder()
            .with_size([1280.0, 800.0])
            .wgpu()
            .build_ui_state(|ui, a: &mut App| a.draw(ui), app);
        h.run_steps(4);
        h
    }

    fn fresh() -> Harness<'static, App> {
        harness(App::from_parts(
            model::load_notes_default(),
            Settings::default(),
        ))
    }

    fn shot(h: &mut Harness<'_, App>, name: &str) {
        let dir = format!("{}/tests-shots", env!("CARGO_MANIFEST_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        h.render()
            .expect("render")
            .save(format!("{dir}/{name}.png"))
            .unwrap();
    }

    fn btn<'a>(h: &'a Harness<'_, App>, label: &'a str) -> egui_kittest::Node<'a> {
        h.get_by_role_and_label(egui::accesskit::Role::Button, label)
    }

    #[test]
    fn arabic_note() {
        let mut h = fresh();
        h.state_mut().notes[0].body =
            "# ملاحظاتي\n\nمرحبا بالعالم، هذه **ملاحظة** باللغة العربية.\n- [ ] شراء الخبز\n- [x] كتابة التقرير\n- عنصر في قائمة\n\nEnglish line with عربي inside.".into();
        h.state_mut().notes[0].title = "ملاحظات الأسبوع".into();
        h.state_mut().view = View::Split;
        h.run_steps(3);
        shot(&mut h, "15-arabic");
    }

    #[test]
    fn screenshots() {
        let mut h = fresh();
        shot(&mut h, "01-edit");
        h.state_mut().view = View::Split;
        h.run_steps(3);
        shot(&mut h, "02-split");
        h.state_mut().view = View::Preview;
        h.run_steps(3);
        shot(&mut h, "03-preview");
        h.state_mut().open("n2".into());
        h.run_steps(3);
        shot(&mut h, "04-chart");
        h.state_mut().calendar = true;
        h.run_steps(3);
        shot(&mut h, "13-calendar");
        h.state_mut().calendar = false;
        h.state_mut().ai_open = true;
        h.state_mut()
            .msgs
            .push(Msg::new("user", "x".into(), "Summarize".into()));
        h.state_mut().msgs.push(Msg::new(
            "assistant",
            "- Sales peaked in April\n- Web is the main channel".into(),
            String::new(),
        ));
        h.state_mut().dark = true;
        h.run_steps(4);
        shot(&mut h, "06-dark-ai");
        h.state_mut().actions_open = false;
        h.run_steps(4);
        shot(&mut h, "14-ai-actions-hidden");
        h.state_mut().actions_open = true;
        h.state_mut().settings_open = true;
        h.run_steps(4);
        shot(&mut h, "07-settings");
        h.state_mut().settings_open = false;
        h.state_mut().confirm_delete = true;
        h.state_mut().dark = false;
        h.run_steps(4);
        shot(&mut h, "08-delete");
        h.state_mut().confirm_delete = false;
        btn(&h, "More").click();
        h.run_steps(3);
        shot(&mut h, "09-menu");
    }

    #[test]
    fn agenda_ticks_todos() {
        let today = chrono::Local::now().date_naive();
        let mut notes = model::load_notes_default();
        notes.push(Note {
            id: "d1".into(),
            title: calendar::daily_title(today),
            body: "# Today\n\n- [ ] 15:00 Dentist with a long name\n- [x] Gym".into(),
            tags: String::new(),
            updated: 1,
            pinned: false,
        });
        let mut h = harness(App::from_parts(notes, Settings::default()));
        h.state_mut().calendar = true;
        h.run_steps(4);
        shot(&mut h, "19-calendar-agenda");
        assert!(h.state().notes.iter().all(|n| n.id != "d1"), "the daily note left the notes list");
        h.get_by_label("15:00  Dentist with a long name").click();
        h.run_steps(3);
        let body = &h.state().days.iter().find(|n| n.id == "d1").unwrap().body;
        assert!(body.contains("- [x] 15:00 Dentist"), "{body}");
        assert!(!body.contains("# Today"), "the old heading is dropped");
        assert!(h.state().calendar, "ticking stays in the calendar");
    }

    #[test]
    fn trash_icon_deletes_and_undo_restores() {
        let mut h = fresh();
        let before = h.state().notes.len();
        let title = h.state().notes[1].title.clone();
        let id = h.state().notes[1].id.clone();
        h.get_by_label(&format!("Trash “{title}”")).hover();
        h.run_steps(2);
        shot(&mut h, "20-trash-hover");
        h.get_by_label(&format!("Trash “{title}”")).click();
        h.run_steps(3);
        assert_eq!(h.state().notes.len(), before - 1);
        assert!(h.state().notes.iter().all(|n| n.id != id));
        shot(&mut h, "21-trash-undo");
        h.get_by_label("Undo").click();
        h.run_steps(3);
        assert_eq!(h.state().notes.len(), before);
        assert_eq!(h.state().notes[1].id, id, "back in its place");
        assert!(h.state().trashed.is_none());
    }

    #[test]
    fn sidebar_and_topbar_fold() {
        let mut h = fresh();
        assert!(h.query_by_label("Search notes or #tags…").is_some() || h.query_by_label("+ New").is_some());
        h.get_by_label("Hide sidebar").click();
        h.run_steps(3);
        assert!(!h.state().sidebar_open);
        assert!(h.query_by_label("+ New").is_none());
        shot(&mut h, "22-sidebar-hidden");
        h.get_by_label("Hide top bar").click();
        h.run_steps(3);
        assert!(!h.state().topbar_open);
        assert!(h.query_by_label("Calendar").is_none());
        assert!(h.query_by_label("Link").is_none(), "formatting toolbar folds too");
        shot(&mut h, "23-both-folded");
        h.get_by_label("Show sidebar").click();
        h.run_steps(3);
        assert!(h.state().sidebar_open);
        shot(&mut h, "24-topbar-folded");
        h.get_by_label("Show top bar").click();
        h.run_steps(3);
        assert!(h.state().topbar_open);
        assert!(h.query_by_label("Calendar").is_some());
        // shortcuts
        h.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
        h.run_steps(2);
        assert!(!h.state().sidebar_open);
        h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Backslash);
        h.run_steps(2);
        assert!(!h.state().topbar_open);
        assert!(h.state().sidebar_open == false, "shift version leaves the sidebar alone");
        // Ctrl+F brings the sidebar back for the search box
        h.key_press_modifiers(Modifiers::COMMAND, Key::F);
        h.run_steps(2);
        assert!(h.state().sidebar_open);
        let saved = model::load_settings();
        assert!(saved.sidebar_open && !saved.topbar_open, "remembered");
    }

    #[test]
    fn talk_button() {
        let mut h = fresh();
        h.state_mut().ai_open = true;
        h.run_steps(4);
        shot(&mut h, "26-talk-button");
        // No whisper in the test data dir: a clear message, and nothing records.
        h.get_by_label("Talk").click();
        h.run_steps(3);
        assert!(matches!(h.state().voice, voice::Voice::Idle));
        assert!(h.state().err.contains("Voice needs whisper.cpp"), "{}", h.state().err);
    }

    #[test]
    fn quick_actions_collapse() {
        let mut h = fresh();
        h.state_mut().ai_open = true;
        h.run_steps(4);
        assert!(h.query_by_label("Find dates").is_some());
        h.state_mut().actions_open = false; // set directly: clicking would save the real settings.json
        h.run_steps(4);
        assert!(h.query_by_label("Find dates").is_none());
        assert!(
            h.query_by_label("Let Pengu edit notes & calendar")
                .is_none()
        );
        assert!(h.query_by_label("QUICK ACTIONS").is_some());
    }

    #[test]
    fn view_and_panel_buttons() {
        let mut h = fresh();
        {
            let a = h.state_mut();
            a.notes[0].body = "# Welcome\n\n**Notes save automatically.**\n\nSome *italic*, ~~strike~~, ==mark==, `code` and [[Sales chart]].\n- [x] done\n- [ ] todo".into();
            a.refocus = true;
            a.pending_sel = Some((0, 0));
        }
        h.run_steps(3);
        shot(&mut h, "10-live-bold");
        btn(&h, "Split").click();
        h.run_steps(2);
        assert_eq!(h.state().view, View::Split);
        btn(&h, "Preview").click();
        h.run_steps(2);
        assert_eq!(h.state().view, View::Preview);
        btn(&h, "Edit").click();
        h.run_steps(2);
        assert_eq!(h.state().view, View::Edit);
        btn(&h, "Pengu").click();
        h.run_steps(2);
        assert!(h.state().ai_open);
        btn(&h, "Settings").click();
        h.run_steps(2);
        assert!(h.state().settings_open);
        btn(&h, "Done").click();
        h.run_steps(2);
        assert!(!h.state().settings_open);
        btn(&h, "☆ Pin").click();
        h.run_steps(2);
        assert!(h.state().notes[0].pinned);
    }

    #[test]
    fn more_menu_items() {
        let mut h = fresh();
        btn(&h, "More").click();
        h.run_steps(2);
        btn(&h, "Dark theme").click();
        h.run_steps(2);
        assert!(h.state().dark);
        btn(&h, "More").click();
        h.run_steps(2);
        btn(&h, "Pengu settings…").click();
        h.run_steps(2);
        assert!(h.state().settings_open);
        h.state_mut().settings_open = false;
        h.run_steps(2);
        btn(&h, "More").click();
        h.run_steps(2);
        btn(&h, "Delete note").click();
        h.run_steps(2);
        assert!(h.state().confirm_delete);
        btn(&h, "Delete").click();
        h.run_steps(2);
        assert_eq!(h.state().notes.len(), 1);
        assert!(!h.state().confirm_delete);
    }

    #[test]
    fn new_note_and_toolbar() {
        let mut h = fresh();
        btn(&h, "+ New").click();
        h.run_steps(2);
        assert_eq!(h.state().notes.len(), 3);
        btn(&h, "B").click();
        h.run_steps(2);
        assert_eq!(h.state().notes[0].body, "****");
        btn(&h, "B").click();
        h.run_steps(2);
        assert_eq!(h.state().notes[0].body, "", "bold toggles off");
        btn(&h, "H").click();
        h.run_steps(2);
        assert_eq!(h.state().notes[0].body, "## ");
        btn(&h, "Insert").click();
        h.run_steps(2);
        btn(&h, "Bar chart").click();
        h.run_steps(2);
        assert!(h.state().notes[0].body.contains("```chart bar"));
    }

    #[test]
    fn toolbar_uses_cursor_of_current_note() {
        let mut h = fresh();
        // Focus a different note's editor first, then switch: the old cursor must not leak.
        h.state_mut().sel = Some((3, 5));
        h.state_mut().open("n2".into());
        h.run_steps(2);
        btn(&h, "Insert").click();
        h.run_steps(2);
        btn(&h, "Divider").click();
        h.run_steps(2);
        assert!(h.state().notes[1].body.ends_with("\n---\n"));
    }

    #[test]
    fn sidebar_card_and_checkbox_and_wikilink() {
        let mut h = fresh();
        h.get_by_label("Sales chart").click();
        h.run_steps(2);
        assert_eq!(h.state().active.as_deref(), Some("n2"));
        h.state_mut().open("n1".into());
        h.state_mut().view = View::Preview;
        h.run_steps(3);
        h.get_all_by_role(egui::accesskit::Role::CheckBox)
            .nth(1)
            .unwrap()
            .click();
        h.run_steps(2);
        assert!(h.state().notes[0].body.contains("- [x] Connect"));
        h.get_by_role_and_label(egui::accesskit::Role::Link, "Sales chart")
            .click();
        h.run_steps(2);
        assert_eq!(h.state().active.as_deref(), Some("n2"));
    }

    #[test]
    fn editor_checkbox_toggles() {
        let mut h = fresh();
        assert!(h.state().notes[0].body.contains("- [ ] Connect"));
        let ed = egui::Id::new(("body", "n1"));
        let r = h
            .ctx
            .read_response(ed.with(("task", 5usize)))
            .expect("checkbox for line 5")
            .rect;
        h.hover_at(r.center());
        h.run_steps(1);
        h.drag_at(r.center());
        h.run_steps(1);
        h.drop_at(r.center());
        h.run_steps(3);
        shot(&mut h, "11-tasks");
        assert!(h.state().notes[0].body.contains("- [x] Connect"));
    }

    #[test]
    fn narrow_window_topbar_does_not_overlap() {
        let mut h = fresh();
        h.state_mut().ai_open = true;
        h.state_mut().dark = true;
        h.state_mut().ai.model = "a-really-long-model-name-like-llama-8b-instruct-q4".into();
        h.set_size(Vec2::new(920.0, 600.0));
        h.run_steps(4);
        shot(&mut h, "12-narrow");
        let rects: Vec<egui::Rect> = [
            "Edit", "Split", "Preview", "☆ Pin", "Pengu", "More",
        ]
        .iter()
        .map(|l| btn(&h, l).rect())
        .collect();
        for (i, a) in rects.iter().enumerate() {
            for b in &rects[i + 1..] {
                assert!(
                    !a.shrink(1.0).intersects(b.shrink(1.0)),
                    "{a:?} overlaps {b:?}"
                );
            }
        }
        let settings = btn(&h, "Settings").rect();
        let model = h.get_by_label_contains("a-really-long").rect();
        assert!(
            model.right() <= settings.left(),
            "model name runs under Settings"
        );
        btn(&h, "Calendar").click();
        h.run_steps(3);
        shot(&mut h, "14-narrow-calendar");
        let row: Vec<egui::Rect> = ["‹", "Today", "›"]
            .iter()
            .map(|l| btn(&h, l).rect())
            .collect();
        let title = h
            .get_by_label(&chrono::Local::now().format("%B %Y").to_string())
            .rect();
        for r in &row {
            assert!(
                !title.intersects(r.shrink(1.0)),
                "month title overlaps {r:?}"
            );
        }
    }

    #[test]
    fn calendar_day_has_its_own_page() {
        let mut h = fresh();
        btn(&h, "Calendar").click();
        h.run_steps(3);
        assert!(h.state().calendar);
        let today = chrono::Local::now().date_naive();
        // a day in this month other than today, so selecting it is visible
        let day = if (today + chrono::Duration::days(1)).month() == today.month() {
            today + chrono::Duration::days(1)
        } else {
            today - chrono::Duration::days(1)
        };
        let notes_before = h.state().notes.len();
        // Click an empty corner of the cell: it selects the day in the agenda.
        let cell = h
            .get_by_role_and_label(egui::accesskit::Role::Button, &day.format("%-d %B").to_string())
            .rect();
        let at = cell.right_bottom() - Vec2::new(12.0, 8.0);
        h.hover_at(at);
        h.run_steps(1);
        h.drag_at(at);
        h.run_steps(1);
        h.drop_at(at);
        h.run_steps(3);
        assert_eq!(h.state().cal.sel, day);
        assert!(h.state().cal.page.is_none());
        // "Open day" shows the day's own page; nothing is made until something is written
        btn(&h, "Open day").click();
        h.run_steps(3);
        assert_eq!(h.state().cal.page, Some(day));
        assert!(h.state().calendar, "the page lives in the calendar, not the editor");
        assert!(h.state().days.is_empty());

        h.get_by_label("New to-do").click();
        h.run_steps(1);
        h.get_by_label("New to-do").type_text("Dentist");
        h.get_by_label("To-do time").click();
        h.run_steps(1);
        h.get_by_label("To-do time").type_text("15");
        h.run_steps(1);
        btn(&h, "Add").click();
        h.run_steps(3);
        h.get_by_label("Day notes").click();
        h.run_steps(1);
        h.get_by_label("Day notes").type_text("bring the insurance card");
        h.run_steps(3);
        shot(&mut h, "22-day-page");
        let pg = calendar::page(&h.state().days, day).expect("day page made").body.clone();
        assert_eq!(pg, "- [ ] 15:00 Dentist\n\nbring the insurance card");
        assert_eq!(h.state().notes.len(), notes_before, "notes are for notes only");

        btn(&h, "Remove “Dentist”").click();
        h.run_steps(3);
        assert_eq!(calendar::page(&h.state().days, day).unwrap().body, "bring the insurance card");

        btn(&h, "›").click();
        h.run_steps(3);
        assert_eq!(h.state().cal.page, Some(day + chrono::Duration::days(1)));
        btn(&h, "‹ Month").click();
        h.run_steps(3);
        assert!(h.state().cal.page.is_none());
        shot(&mut h, "23-month-with-page");
        btn(&h, "›").click();
        h.run_steps(2);
        assert_ne!(h.state().cal.month, today.with_day(1).unwrap());
    }

    /// Serve one streaming chat reply the way OpenAI-compatible APIs do (SSE).
    fn fake_server(reply: &'static str) -> String {
        use std::io::{BufRead, BufReader, Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut len = 0;
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            assert!(String::from_utf8_lossy(&body).contains("You are Pengu"));
            let mut out = String::from(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
            );
            for chunk in reply.as_bytes().chunks(20) {
                let piece = String::from_utf8_lossy(chunk);
                out += &format!(
                    "data: {}\n\n",
                    serde_json::json!({"choices": [{"delta": {"content": piece}}]})
                );
            }
            out += "data: [DONE]\n\n";
            s.write_all(out.as_bytes()).unwrap();
        });
        url
    }

    #[test]
    fn pengu_sorts_words_into_note_and_calendar() {
        let reply = "Sorted! I made a shopping list and added the dentist to your calendar.\n```pengu\n[{\"action\":\"create_note\",\"title\":\"Shopping\",\"tags\":\"errands\",\"body\":\"# Shopping\\n\\n- [ ] milk\\n- [ ] eggs\"},{\"action\":\"schedule\",\"date\":\"2026-10-20\",\"time\":\"15:00\",\"text\":\"Dentist\"}]\n```";
        let url = fake_server(reply);
        let mut h = fresh();
        h.state_mut().ai = AiCfg {
            preset: "Custom".into(),
            url,
            model: "test-model".into(),
            key: "sk-test".into(),
        };
        h.state_mut().ai_open = true;
        h.run_steps(2);
        h.state_mut().draft = "milk eggs, dentist next tuesday 3pm".into();
        btn(&h, "Send").click();
        for _ in 0..200 {
            h.run_steps(1);
            if !h.state().busy {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(h.state().err.is_empty(), "{}", h.state().err);
        let a = h.state();
        let shop = a
            .notes
            .iter()
            .find(|n| n.title == "Shopping")
            .expect("note created");
        assert!(shop.body.contains("- [ ] eggs"));
        assert!(a.notes.iter().all(|n| n.title != "2026-10-20"), "no daily note among the notes");
        let day = a
            .days
            .iter()
            .find(|n| n.title == "2026-10-20")
            .expect("day page");
        assert_eq!(day.body, "- [ ] 15:00 Dentist");
        assert_eq!(
            a.active.as_deref(),
            Some(shop.id.as_str()),
            "opens the new note"
        );
        h.run_steps(3);
        shot(&mut h, "15-pengu");
        h.state_mut().calendar = true;
        h.state_mut().cal.month = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        h.run_steps(3);
        shot(&mut h, "16-pengu-calendar");
        h.state_mut().calendar = false;
        h.run_steps(2);
        btn(&h, "Undo").click();
        h.run_steps(2);
        assert!(!h.state().notes.iter().any(|n| n.title == "Shopping"));
        assert!(h.state().days.is_empty(), "undo removes the day page too");
    }

    #[test]
    fn ctrl_n_creates_note() {
        let mut h = fresh();
        h.key_press_modifiers(Modifiers::COMMAND, Key::N);
        h.run_steps(2);
        assert_eq!(h.state().notes.len(), 3);
    }
}
