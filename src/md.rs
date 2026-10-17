//! Markdown subset parser (matches the Penguin Notes design) and egui renderer.
use eframe::egui::{self, Align2, Color32, FontId, RichText, Sense, Stroke, Vec2};

use crate::theme::{self, Palette};

#[derive(Clone, Debug, PartialEq)]
pub enum Span {
    Text(String),
    Bold(String),
    Italic(String),
    Strike(String),
    Mark(String),
    Code(String),
    Link { label: String, url: String },
    Wiki(String),
}

impl Span {
    fn text(&self) -> &str {
        match self {
            Span::Text(s)
            | Span::Bold(s)
            | Span::Italic(s)
            | Span::Strike(s)
            | Span::Mark(s)
            | Span::Code(s)
            | Span::Wiki(s) => s,
            Span::Link { label, .. } => label,
        }
    }

    /// Same span with its visible text swapped; wiki and link targets are kept.
    fn with_text(&self, t: &str) -> Span {
        let t = t.to_string();
        match self {
            Span::Text(_) => Span::Text(t),
            Span::Bold(_) => Span::Bold(t),
            Span::Italic(_) => Span::Italic(t),
            Span::Strike(_) => Span::Strike(t),
            Span::Mark(_) => Span::Mark(t),
            Span::Code(_) => Span::Code(t),
            Span::Link { url, .. } => Span::Link {
                label: t,
                url: url.clone(),
            },
            Span::Wiki(_) => Span::Wiki(t),
        }
    }
}

fn spans_rtl(spans: &[Span]) -> bool {
    theme::is_rtl(&spans.iter().map(Span::text).collect::<String>())
}

/// A row that starts on the right for Arabic text.
fn row<R>(ui: &mut egui::Ui, rtl: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if rtl {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), add)
            .inner
    } else {
        ui.horizontal_top(add).inner
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    pub kind: String,
    pub title: String,
    pub rows: Vec<(String, f64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Span>),
    Para(Vec<Span>),
    Bullet(Vec<Span>),
    Ordered(String, Vec<Span>),
    Check {
        line: usize,
        done: bool,
        text: Vec<Span>,
    },
    Quote(Vec<Span>),
    Hr,
    Code(String),
    Table {
        head: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
    Chart(Chart),
}

fn delim<'a>(r: &'a str, open: &str, close: &str) -> Option<(&'a str, usize)> {
    let after = r.strip_prefix(open)?;
    let p = after.find(close)?;
    (p > 0).then(|| (&after[..p], open.len() + p + close.len()))
}

pub fn parse_inline(s: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    while i < s.len() {
        let r = &s[i..];
        let hit: Option<(Span, usize)> = if let Some((c, n)) = delim(r, "`", "`") {
            Some((Span::Code(c.into()), n))
        } else if let Some((c, n)) = delim(r, "[[", "]]") {
            (!c.contains(']')).then(|| (Span::Wiki(c.into()), n))
        } else if r.starts_with('[') {
            r.find("](").and_then(|a| {
                let b = r[a + 2..].find(')')?;
                let url = &r[a + 2..a + 2 + b];
                let label = &r[1..a];
                (url.starts_with("http")
                    && !url.contains(char::is_whitespace)
                    && !label.contains(']')
                    && !label.is_empty())
                .then(|| {
                    (
                        Span::Link {
                            label: label.into(),
                            url: url.into(),
                        },
                        a + 2 + b + 1,
                    )
                })
            })
        } else if let Some((c, n)) = delim(r, "**", "**") {
            Some((Span::Bold(c.into()), n))
        } else if let Some((c, n)) = delim(r, "~~", "~~") {
            Some((Span::Strike(c.into()), n))
        } else if let Some((c, n)) = delim(r, "==", "==") {
            Some((Span::Mark(c.into()), n))
        } else if let Some((c, n)) = delim(r, "*", "*") {
            Some((Span::Italic(c.into()), n))
        } else {
            None
        };
        if let Some((sp, n)) = hit {
            if !buf.is_empty() {
                out.push(Span::Text(std::mem::take(&mut buf)));
            }
            out.push(sp);
            i += n;
        } else {
            let ch = r.chars().next().unwrap();
            buf.push(ch);
            i += ch.len_utf8();
        }
    }
    if !buf.is_empty() {
        out.push(Span::Text(buf));
    }
    out
}

