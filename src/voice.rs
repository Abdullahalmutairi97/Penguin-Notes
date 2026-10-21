//! Talking to Pengu: record the mic (PipeWire's pw-record or arecord on Linux, the system
//! audio API on Windows), then turn the recording into text with whisper.cpp on this machine.
//! Nothing leaves the computer; only the speech model is downloaded, once.
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Command;
#[cfg(not(windows))]
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use crate::model;

pub const MODEL: &str = "ggml-small-q5_1.bin";
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin";
pub const MODEL_SIZE: u64 = 190_085_487;
const CLI: &str = if cfg!(windows) { "whisper-cli.exe" } else { "whisper-cli" };

/// The model lives next to the notes: <data>/whisper/.
pub fn whisper_dir() -> PathBuf {
    // PN_WHISPER_DIR: lets the tests use a real install (data_dir is a temp dir there).
    std::env::var_os("PN_WHISPER_DIR").map(PathBuf::from).unwrap_or_else(|| model::data_dir().join("whisper"))
}

/// whisper-cli ships with the app in a whisper/ folder beside it; an older install keeps
/// it in the data dir instead.
fn cli() -> PathBuf {
    let bundled = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("whisper").join(CLI)))
        .filter(|p| p.is_file() && std::env::var_os("PN_WHISPER_DIR").is_none() && !cfg!(test));
    bundled.unwrap_or_else(|| whisper_dir().join(CLI))
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

#[cfg(test)]
fn installed() -> bool {
    cli().is_file() && model_path().is_file()
}

/// Fetches the speech model into the whisper dir, counting bytes into `got` as they arrive.
pub fn download_model(got: &AtomicU64) -> Result<(), String> {
    let dir = whisper_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let part = dir.join(format!("{MODEL}.part"));
    let fail = |e: String| format!("couldn't download the voice model ({e}); check the internet and try again");
    let res = ureq::get(MODEL_URL).call().map_err(|e| fail(e.to_string()))?;
    let mut body = res.into_body().into_reader();
    let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 1 << 16];
    let mut n = 0u64;
    loop {
        let k = body.read(&mut buf).map_err(|e| fail(e.to_string()))?;
        if k == 0 {
            break;
        }
        out.write_all(&buf[..k]).map_err(|e| e.to_string())?;
        n += k as u64;
        got.store(n, Ordering::Relaxed);
    }
    drop(out);
    if n != MODEL_SIZE {
        let _ = std::fs::remove_file(&part);
        return Err(fail("the download was cut short".into()));
    }
    std::fs::rename(&part, model_path()).map_err(|e| e.to_string())
}

/// A running recorder; dropping it (app closed mid-recording) stops it and removes the file.
pub struct Rec {
    #[cfg(not(windows))]
    child: Option<Child>,
    #[cfg(windows)]
    mic: Option<mic::Mic>,
    file: PathBuf,
}

impl Drop for Rec {
    fn drop(&mut self) {
        #[cfg(not(windows))]
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
            let _ = std::fs::remove_file(&self.file);
        }
        #[cfg(windows)]
        if self.mic.take().is_some() {
            let _ = std::fs::remove_file(&self.file);
        }
    }
}

#[derive(Default)]
pub enum Voice {
    #[default]
    Idle,
    /// Fetching the speech model the first time Talk is used.
    Downloading { got: Arc<AtomicU64>, rx: Receiver<Result<(), String>> },
    Recording { rec: Rec, since: Instant },
    Transcribing { rx: Receiver<Result<String, String>> },
}

