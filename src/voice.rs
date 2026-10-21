//! Talking to Pengu: record the mic with PipeWire (pw-record, or arecord), then turn the
//! recording into text with whisper.cpp on this machine. Nothing leaves the computer.
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use crate::model;

pub const MODEL: &str = "ggml-small-q5_1.bin";

/// whisper.cpp lives next to the notes: <data>/whisper/{whisper-cli, model}.
pub fn whisper_dir() -> PathBuf {
    // PN_WHISPER_DIR: lets the tests use a real install (data_dir is a temp dir there).
    std::env::var_os("PN_WHISPER_DIR").map(PathBuf::from).unwrap_or_else(|| model::data_dir().join("whisper"))
}

fn cli() -> PathBuf {
    whisper_dir().join("whisper-cli")
}

fn model_path() -> PathBuf {
    whisper_dir().join(MODEL)
}

/// The spoken language for Talk, "en" or "ar" ("auto" still works but guesses badly).
/// Its own file, so the app, the widget and `penguin-notes transcribe` always agree.
pub fn lang() -> String {
    let l = std::fs::read_to_string(model::data_dir().join("voice-lang")).unwrap_or_default();
    match l.trim() {
        "ar" => "ar".into(),
        "auto" => "auto".into(),
        _ => "en".into(),
    }
}

pub fn set_lang(l: &str) {
    model::write_atomic("voice-lang", l);
}

pub fn installed() -> bool {
    cli().is_file() && model_path().is_file()
}

/// A running recorder; dropping it (app closed mid-recording) stops it and removes the file.
pub struct Rec {
    child: Option<Child>,
    file: PathBuf,
}

impl Drop for Rec {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
            let _ = std::fs::remove_file(&self.file);
        }
    }
}

#[derive(Default)]
pub enum Voice {
    #[default]
    Idle,
    Recording { rec: Rec, since: Instant },
    Transcribing { rx: Receiver<Result<String, String>> },
}

impl Voice {
    pub fn start(&mut self) -> Result<(), String> {
        if !installed() {
            return Err(format!(
                "Voice needs whisper.cpp: put whisper-cli and {MODEL} in {}",
                whisper_dir().display()
            ));
        }
        let file = std::env::temp_dir().join(format!("pengu-voice-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let f = file.to_string_lossy().to_string();
        let child = Command::new("pw-record")
            .args(["--rate", "16000", "--channels", "1", "--format", "s16", &f])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .or_else(|_| {
                Command::new("arecord")
                    .args(["-q", "-f", "S16_LE", "-r", "16000", "-c", "1", &f])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
            })
            .map_err(|_| "can't record: install pipewire (pw-record) or alsa-utils (arecord)".to_string())?;
        *self = Voice::Recording { rec: Rec { child: Some(child), file }, since: Instant::now() };
        Ok(())
    }

    /// Stop recording and transcribe in the background; `lang` is "auto", "en" or "ar".
    pub fn stop(&mut self, ctx: &eframe::egui::Context, lang: &str) {
        let Voice::Recording { mut rec, .. } = std::mem::take(self) else {
            return;
        };
        let Some(mut child) = rec.child.take() else { return };
        let file = rec.file.clone();
        // SIGINT lets the recorder finish the WAV header; SIGKILL would leave it broken.
        let _ = Command::new("kill").args(["-INT", &child.id().to_string()]).status();
        let _ = child.wait();
        let (tx, rx) = channel();
        let c = ctx.clone();
        let lang = lang.to_string();
        std::thread::spawn(move || {
            let _ = tx.send(transcribe(&file, &lang));
            let _ = std::fs::remove_file(&file);
            c.request_repaint();
        });
        *self = Voice::Transcribing { rx };
    }

    /// The finished transcript, once.
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        let Voice::Transcribing { rx } = self else {
            return None;
        };
        let res = rx.try_recv().ok()?;
        *self = Voice::Idle;
        Some(res)
    }
}

/// Below this, an "Arabic" guess on a clip is not trusted and it is read as English instead.
const SURE_ARABIC: f32 = 0.6;

/// `lang`: "en" or "ar" to fix the language, anything else to let whisper pick between them.
pub fn transcribe(file: &std::path::Path, lang: &str) -> Result<String, String> {
    let (text, heard) = run(file, if lang == "en" || lang == "ar" { lang } else { "auto" })?;
    // Whisper guesses the language from the clip's start, and a wrong guess comes out
    // translated (English speech written in Arabic). Only English and Arabic are spoken here.
    let text = match heard {
        Some((l, p)) if l != "en" && (l != "ar" || p < SURE_ARABIC) => run(file, "en")?.0,
        _ => text,
    };
    if text.is_empty() {
        return Err("didn't catch anything; try again a little closer to the mic".into());
    }
    Ok(text)
}

/// One whisper pass: the text, and (with "auto") the language it heard and how sure it was.
fn run(file: &std::path::Path, lang: &str) -> Result<(String, Option<(String, f32)>), String> {
    let threads = std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(4);
    let out = Command::new(cli())
        .arg("-m")
        .arg(model_path())
        .arg("-f")
        .arg(file)
        // no -np: it would also hide the "auto-detected language" line read below
        .args(["-t", &threads.to_string(), "-l", lang, "-nt"])
        .output()
        .map_err(|e| format!("can't run whisper: {e}"))?;
    if !out.status.success() {
        return Err("whisper couldn't read the recording".into());
    }
    Ok((clean(&String::from_utf8_lossy(&out.stdout)), detected(&String::from_utf8_lossy(&out.stderr))))
}

/// "auto-detected language: en (p = 0.964298)" -> ("en", 0.96)
fn detected(log: &str) -> Option<(String, f32)> {
    let rest = log.split("auto-detected language: ").nth(1)?;
    let (l, rest) = rest.split_once(" (p = ")?;
    let p = rest.split(')').next()?.trim().parse().ok()?;
    Some((l.trim().to_string(), p))
}

/// Joins whisper's lines and drops its markers for silence and noise ([BLANK_AUDIO], (music), …).
pub fn clean(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && !((l.starts_with('[') && l.ends_with(']')) || (l.starts_with('(') && l.ends_with(')')))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PN_WHISPER_DIR=~/.local/share/penguin-notes/whisper PN_VOICE_SAMPLE=some.wav cargo test voice_live -- --ignored
    #[test]
    #[ignore]
    fn voice_live() {
        let wav = PathBuf::from(std::env::var("PN_VOICE_SAMPLE").unwrap());
        assert!(installed());
        let text = transcribe(&wav, "auto").unwrap();
        assert_eq!(transcribe(&wav, "en").unwrap(), text);
        println!("heard: {text}");
        assert!(text.to_lowercase().contains("your country"), "{text}");
    }

    #[test]
    fn reads_the_language_guess() {
        let log = "whisper_init...\nwhisper_full_with_state: auto-detected language: ar (p = 0.41)\n";
        assert_eq!(detected(log), Some(("ar".into(), 0.41)));
        assert_eq!(detected("nothing here"), None);
    }

    #[test]
    fn drops_noise_markers() {
        assert_eq!(clean(" [BLANK_AUDIO]\n"), "");
        assert_eq!(clean(" Hello there.\n (music)\n How are you?\n"), "Hello there. How are you?");
        assert_eq!(clean(" ذكرني بالموعد غداً\n"), "ذكرني بالموعد غداً");
    }
}