fn parse_chart(kind: &str, lines: &[&str]) -> Chart {
    let mut title = String::new();
    let mut rows = Vec::new();
    for l in lines {
        let Some(i) = l.rfind(':') else { continue };
        let (k, v) = (l[..i].trim(), l[i + 1..].trim());
        if k.eq_ignore_ascii_case("title") {
            title = v.to_string();
        } else if let Ok(n) = v.parse::<f64>() {
            rows.push((k.to_string(), n));
        }
    }
    Chart {
        kind: if kind.is_empty() {
            "bar".into()
        } else {
            kind.into()
        },
        title,
        rows,
    }
}

fn cells(r: &str) -> Vec<Vec<Span>> {
    let r = r.trim();
    let r = r.strip_prefix('|').unwrap_or(r);
    let r = r.strip_suffix('|').unwrap_or(r);
    r.split('|').map(|c| parse_inline(c.trim())).collect()
}

fn is_table_sep(l: &str) -> bool {
    let t = l.trim_start_matches('|').trim_start();
    let t = t.strip_prefix(':').unwrap_or(t);
    l.contains('|') && t.starts_with('-')
}

pub fn parse(src: &str) -> Vec<Block> {
    let lines: Vec<&str> = src.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if let Some(rest) = l.strip_prefix("```") {
            let mut w = rest.split_whitespace();
            let (lang, sub) = (w.next().unwrap_or(""), w.next().unwrap_or(""));
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].starts_with("```") {
                body.push(lines[i]);
                i += 1;
            }
            i += 1;
            out.push(if lang == "chart" {
                Block::Chart(parse_chart(sub, &body))
            } else {
                Block::Code(body.join("\n"))
            });
            continue;
        }
        if l.starts_with('|') && i + 1 < lines.len() && is_table_sep(lines[i + 1]) {
            let head = cells(l);
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].starts_with('|') {
                rows.push(cells(lines[i]));
                i += 1;
            }
            out.push(Block::Table { head, rows });
            continue;
        }
        let bullet = l.strip_prefix("- ").or_else(|| l.strip_prefix("* "));
        if let Some(b) = bullet {
            let b_lower = b.to_lowercase();
            if b_lower.starts_with("[ ] ") || b_lower.starts_with("[x] ") {
                out.push(Block::Check {
                    line: i,
                    done: !b.starts_with("[ ]"),
                    text: parse_inline(&b[4..]),
                });
            } else {
                out.push(Block::Bullet(parse_inline(b)));
            }
            i += 1;
            continue;
        }
        let digits = l.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && l[digits..].starts_with(". ") {
            out.push(Block::Ordered(
                l[..digits].to_string(),
                parse_inline(&l[digits + 2..]),
            ));
            i += 1;
            continue;
        }
        let hashes = l.bytes().take_while(|&b| b == b'#').count();
        if (1..=3).contains(&hashes) && l[hashes..].starts_with(' ') {
            out.push(Block::Heading(hashes as u8, parse_inline(&l[hashes + 1..])));
        } else if let Some(q) = l.strip_prefix("> ") {
            out.push(Block::Quote(parse_inline(q)));
        } else if l.len() >= 3 && l.bytes().all(|b| b == b'-') {
            out.push(Block::Hr);
        } else if !l.trim().is_empty() {
            out.push(Block::Para(parse_inline(l)));
        }
        i += 1;
    }
    out
}

