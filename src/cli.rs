//! Commands for the desktop widget, run without a window:
//!   penguin-notes ask "message"      Pengu answers on stdout, and applies its changes
//!   penguin-notes transcribe x.wav   the spoken words on stdout (Talk's language setting)
//!   penguin-notes voice-lang [en|ar] print or set that language
//! A Penguin Notes window that is open notices the saved files and merges them in.
use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;

use crate::ai::{self, Ev};
use crate::model;
use crate::pengu;
use crate::voice;

/// Returns the exit code when the arguments are a command, None to start the app.
pub fn run() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let res = match args.first().map(String::as_str)? {
        "ask" => ask(&args[1..]),
        "transcribe" => match args.get(1) {
            Some(f) => voice::transcribe(std::path::Path::new(f), &voice::lang()),
            None => Err("usage: penguin-notes transcribe <file.wav>".into()),
        },
        "voice-lang" => match args.get(1).map(String::as_str) {
            None => Ok(voice::lang()),
            Some(l @ ("en" | "ar" | "auto")) => {
                voice::set_lang(l);
                Ok(l.to_string())
            }
            Some(_) => Err("usage: penguin-notes voice-lang [en|ar]".into()),
        },
        _ => return None,
    };
    Some(match res {
        Ok(out) => {
            println!("{out}");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    })
}

/// The widget's short conversation: kept while it is in use, forgotten after a quiet spell.
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct Chat {
    at: i64,
    turns: Vec<(String, String)>,
}

const FORGET_MS: i64 = 15 * 60 * 1000;
const TURNS: usize = 8;

fn ask(args: &[String]) -> Result<String, String> {
    let mut text = args.join(" ");
    if text.trim() == "-" || text.trim().is_empty() {
        text.clear();
        std::io::stdin().read_to_string(&mut text).map_err(|e| e.to_string())?;
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("usage: penguin-notes ask \"message\"".into());
    }
    let s = model::load_settings();
    if s.ai.model.trim().is_empty() {
        return Err("pick a model in Penguin Notes > Pengu settings first".into());
    }
    let mut notes = model::read_list("notes.json").unwrap_or_default();
    let mut days = model::load_days();
    let mut trash = pengu::load_trash();
    let mut chat: Chat = std::fs::read_to_string(model::data_dir().join("widget-chat.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    if model::now_ms() - chat.at > FORGET_MS {
        chat.turns.clear();
    }
    let mut msgs = vec![("system".to_string(), pengu::system_prompt(&notes, &days, None, &trash))];
    for (u, a) in &chat.turns {
        msgs.push(("user".into(), u.clone()));
        msgs.push(("assistant".into(), a.clone()));
    }
    msgs.push(("user".into(), text.clone()));

    let (tx, rx) = channel();
    ai::chat(s.ai.clone(), msgs, tx, Arc::new(AtomicBool::new(false)), eframe::egui::Context::default());
    let mut reply = String::new();
    for ev in rx {
        match ev {
            Ev::Chunk(c) => reply += &c,
            Ev::Done => break,
            Ev::Err(e) => return Err(e),
            Ev::Models(_) => {}
        }
    }
    let acts = pengu::parse(&reply);
    let mut said = pengu::visible(&reply).to_string();
    if acts.is_empty() && reply.contains("```pengu") {
        said += "\n(Pengu meant to change something, but its changes couldn't be read. Nothing was saved; try asking again.)";
    }
    if !acts.is_empty() {
        if s.pengu_edits {
            pengu::apply(&mut notes, &mut days, &mut trash, &acts);
            model::save_notes(&notes);
            model::save_days(&days);
            pengu::save_trash(&trash);
        } else {
            said += "\n(Pengu isn't allowed to edit; turn it on in Penguin Notes.)";
        }
    }
    chat.turns.push((text, reply));
    let n = chat.turns.len();
    chat.turns.drain(..n.saturating_sub(TURNS));
    chat.at = model::now_ms();
    if let Ok(j) = serde_json::to_string(&chat) {
        model::write_atomic("widget-chat.json", &j);
    }
    Ok(said)
}