impl Voice {
    pub fn start(&mut self, ctx: &eframe::egui::Context) -> Result<(), String> {
        if !cli().is_file() {
            return Err(format!("Voice needs whisper.cpp: put {CLI} in {}", whisper_dir().display()));
        }
        if !model_path().is_file() {
            let got = Arc::new(AtomicU64::new(0));
            let (tx, rx) = channel();
            let (g, c) = (got.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(download_model(&g));
                c.request_repaint();
            });
            *self = Voice::Downloading { got, rx };
            return Ok(());
        }
        let file = std::env::temp_dir().join(format!("pengu-voice-{}.wav", std::process::id()));
        let _ = std::fs::remove_file(&file);
        #[cfg(not(windows))]
        let rec = {
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
            Rec { child: Some(child), file }
        };
        #[cfg(windows)]
        let rec = Rec { mic: Some(mic::Mic::start()?), file };
        *self = Voice::Recording { rec, since: Instant::now() };
        Ok(())
    }

    /// Stop recording and transcribe in the background; `lang` is "auto", "en" or "ar".
    pub fn stop(&mut self, ctx: &eframe::egui::Context, lang: &str) {
        let Voice::Recording { mut rec, .. } = std::mem::take(self) else {
            return;
        };
        let file = rec.file.clone();
        #[cfg(not(windows))]
        {
            let Some(mut child) = rec.child.take() else { return };
            // SIGINT lets the recorder finish the WAV header; SIGKILL would leave it broken.
            let _ = Command::new("kill").args(["-INT", &child.id().to_string()]).status();
            let _ = child.wait();
        }
        #[cfg(windows)]
        {
            let Some(m) = rec.mic.take() else { return };
            if let Err(e) = m.finish(&file) {
                let (tx, rx) = channel();
                let _ = tx.send(Err(e));
                *self = Voice::Transcribing { rx };
                return;
            }
        }
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

    /// The finished transcript, once; a failed model download comes back as its error.
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        match self {
            Voice::Transcribing { rx } => {
                let res = rx.try_recv().ok()?;
                *self = Voice::Idle;
                Some(res)
            }
            Voice::Downloading { rx, .. } => {
                let res = rx.try_recv().ok()?;
                *self = Voice::Idle;
                res.err().map(Err)
            }
            _ => None,
        }
    }
}

/// Windows has no recorder program to call, so the mic is read with cpal and written out
/// as a WAV (at the device's own rate; whisper-cli converts it).
#[cfg(windows)]
mod mic {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    pub struct Mic {
        _stream: cpal::Stream,
        buf: Arc<Mutex<Vec<f32>>>,
        rate: u32,
    }

    impl Mic {
        pub fn start() -> Result<Self, String> {
            let dev = cpal::default_host().default_input_device().ok_or("no microphone found")?;
            let cfg = dev.default_input_config().map_err(|e| format!("can't use the microphone: {e}"))?;
            let (rate, ch) = (cfg.sample_rate().0, cfg.channels() as usize);
            let buf = Arc::new(Mutex::new(Vec::new()));
            let b = buf.clone();
            let err = |e| eprintln!("mic: {e}");
            // Keep one channel; whisper only needs mono.
            let stream = match cfg.sample_format() {
                cpal::SampleFormat::F32 => dev.build_input_stream(
                    &cfg.into(),
                    move |d: &[f32], _: &_| b.lock().unwrap().extend(d.iter().step_by(ch)),
                    err,
                    None,
                ),
                cpal::SampleFormat::I16 => dev.build_input_stream(
                    &cfg.into(),
                    move |d: &[i16], _: &_| b.lock().unwrap().extend(d.iter().step_by(ch).map(|&x| x as f32 / 32768.0)),
                    err,
                    None,
                ),
                f => return Err(format!("unsupported microphone format {f:?}")),
            }
            .map_err(|e| format!("can't use the microphone: {e}"))?;
            stream.play().map_err(|e| format!("can't use the microphone: {e}"))?;
            Ok(Mic { _stream: stream, buf, rate })
        }

        pub fn finish(self, file: &Path) -> Result<(), String> {
            let Mic { _stream, buf, rate } = self;
            drop(_stream);
            let pcm: Vec<u8> = buf
                .lock()
                .unwrap()
                .iter()
                .flat_map(|&x| ((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
                .collect();
            std::fs::write(file, super::wav(rate, &pcm)).map_err(|e| e.to_string())
        }
    }
}

/// A mono 16-bit PCM WAV file around `pcm`.
#[cfg(any(windows, test))]
fn wav(rate: u32, pcm: &[u8]) -> Vec<u8> {
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend(b"RIFF");
    w.extend((36 + pcm.len() as u32).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes()); // PCM
    w.extend(1u16.to_le_bytes()); // mono
    w.extend(rate.to_le_bytes());
    w.extend((rate * 2).to_le_bytes());
    w.extend(2u16.to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend(b"data");
    w.extend((pcm.len() as u32).to_le_bytes());
    w.extend(pcm);
    w
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
    let mut cmd = Command::new(cli());
    // From a windowed app, Windows would pop up a console for whisper-cli.
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    let out = cmd
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

    /// PN_WHISPER_DIR=/some/empty/dir cargo test model_download_live -- --ignored
    #[test]
    #[ignore]
    fn model_download_live() {
        let got = AtomicU64::new(0);
        download_model(&got).unwrap();
        assert_eq!(got.load(Ordering::Relaxed), MODEL_SIZE);
        assert_eq!(std::fs::metadata(model_path()).unwrap().len(), MODEL_SIZE);
    }

    #[test]
    fn wav_header() {
        let w = wav(48000, &[1, 0, 2, 0]);
        assert_eq!(w.len(), 48);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 40);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 48000);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 4);
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