/// Checklist items: (line index, char index of the `[`, done).
pub fn checklist(body: &str) -> Vec<(usize, usize, bool)> {
    let mut out = Vec::new();
    let mut at = 0;
    for (n, line) in body.split('\n').enumerate() {
        let b = line.strip_prefix("- ").or_else(|| line.strip_prefix("* "));
        if let Some(rest) = b {
            let low = rest.get(..4).map(str::to_lowercase);
            if low.as_deref() == Some("[ ] ") || low.as_deref() == Some("[x] ") {
                out.push((n, at + 2, low.as_deref() == Some("[x] ")));
            }
        }
        at += line.chars().count() + 1;
    }
    out
}

pub fn toggle_line(body: &str, line: usize) -> String {
    let mut ls: Vec<String> = body.split('\n').map(String::from).collect();
    if let Some(l) = ls.get_mut(line) {
        *l = if l.contains("[ ]") {
            l.replacen("[ ]", "[x]", 1)
        } else {
            l.replacen("[x]", "[ ]", 1).replacen("[X]", "[ ]", 1)
        };
    }
    ls.join("\n")
}

// ---------------------------------------------------------------- rendering

pub enum Action {
    Toggle(usize),
    Wiki(String),
    Url(String),
}

fn span_text(
    sp: &Span,
    size: f32,
    base: Color32,
    pal: &Palette,
) -> (RichText, Option<Action>, bool) {
    let t = |s: &str| RichText::new(s.to_string()).size(size).color(base);
    match sp {
        Span::Text(s) => (t(s), None, false),
        Span::Bold(s) => (
            RichText::new(s.clone())
                .size(size)
                .color(base)
                .family(egui::FontFamily::Name("bold".into())),
            None,
            false,
        ),
        Span::Italic(s) => (t(s).italics(), None, false),
        Span::Strike(s) => (t(s).strikethrough(), None, false),
        Span::Mark(s) => (
            t(s).background_color(pal.accent2.gamma_multiply(0.75))
                .color(Color32::from_rgb(26, 22, 20)),
            None,
            false,
        ),
        Span::Code(s) => (
            t(s).monospace()
                .size(size * 0.92)
                .background_color(pal.paper2),
            None,
            false,
        ),
        Span::Link { label, url } => (
            t(label).color(pal.accent),
            Some(Action::Url(url.clone())),
            true,
        ),
        Span::Wiki(s) => (t(s).color(pal.accent), Some(Action::Wiki(s.clone())), true),
    }
}

fn inline(
    ui: &mut egui::Ui,
    spans: &[Span],
    size: f32,
    color: Color32,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    let body = |ui: &mut egui::Ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        inline_row(ui, spans, size, color, pal, acts);
    };
    if spans_rtl(spans) {
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Min).with_main_wrap(true),
            body,
        );
    } else {
        ui.horizontal_wrapped(body);
    }
}

fn inline_row(
    ui: &mut egui::Ui,
    spans: &[Span],
    size: f32,
    color: Color32,
    pal: &Palette,
    acts: &mut Vec<Action>,
) {
    let pieces = spans.iter().flat_map(|sp| {
        theme::arabic_runs(sp.text())
            .into_iter()
            .map(move |(t, ar)| (sp.with_text(t), ar, matches!(sp, Span::Bold(_))))
    });
    for (sp, ar, bold) in pieces {
        let (mut rt, act, clickable) = span_text(&sp, size, color, pal);
        if ar {
            rt = rt.family(theme::arabic(bold));
        }
        if clickable {
            if ui
                .add(egui::Link::new(rt))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                acts.extend(act);
            }
        } else {
            ui.label(rt);
        }
    }
}

fn boldify(spans: &[Span]) -> Vec<Span> {
    spans
        .iter()
        .map(|s| match s {
            Span::Text(t) => Span::Bold(t.clone()),
            o => o.clone(),
        })
        .collect()
}

