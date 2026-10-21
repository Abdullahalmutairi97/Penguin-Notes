//! iPhone sync: iCloud calendar (CalDAV, background thread), Apple Notes (IMAP, background
//! thread) and the Markdown notes folder.
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use chrono::NaiveDate;
use eframe::egui::{self, Align, Layout as ELayout, RichText, Stroke};
use serde::{Deserialize, Serialize};

use super::App;
use crate::applenotes;
use crate::icloud::{self, Creds, LocalOp, Outcome};
use crate::ics::Event;
use crate::mirror;
use crate::model::{self, Note};
use crate::pengu;
use crate::theme::{Palette, bold, caps, pill, primary};

const EVERY: Duration = Duration::from_secs(5 * 60);
const AFTER_EDIT: Duration = Duration::from_secs(4);

/// Saved separately from settings.json; holds the app-specific password, so it's 0600.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SyncCfg {
    #[serde(default)]
    pub icloud: Creds,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub apple: applenotes::Creds,
}

fn load<T: for<'de> Deserialize<'de> + Default>(name: &str) -> T {
    std::fs::read_to_string(model::data_dir().join(name))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save<T: Serialize>(name: &str, v: &T) {
    if let Ok(s) = serde_json::to_string(v) {
        model::write_atomic(name, &s);
    }
}

#[derive(Default)]
pub struct SyncRt {
    pub cfg: SyncCfg,
    pub state: icloud::State,
    mirror: mirror::State,
    rx: Option<Receiver<Result<Outcome, String>>>,
    pub msg: String,
    pub failed: bool,
    last: Option<Instant>,
    sent_sig: u64,
    cur_sig: u64,
    sig_at: Option<Instant>,
    mirror_at: Option<Instant>,
    mirror_msg: String,
    pub open: bool,
    force: bool,
    apple: AppleRt,
}

/// Apple Notes runs: one at a time, every minute and shortly after an edit.
#[derive(Default)]
struct AppleRt {
    state: applenotes::State,
    rx: Option<Receiver<Result<applenotes::Outcome, String>>>,
    msg: String,
    failed: bool,
    last: Option<Instant>,
    checked: Option<Instant>,
    sent_sig: u64,
    cur_sig: u64,
    sig_at: Option<Instant>,
    force: bool,
}

const APPLE_EVERY: Duration = Duration::from_secs(60);

fn notes_sig(notes: &[Note]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for n in notes {
        (&n.id, &n.title, &n.body).hash(&mut h);
    }
    h.finish()
}

impl SyncRt {
    pub fn load() -> Self {
        Self {
            cfg: load("sync-settings.json"),
            state: load("calendar-sync.json"),
            mirror: load("folder-sync.json"),
            apple: AppleRt {
                state: load("applenotes-sync.json"),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Short status for the calendar header, or None when sync is off.
    pub fn status(&self) -> Option<String> {
        if !self.cfg.icloud.enabled {
            return None;
        }
        Some(if self.busy() {
            "Syncing with iPhone…".into()
        } else if self.failed {
            format!("iPhone sync failed: {}", self.msg)
        } else if self.state.last_ok > 0 {
            let t = chrono::DateTime::from_timestamp_millis(self.state.last_ok)
                .unwrap_or_default()
                .with_timezone(&chrono::Local);
            format!("iPhone · synced {}", t.format("%H:%M"))
        } else {
            "iPhone sync on".into()
        })
    }

    pub fn sync_now(&mut self) {
        self.force = true;
    }

    /// Apple Notes sync is set up.
    pub fn notes_on(&self) -> bool {
        self.cfg.apple.enabled
    }

    pub fn notes_busy(&self) -> bool {
        self.apple.rx.is_some()
    }

    /// Pull the iPhone's note changes now instead of on the next minute.
    pub fn notes_sync_now(&mut self) {
        self.apple.failed = false;
        self.apple.force = true;
    }

    /// Hover text for the refresh button.
    pub fn notes_status(&self) -> String {
        let a = &self.apple;
        if a.rx.is_some() {
            "Syncing with iPhone…".into()
        } else if a.failed {
            format!("iPhone sync failed: {} (click to retry)", a.msg)
        } else if a.state.last_ok > 0 {
            let t = chrono::DateTime::from_timestamp_millis(a.state.last_ok)
                .unwrap_or_default()
                .with_timezone(&chrono::Local);
            format!("Synced with iPhone at {}. Click to sync now", t.format("%H:%M"))
        } else {
            "Sync with iPhone now".into()
        }
    }
}

/// Every to-do on a day page, as (date, line).
pub fn items(days: &[Note]) -> Vec<(NaiveDate, String)> {
    let mut v = vec![];
    for n in days {
        let Some(date) = model::day_date(n) else {
            continue;
        };
        for line in n.body.lines() {
            if Event::from_line(date, line).is_some() {
                v.push((date, line.trim_end().to_string()));
            }
        }
    }
    v
}

fn sig(items: &[(NaiveDate, String)]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    items.hash(&mut h);
    h.finish()
}

impl App {
    pub(super) fn sync_tick(&mut self, ctx: &egui::Context) {
        // Calendar: collect a finished run.
        if let Some(Ok(res)) = self.sync.rx.as_ref().map(|rx| rx.try_recv()) {
            self.sync.rx = None;
            match res {
                Ok(out) => {
                    for op in &out.ops {
                        match op {
                            LocalOp::Add { date, line } => {
                                pengu::add_to_daily(&mut self.days, *date, line)
                            }
                            LocalOp::Remove { date, line } => {
                                pengu::remove_from_daily_pages(&mut self.days, *date, line)
                            }
                        }
                    }
                    if !out.ops.is_empty() {
                        self.dirty = true;
                    }
                    self.sync.state = out.state;
                    save("calendar-sync.json", &self.sync.state);
                    self.sync.failed = false;
                    self.sync.msg = format!("Synced {}", chrono::Local::now().format("%H:%M"));
                    let s = sig(&items(&self.days));
                    self.sync.sent_sig = s;
                    self.sync.cur_sig = s;
                }
                Err(e) => {
                    self.sync.failed = true;
                    self.sync.msg = e;
                }
            }
        }
        // Calendar: start a run when due.
        if self.sync.cfg.icloud.enabled && self.sync.rx.is_none() {
            let list = items(&self.days);
            let s = sig(&list);
            if s != self.sync.cur_sig {
                self.sync.cur_sig = s;
                self.sync.sig_at = Some(Instant::now());
            }
            let edited = s != self.sync.sent_sig
                && self.sync.sig_at.is_some_and(|t| t.elapsed() > AFTER_EDIT);
            let due = self.sync.last.is_none_or(|t| t.elapsed() > EVERY);
            // After a failure, only retry on the timer or when asked.
            if self.sync.force || due || (edited && !self.sync.failed) {
                self.sync.force = false;
                self.sync.last = Some(Instant::now());
                self.sync.sent_sig = s;
                let (tx, rx) = channel();
                self.sync.rx = Some(rx);
                let (creds, state, c) = (
                    self.sync.cfg.icloud.clone(),
                    self.sync.state.clone(),
                    ctx.clone(),
                );
                std::thread::spawn(move || {
                    let _ = tx.send(icloud::sync(&creds, &list, state));
                    c.request_repaint();
                });
            }
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        self.apple_tick(ctx);
        // Notes folder.
        if !self.sync.cfg.folder.trim().is_empty()
            && self
                .sync
                .mirror_at
                .is_none_or(|t| t.elapsed() > Duration::from_secs(3))
        {
            self.sync.mirror_at = Some(Instant::now());
            let dir = expand(&self.sync.cfg.folder);
            if self.sync.mirror.dir != dir {
                // New folder: forget what we knew about the old one.
                self.sync.mirror = mirror::State {
                    dir,
                    ..Default::default()
                };
            }
            if mirror::import(&mut self.notes, &mut self.sync.mirror) {
                self.dirty = true;
                if self.idx().is_none() {
                    self.active = self.notes.first().map(|n| n.id.clone());
                }
            }
            self.sync.mirror_msg = match mirror::export(&self.notes, &mut self.sync.mirror) {
                Ok(()) => format!("{} notes in the folder", self.notes.len()),
                Err(e) => e,
            };
            save("folder-sync.json", &self.sync.mirror);
            ctx.request_repaint_after(Duration::from_secs(3));
        }
    }

    fn apple_tick(&mut self, ctx: &egui::Context) {
        let a = &mut self.sync.apple;
        if let Some(Ok(res)) = a.rx.as_ref().map(|rx| rx.try_recv()) {
            a.rx = None;
            match res {
                Ok(mut out) => {
                    for op in &out.ops {
                        match op {
                            applenotes::Op::Put { id, title, body, base } => {
                                match self.notes.iter_mut().find(|n| &n.id == id) {
                                    // Edited here while the run was out: keep ours, it goes up next time.
                                    Some(n) if Some(n.updated) != *base => {
                                        if let Some(e) = out.state.notes.get_mut(id) {
                                            e.hash = 0;
                                        }
                                    }
                                    Some(n) => {
                                        n.title = title.clone();
                                        n.body = body.clone();
                                        n.updated = model::now_ms();
                                    }
                                    None if base.is_none() => self.notes.insert(
                                        0,
                                        Note {
                                            id: id.clone(),
                                            title: title.clone(),
                                            body: body.clone(),
                                            tags: String::new(),
                                            updated: model::now_ms(),
                                            pinned: false,
                                        },
                                    ),
                                    // Deleted here meanwhile.
                                    None => {
                                        out.state.notes.remove(id);
                                    }
                                }
                            }
                            applenotes::Op::Delete { id, base } => {
                                self.notes.retain(|n| &n.id != id || n.updated != *base);
                            }
                        }
                    }
                    if !out.ops.is_empty() {
                        self.dirty = true;
                        if self.idx().is_none() {
                            self.active = self.notes.first().map(|n| n.id.clone());
                        }
                    }
                    let a = &mut self.sync.apple;
                    a.state = out.state;
                    save("applenotes-sync.json", &a.state);
                    a.failed = false;
                    a.msg = format!("Synced {}", chrono::Local::now().format("%H:%M"));
                    let s = notes_sig(&self.notes);
                    a.sent_sig = s;
                    a.cur_sig = s;
                }
                Err(e) => {
                    a.failed = true;
                    a.msg = e;
                }
            }
        }
        let a = &mut self.sync.apple;
        if !self.sync.cfg.apple.enabled || a.rx.is_some() || a.checked.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        a.checked = Some(Instant::now());
        let s = notes_sig(&self.notes);
        if s != a.cur_sig {
            a.cur_sig = s;
            a.sig_at = Some(Instant::now());
        }
        let edited = s != a.sent_sig && a.sig_at.is_some_and(|t| t.elapsed() > AFTER_EDIT);
        let due = a.last.is_none_or(|t| t.elapsed() > APPLE_EVERY);
        if a.force || due || (edited && !a.failed) {
            a.force = false;
            a.last = Some(Instant::now());
            a.sent_sig = s;
            let (tx, rx) = channel();
            a.rx = Some(rx);
            let (creds, notes, state, c) = (self.sync.cfg.apple.clone(), self.notes.clone(), a.state.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(applenotes::sync(&creds, &notes, state));
                c.request_repaint();
            });
        }
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    pub(super) fn sync_modal(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.sync.open {
            return;
        }
        let mut done = false;
        let mut changed = false;
        let frame = egui::Frame::window(&ctx.global_style()).inner_margin(18);
        let resp = egui::Modal::new(egui::Id::new("sync")).frame(frame).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("iPhone sync").size(21.0).family(bold()));
                ui.with_layout(ELayout::right_to_left(Align::Center), |ui| {
                    done = pill(ui, p, "Done", false).clicked();
                });
            });
            ui.add_space(8.0);
            let max_h = (ctx.content_rect().height() - 120.0).max(200.0);
            egui::ScrollArea::vertical().max_height(max_h).min_scrolled_height(max_h.min(560.0)).auto_shrink([false, true]).show(ui, |ui| {
            ui.set_width(450.0);

            // ---- Calendar
            ui.label(RichText::new("Calendar").size(17.0).family(bold()));
            ui.label(
                RichText::new("The to-dos on your day pages go to a “Penguin Notes” calendar on your iPhone, and events you add or change there come back here. Events from your other iPhone calendars show up in the calendar view.")
                    .size(13.0)
                    .color(p.ink_2),
            );
            ui.add_space(4.0);
            let field = |ui: &mut egui::Ui, label: &str, val: &mut String, hint: &str, pw: bool| -> bool {
                ui.add_space(2.0);
                ui.label(caps(label, p, 10.0));
                ui.add(egui::TextEdit::singleline(val).hint_text(hint).password(pw).font(egui::FontId::monospace(12.5)).margin(egui::Margin::symmetric(8, 6)).desired_width(f32::INFINITY)).changed()
            };
            changed |= field(ui, "Apple ID email", &mut self.sync.cfg.icloud.user, "you@icloud.com", false);
            changed |= field(ui, "App-specific password", &mut self.sync.cfg.icloud.pass, "xxxx-xxxx-xxxx-xxxx", true);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.label(RichText::new("Make one at ").size(12.5).color(p.ink_3));
                ui.hyperlink_to(RichText::new("account.apple.com").size(12.5), "https://account.apple.com/account/manage");
                ui.label(RichText::new(" > Sign-In and Security > App-Specific Passwords. Not your normal Apple ID password.").size(12.5).color(p.ink_3));
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let on = self.sync.cfg.icloud.enabled;
                if primary(ui, p, if on { "Sync now" } else { "Connect & sync" }).clicked() {
                    self.sync.cfg.icloud.enabled = true;
                    self.sync.failed = false;
                    self.sync.force = true;
                    changed = true;
                }
                if on && pill(ui, p, "Turn off", false).clicked() {
                    self.sync.cfg.icloud.enabled = false;
                    changed = true;
                }
                let (txt, col) = if self.sync.busy() {
                    ("Syncing…".to_string(), p.ink_2)
                } else if self.sync.failed {
                    (self.sync.msg.clone(), p.accent)
                } else {
                    (self.sync.msg.clone(), p.ink_2)
                };
                ui.add(egui::Label::new(RichText::new(txt).size(12.5).color(col)).wrap());
            });

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);

            // ---- Apple Notes
            ui.label(RichText::new("Apple Notes").size(17.0).family(bold()));
            ui.label(
                RichText::new("Every note goes to the Notes app on your iPhone through a mail account's Notes folder, and notes you write or change there come back here. They show as plain text: the Markdown is kept as it is.")
                    .size(13.0)
                    .color(p.ink_2),
            );
            ui.add_space(4.0);
            changed |= field(ui, "IMAP server", &mut self.sync.cfg.apple.host, "mail.example.com", false);
            changed |= field(ui, "User name", &mut self.sync.cfg.apple.user, "notes", false);
            changed |= field(ui, "Password", &mut self.sync.cfg.apple.pass, "", true);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let a = &mut self.sync.apple;
                let on = self.sync.cfg.apple.enabled;
                if primary(ui, p, if on { "Sync now" } else { "Connect & sync" }).clicked() {
                    self.sync.cfg.apple.enabled = true;
                    a.failed = false;
                    a.force = true;
                    changed = true;
                }
                if on && pill(ui, p, "Turn off", false).clicked() {
                    self.sync.cfg.apple.enabled = false;
                    changed = true;
                }
                let (txt, col) = if a.rx.is_some() {
                    ("Syncing…".to_string(), p.ink_2)
                } else if a.failed {
                    (a.msg.clone(), p.accent)
                } else {
                    (a.msg.clone(), p.ink_2)
                };
                ui.add(egui::Label::new(RichText::new(txt).size(12.5).color(col)).wrap());
            });
            ui.label(RichText::new("On the iPhone: Settings > Apps > Mail > Mail Accounts > Add Account > Other > Add Mail Account, with the same server and login, then turn on Notes for it.").size(12.5).color(p.ink_3));

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);

            // ---- Notes folder
            ui.label(RichText::new("Notes folder").size(17.0).family(bold()));
            ui.label(
                RichText::new("Every note can also be kept as a Markdown file in a folder. Sync that folder to your iPhone and open it in Obsidian (free): checkboxes, [[links]] and ==highlights== all work there, and edits on the phone come back here.")
                    .size(13.0)
                    .color(p.ink_2),
            );
            changed |= field(ui, "Notes folder (empty = off)", &mut self.sync.cfg.folder, "~/PenguinNotes", false);
            ui.horizontal(|ui| {
                if self.sync.cfg.folder.trim().is_empty() {
                    if pill(ui, p, "Use ~/PenguinNotes", true).clicked() {
                        self.sync.cfg.folder = "~/PenguinNotes".into();
                        changed = true;
                    }
                } else {
                    if pill(ui, p, "Open folder", false).clicked() {
                        let _ = std::process::Command::new("xdg-open").arg(expand(&self.sync.cfg.folder)).spawn();
                    }
                    ui.label(RichText::new(&self.sync.mirror_msg).size(12.5).color(p.ink_2));
                }
            });
            ui.add_space(4.0);
            egui::Frame::new().fill(p.paper).stroke(Stroke::new(1.0, p.line)).corner_radius(8).inner_margin(10).show(ui, |ui| {
                ui.label(RichText::new("Getting it onto the iPhone").size(13.5).family(bold()));
                for step in [
                    "1. Here: install Syncthing (sudo pacman -S syncthing) and run: systemctl --user enable --now syncthing",
                    "2. iPhone: install Möbius Sync and Obsidian from the App Store.",
                    "3. In Obsidian on the iPhone, create a vault named Penguin Notes (not in iCloud).",
                    "4. In Möbius Sync, add this computer, then share the notes folder into Obsidian's “Penguin Notes” folder.",
                ] {
                    ui.add(egui::Label::new(RichText::new(step).size(12.5).color(p.ink_2)).wrap());
                }
            });
            });
        });
        if changed {
            save("sync-settings.json", &self.sync.cfg);
        }
        if done || resp.should_close() {
            self.sync.open = false;
        }
    }
}

