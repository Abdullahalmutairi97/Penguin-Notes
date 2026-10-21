//! OpenAI-compatible client (models list + streaming chat) on worker threads.
use crate::model::AiCfg;
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc::Sender};

pub enum Ev {
    Chunk(String),
    Done,
    Err(String),
    Models(Result<Vec<String>, String>),
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(std::time::Duration::from_secs(8)))
        .build()
        .into()
}

fn base(cfg: &AiCfg) -> String {
    cfg.url.trim_end_matches('/').to_string()
}

pub fn list_models(cfg: AiCfg, tx: Sender<Ev>, ctx: eframe::egui::Context) {
    std::thread::spawn(move || {
        let r = (|| -> Result<Vec<String>, String> {
            let mut req = agent().get(format!("{}/models", base(&cfg)));
            if !cfg.key.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", cfg.key));
            }
            let mut resp = req.call().map_err(|e| e.to_string())?;
            if !resp.status().is_success() {
                let code = resp.status().as_u16();
                return Err(if code == 401 || code == 403 {
                    format!("HTTP {code}, check your API key")
                } else {
                    format!("HTTP {code}")
                });
            }
            let j: serde_json::Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
            let arr = j
                .get("data")
                .or_else(|| j.get("models"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            Ok(arr
                .iter()
                .filter_map(|m| {
                    m.get("id")
                        .or_else(|| m.get("name"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .collect())
        })();
        let _ = tx.send(Ev::Models(r));
        ctx.request_repaint();
    });
}

pub fn chat(
    cfg: AiCfg,
    messages: Vec<(String, String)>,
    tx: Sender<Ev>,
    cancel: Arc<AtomicBool>,
    ctx: eframe::egui::Context,
) {
    std::thread::spawn(move || {
        let send = |e: Ev| {
            let _ = tx.send(e);
            ctx.request_repaint();
        };
        let msgs: Vec<_> = messages
            .iter()
            .map(|(r, c)| serde_json::json!({"role": r, "content": c}))
            .collect();
        let body = serde_json::json!({"model": cfg.model, "stream": true, "messages": msgs});
        let mut req = agent().post(format!("{}/chat/completions", base(&cfg)));
        if !cfg.key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", cfg.key));
        }
        let mut resp = match req.send_json(&body) {
            Ok(r) => r,
            Err(e) => {
                return send(Ev::Err(format!(
                    "Could not reach the model: {e}. Check the endpoint and that the server is running."
                )));
            }
        };
        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            let t = resp.body_mut().read_to_string().unwrap_or_default();
            let hint = match code {
                401 | 403 => " (check your API key in Pengu settings)",
                404 => " (check the endpoint and model name)",
                402 => " (the account may be out of credit)",
                _ => "",
            };
            return send(Ev::Err(format!(
                "HTTP {code}{hint}: {}",
                t.chars().take(160).collect::<String>()
            )));
        }
        let reader = BufReader::new(resp.into_body().into_reader());
        for line in reader.lines() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let Ok(line) = line else { break };
            let Some(d) = line.trim().strip_prefix("data:") else {
                continue;
            };
            let d = d.trim();
            if d == "[DONE]" {
                break;
            }
            if let Ok(j) = serde_json::from_str::<serde_json::Value>(d) {
                let c = &j["choices"][0];
                let t = c["delta"]["content"]
                    .as_str()
                    .or_else(|| c["message"]["content"].as_str())
                    .unwrap_or("");
                if !t.is_empty() {
                    send(Ev::Chunk(t.to_string()));
                }
            }
        }
        send(Ev::Done);
    });
}

#[cfg(test)]
mod tests {
    /// Network check that HTTPS works against DeepSeek; run with `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn deepseek_reachable_over_tls() {
        let (tx, rx) = std::sync::mpsc::channel();
        let cfg = crate::model::AiCfg {
            preset: "DeepSeek".into(),
            url: "https://api.deepseek.com".into(),
            model: String::new(),
            key: String::new(),
        };
        super::list_models(cfg, tx, eframe::egui::Context::default());
        match rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap() {
            super::Ev::Models(r) => assert!(matches!(&r, Err(e) if e.contains("401")), "{r:?}"),
            _ => panic!(),
        }
    }
}