pub fn render(ui: &mut egui::Ui, blocks: &[Block], pal: &Palette) -> Vec<Action> {
    let mut acts = Vec::new();
    let base = 16.0;
    for b in blocks {
        match b {
            Block::Heading(n, sp) => {
                ui.add_space(8.0);
                let size = [26.0, 21.0, 17.0][(*n - 1) as usize];
                inline(ui, &boldify(sp), size, pal.ink, pal, &mut acts);
                ui.add_space(4.0);
            }
            Block::Para(sp) => {
                inline(ui, sp, base, pal.ink, pal, &mut acts);
                ui.add_space(6.0);
            }
            Block::Bullet(sp) => {
                row(ui, spans_rtl(sp), |ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new("•").size(base).color(pal.ink));
                    ui.vertical(|ui| inline(ui, sp, base, pal.ink, pal, &mut acts));
                });
            }
            Block::Ordered(n, sp) => {
                row(ui, spans_rtl(sp), |ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("{n}.")).size(base).color(pal.ink));
                    ui.vertical(|ui| inline(ui, sp, base, pal.ink, pal, &mut acts));
                });
            }
            Block::Check { line, done, text } => {
                row(ui, spans_rtl(text), |ui| {
                    let mut d = *done;
                    if ui.checkbox(&mut d, "").clicked() {
                        acts.push(Action::Toggle(*line));
                    }
                    let col = if *done { pal.ink_3 } else { pal.ink };
                    let sp: Vec<Span> = if *done {
                        text.iter()
                            .map(|s| match s {
                                Span::Text(t) => Span::Strike(t.clone()),
                                o => o.clone(),
                            })
                            .collect()
                    } else {
                        text.clone()
                    };
                    ui.vertical(|ui| inline(ui, &sp, base, col, pal, &mut acts));
                });
            }
            Block::Quote(sp) => {
                let r = egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 14,
                        right: 0,
                        top: 2,
                        bottom: 2,
                    })
                    .show(ui, |ui| {
                        inline(ui, sp, base, pal.ink_2, pal, &mut acts);
                    });
                let rc = r.response.rect;
                ui.painter().line_segment(
                    [rc.left_top(), rc.left_bottom()],
                    Stroke::new(3.0, pal.ink_3),
                );
                ui.add_space(6.0);
            }
            Block::Hr => {
                ui.add_space(4.0);
                let (rc, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), Sense::hover());
                ui.painter().add(egui::Shape::dashed_line(
                    &[rc.left_center(), rc.right_center()],
                    Stroke::new(1.3, pal.line),
                    6.0,
                    4.0,
                ));
                ui.add_space(4.0);
            }
            Block::Code(s) => {
                egui::Frame::new()
                    .fill(pal.paper2)
                    .stroke(Stroke::new(1.4, pal.ink_3))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(s.clone())
                                .monospace()
                                .size(13.5)
                                .color(pal.ink),
                        );
                    });
                ui.add_space(8.0);
            }
            Block::Table { head, rows } => {
                ui.add_space(2.0);
                egui::Frame::new()
                    .stroke(Stroke::new(1.3, pal.ink_3))
                    .corner_radius(6)
                    .show(ui, |ui| {
                        egui::Grid::new(ui.next_auto_id())
                            .spacing([0.0, 0.0])
                            .show(ui, |ui| {
                                let cell =
                                    |ui: &mut egui::Ui, sp: &[Span], acts: &mut Vec<Action>| {
                                        egui::Frame::new()
                                            .inner_margin(egui::Margin::symmetric(12, 5))
                                            .show(ui, |ui| {
                                                ui.style_mut().wrap_mode =
                                                    Some(egui::TextWrapMode::Extend);
                                                ui.horizontal(|ui| {
                                                    ui.spacing_mut().item_spacing.x = 0.0;
                                                    inline_row(ui, sp, 15.0, pal.ink, pal, acts);
                                                });
                                            });
                                    };
                                for c in head {
                                    cell(ui, &boldify(c), &mut acts);
                                }
                                ui.end_row();
                                for r in rows {
                                    for c in r {
                                        cell(ui, c, &mut acts);
                                    }
                                    ui.end_row();
                                }
                            });
                    });
                ui.add_space(10.0);
            }
            Block::Chart(c) => {
                chart(ui, c, pal);
                ui.add_space(8.0);
            }
        }
    }
    acts
}