/// "~/x" -> "$HOME/x".
pub fn expand(p: &str) -> String {
    let p = p.trim();
    match (p.strip_prefix("~/"), std::env::var("HOME")) {
        (Some(rest), Ok(home)) => format!("{home}/{rest}"),
        _ => p.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icloud::tests::{Fake, creds, serve};
    use crate::model::Settings;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use std::sync::{Arc, Mutex};

    fn wait(h: &mut Harness<'_, App>) {
        for _ in 0..300 {
            h.run_steps(1);
            if !h.state().sync.busy() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("sync never finished");
    }

    fn shot(h: &mut Harness<'_, App>, name: &str) {
        let dir = format!("{}/tests-shots", env!("CARGO_MANIFEST_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        h.render()
            .expect("render")
            .save(format!("{dir}/{name}.png"))
            .unwrap();
    }

    #[test]
    fn calendar_syncs_both_ways_through_the_app() {
        let today = chrono::Local::now().date_naive();
        let fake = Arc::new(Mutex::new(Fake {
            cals: vec![("/123/calendars/home/".into(), "Home".into())],
            ..Default::default()
        }));
        let mut notes = model::load_notes_default();
        notes.push(Note {
            id: "d1".into(),
            title: crate::calendar::daily_title(today),
            body: "# Today\n\n- [ ] 15:00 Dentist".into(),
            tags: String::new(),
            updated: 1,
            pinned: false,
        });
        let mut app = App::from_parts(notes, Settings::default());
        app.sync.cfg.icloud = creds(serve(fake.clone()));
        let mut h = Harness::builder()
            .with_size([1280.0, 800.0])
            .wgpu()
            .build_ui_state(|ui, a: &mut App| a.draw(ui), app);
        h.run_steps(3);
        wait(&mut h);
        assert!(!h.state().sync.failed, "{}", h.state().sync.msg);
        assert!(
            fake.lock()
                .unwrap()
                .objects
                .values()
                .any(|(_, ics)| ics.contains("SUMMARY:Dentist"))
        );

        // Something added on the phone, in the Penguin Notes calendar and in Home.
        let cal = h.state().sync.state.cal_href.clone();
        let path = cal[cal.find("/123").unwrap()..].to_string();
        let gym = Event {
            uid: "p1".into(),
            date: today,
            time: "07:00".into(),
            text: "Gym".into(),
            done: false,
        };
        let bday = Event {
            uid: "h1".into(),
            date: today,
            time: String::new(),
            text: "Sara's birthday".into(),
            done: false,
        };
        fake.lock().unwrap().put_phone(&path, "p1", &gym.to_ics());
        fake.lock()
            .unwrap()
            .put_phone("/123/calendars/home/", "h1", &bday.to_ics());
        h.state_mut().sync.sync_now();
        h.run_steps(2);
        wait(&mut h);
        let day = h
            .state()
            .days
            .iter()
            .find(|n| n.id == "d1")
            .unwrap()
            .body
            .clone();
        assert!(day.contains("- [ ] 07:00 Gym"), "{day}");

        h.state_mut().calendar = true;
        h.run_steps(3);
        shot(&mut h, "17-iphone-calendar");
        assert!(h.query_by_label_contains("iPhone · synced").is_some());
        h.state_mut().sync.open = true;
        h.state_mut().calendar = false;
        h.run_steps(3);
        shot(&mut h, "18-iphone-sync-settings");
    }

    #[test]
    fn refresh_button_syncs_notes() {
        let app = App::from_parts(model::load_notes_default(), Settings::default());
        let mut h = Harness::builder()
            .with_size([1280.0, 800.0])
            .wgpu()
            .build_ui_state(|ui, a: &mut App| a.draw(ui), app);
        h.run_steps(3);
        assert!(h.query_by_label("Refresh notes").is_none(), "hidden until Apple Notes sync is on");
        // On, but with no server: every run fails at once without touching the network.
        h.state_mut().sync.cfg.apple.enabled = true;
        h.run_steps(3);
        for _ in 0..100 {
            h.run_steps(1);
            if !h.state().sync.notes_busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(h.state().sync.notes_status().starts_with("iPhone sync failed"));
        h.get_by_label("Refresh notes").click();
        h.run_steps(1);
        assert!(h.state().sync.apple.force || h.state().sync.notes_busy(), "click starts a sync");
        h.run_steps(3);
        shot(&mut h, "25-refresh-notes");
    }

    #[test]
    fn apple_notes_settings_render() {
        let mut app = App::from_parts(model::load_notes_default(), Settings::default());
        app.sync.cfg.apple = applenotes::Creds { host: "vps.example.ts.net".into(), user: "notes".into(), pass: "x".into(), enabled: false };
        app.sync.open = true;
        let mut h = Harness::builder()
            .with_size([1280.0, 1100.0])
            .wgpu()
            .build_ui_state(|ui, a: &mut App| a.draw(ui), app);
        h.run_steps(3);
        assert!(h.query_by_label_contains("Apple Notes").is_some());
        assert!(h.state().sync.apple.rx.is_none(), "off until connected");
        shot(&mut h, "19-apple-notes-settings");
    }
}