const CHART_PAL: [[u8; 3]; 6] = [
    [217, 77, 42],
    [240, 193, 43],
    [47, 122, 107],
    [59, 91, 169],
    [138, 90, 168],
    [122, 154, 42],
];

fn num(n: f64) -> String {
    let r = (n * 100.0).round() / 100.0;
    format!("{r}")
}

fn chart(ui: &mut egui::Ui, c: &Chart, pal: &Palette) {
    if c.rows.is_empty() {
        ui.label(
            RichText::new("Chart needs lines like `Label: 12`")
                .size(13.0)
                .color(pal.ink_3),
        );
        return;
    }
    let (w, h) = (520.0_f32, 230.0_f32);
    let width = ui.available_width().min(560.0);
    let k = width / w;
    let (rc, _) = ui.allocate_exact_size(Vec2::new(width, h * k), Sense::hover());
    let p = ui.painter_at(rc);
    p.rect(
        rc,
        8,
        pal.paper2,
        Stroke::new(1.4, pal.ink_3),
        egui::StrokeKind::Inside,
    );
    let pt = |x: f32, y: f32| rc.min + Vec2::new(x * k, y * k);
    let font = |s: f32| FontId::proportional(s * k);
    let ink = Stroke::new(1.4, pal.ink);
    let color = |i: usize| {
        let c = CHART_PAL[i % CHART_PAL.len()];
        Color32::from_rgb(c[0], c[1], c[2])
    };
    let (lm, r, b) = (38.0_f32, 14.0_f32, 34.0_f32);
    let t = if c.title.is_empty() { 16.0 } else { 36.0 };
    if !c.title.is_empty() {
        p.text(
            pt(w / 2.0, 20.0),
            Align2::CENTER_CENTER,
            &c.title,
            FontId::new(14.0 * k, egui::FontFamily::Name("bold".into())),
            pal.ink,
        );
    }
    if c.kind == "pie" {
        let tot: f64 = c.rows.iter().map(|r| r.1.max(0.0)).sum::<f64>().max(1e-9);
        let (cx, cy, rad) = (120.0_f32, t + (h - t) / 2.0 - 4.0, 80.0_f32);
        let mut a0 = -std::f32::consts::FRAC_PI_2;
        for (i, (label, v)) in c.rows.iter().enumerate() {
            let fr = (v.max(0.0) / tot) as f32;
            let a1 = a0 + fr * std::f32::consts::TAU;
            if fr > 0.0 {
                let steps = ((fr * 64.0).ceil() as usize).max(2);
                let mut pts = vec![pt(cx, cy)];
                for s in 0..=steps {
                    let a = a0 + (a1 - a0) * s as f32 / steps as f32;
                    pts.push(pt(cx + rad * a.cos(), cy + rad * a.sin()));
                }
                let mut mesh = egui::Mesh::default();
                for q in &pts {
                    mesh.colored_vertex(*q, color(i));
                }
                for s in 1..pts.len() as u32 - 1 {
                    mesh.add_triangle(0, s, s + 1);
                }
                p.add(mesh);
                if fr < 0.999 {
                    pts.push(pts[0]);
                }
                p.add(egui::Shape::line(pts, ink));
            }
            let y = t + 6.0 + i as f32 * 22.0;
            p.rect(
                egui::Rect::from_min_size(pt(250.0, y), Vec2::splat(12.0 * k)),
                0,
                color(i),
                Stroke::new(1.0, pal.ink),
                egui::StrokeKind::Inside,
            );
            p.text(
                pt(270.0, y + 6.0),
                Align2::LEFT_CENTER,
                format!("{label} ({}%)", (fr * 100.0).round()),
                font(12.0),
                pal.ink,
            );
            a0 = a1;
        }
        return;
    }
    let (pw, ph) = (w - lm - r, h - t - b);
    let max = c.rows.iter().map(|r| r.1).fold(1.0_f64, f64::max);
    for i in 0..=4 {
        let y = t + ph - ph * i as f32 / 4.0;
        p.add(egui::Shape::dashed_line(
            &[pt(lm, y), pt(w - r, y)],
            Stroke::new(1.0, pal.ink_3.gamma_multiply(0.5)),
            3.0 * k,
            4.0 * k,
        ));
        p.text(
            pt(lm - 6.0, y),
            Align2::RIGHT_CENTER,
            num(max * i as f64 / 4.0),
            font(10.0),
            pal.ink_3,
        );
    }
    let bw = pw / c.rows.len() as f32;
    let pts: Vec<egui::Pos2> = c
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            pt(
                lm + bw * (i as f32 + 0.5),
                t + ph - ph * (r.1.max(0.0) / max) as f32,
            )
        })
        .collect();
    if c.kind == "line" {
        p.add(egui::Shape::line(
            pts.clone(),
            Stroke::new(2.4 * k, pal.accent),
        ));
    }
    for (i, (label, v)) in c.rows.iter().enumerate() {
        let x = lm + bw * i as f32;
        let top = pts[i];
        if c.kind == "line" {
            p.circle(top, 4.5 * k, pal.accent2, ink);
        } else {
            let rect = egui::Rect::from_min_max(pt(x + bw * 0.15, 0.0), pt(x + bw * 0.85, t + ph));
            let rect = egui::Rect::from_min_max(egui::pos2(rect.min.x, top.y), rect.max);
            p.rect(rect, 2, color(i), ink, egui::StrokeKind::Inside);
        }
        p.text(
            egui::pos2(pt(x + bw / 2.0, 0.0).x, top.y - 8.0 * k),
            Align2::CENTER_CENTER,
            num(*v),
            font(11.0),
            pal.ink,
        );
        p.text(
            pt(x + bw / 2.0, h - 12.0),
            Align2::CENTER_CENTER,
            label,
            font(11.0),
            pal.ink,
        );
    }
    p.line_segment([pt(lm, t + ph), pt(w - r, t + ph)], ink);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_spans() {
        let s = parse_inline("a **b** [[W]] `c` [x](https://e.com) ==m== ~~s~~ *i*");
        assert!(s.contains(&Span::Bold("b".into())));
        assert!(s.contains(&Span::Wiki("W".into())));
        assert!(s.contains(&Span::Code("c".into())));
        assert!(s.contains(&Span::Link {
            label: "x".into(),
            url: "https://e.com".into()
        }));
        assert!(s.contains(&Span::Mark("m".into())));
        assert!(s.contains(&Span::Strike("s".into())));
        assert!(s.contains(&Span::Italic("i".into())));
    }

    #[test]
    fn blocks() {
        let b = parse(
            "# T\n- [x] done\n- item\n1. one\n| a | b |\n| --- | --- |\n| 1 | 2 |\n```chart pie\ntitle: X\nA: 1\n```",
        );
        assert!(matches!(b[0], Block::Heading(1, _)));
        assert!(matches!(
            b[1],
            Block::Check {
                line: 1,
                done: true,
                ..
            }
        ));
        assert!(matches!(b[2], Block::Bullet(_)));
        assert!(matches!(b[3], Block::Ordered(..)));
        assert!(matches!(b[4], Block::Table { .. }));
        match &b[5] {
            Block::Chart(c) => assert_eq!(
                (c.kind.as_str(), c.title.as_str(), c.rows.len()),
                ("pie", "X", 1)
            ),
            _ => panic!(),
        }
    }
}
