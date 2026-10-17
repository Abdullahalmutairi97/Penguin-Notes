//! Live markdown styling for the editor. Every input char is emitted exactly once,
//! so cursor positions are unaffected; markup markers are just dimmed.
use eframe::egui::{Color32, FontFamily, FontId, Stroke, TextFormat, text::LayoutJob};

use crate::theme::{self, Palette};

const BASE: f32 = 14.0;

#[derive(Clone, Copy, Default)]
struct St {
    bold: bool,
    italic: bool,
    strike: bool,
    mark: bool,
    code: bool,
    link: bool,
    dim: bool,
    /// Markup marker on a line without the cursor: laid out but invisible.
    hide: bool,
    /// Takes up space but is painted over (checkbox widgets).
    ghost: bool,
    size: Option<f32>,
}

fn format(st: St, p: &Palette) -> TextFormat {
    if st.hide {
        return TextFormat {
            font_id: FontId::monospace(0.5),
            color: Color32::TRANSPARENT,
            ..Default::default()
        };
    }
    let family = match (st.bold, st.italic) {
        (false, false) => FontFamily::Monospace,
        (true, false) => FontFamily::Name("mono-bold".into()),
        (false, true) => FontFamily::Name("mono-italic".into()),
        (true, true) => FontFamily::Name("mono-bold-italic".into()),
    };
    let mut color = if st.dim {
        p.ink_3
    } else if st.link {
        p.accent
    } else {
        p.ink
    };
    let mut background = Color32::TRANSPARENT;
    if st.mark && !st.dim {
        background = p.accent2.gamma_multiply(0.75);
        color = Color32::from_rgb(26, 22, 20);
    }
    if st.code && !st.dim {
        background = p.line.gamma_multiply(0.6);
    }
    if st.ghost {
        color = Color32::TRANSPARENT;
    }
    TextFormat {
        font_id: FontId::new(st.size.unwrap_or(BASE), family),
        color,
        background,
        strikethrough: if st.strike {
            Stroke::new(1.3, color)
        } else {
            Stroke::NONE
        },
        underline: if st.link && !st.dim {
            Stroke::new(1.0, p.accent.gamma_multiply(0.6))
        } else {
            Stroke::NONE
        },
        line_height: Some(st.size.unwrap_or(BASE) * 1.6),
        ..Default::default()
    }
}

fn push(job: &mut LayoutJob, s: &str, st: St, p: &Palette) {
    for (piece, ar) in theme::arabic_runs(s) {
        let mut f = format(st, p);
        if ar && !st.hide {
            f.font_id.family = theme::arabic(st.bold);
        }
        job.append(piece, 0.0, f);
    }
}

fn delim<'a>(r: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let after = r.strip_prefix(open)?;
    let n = after.find(close)?;
    (n > 0 && !after[..n].contains('\n')).then(|| &after[..n])
}

fn inline(job: &mut LayoutJob, s: &str, st: St, p: &Palette, hide: bool) {
    let dim = St {
        dim: true,
        hide,
        ..st
    };
    let mut plain = String::new();
    let mut i = 0;
    let flush = |job: &mut LayoutJob, plain: &mut String| {
        push(job, plain, st, p);
        plain.clear();
    };
    while i < s.len() {
        let r = &s[i..];
        if let Some(c) = delim(r, "`", "`") {
            flush(job, &mut plain);
            push(job, "`", dim, p);
            push(job, c, St { code: true, ..st }, p);
            push(job, "`", dim, p);
            i += c.len() + 2;
            continue;
        }
        if let Some(c) = delim(r, "[[", "]]").filter(|c| !c.contains(']')) {
            flush(job, &mut plain);
            push(job, "[[", dim, p);
            push(job, c, St { link: true, ..st }, p);
            push(job, "]]", dim, p);
            i += c.len() + 4;
            continue;
        }
        if r.starts_with('[') {
            if let Some((label, url)) = r.find("](").and_then(|a| {
                let b = r[a + 2..].find(')')?;
                let (label, url) = (&r[1..a], &r[a + 2..a + 2 + b]);
                (!label.is_empty()
                    && !label.contains(']')
                    && !label.contains('\n')
                    && !url.contains(char::is_whitespace))
                .then_some((label, url))
            }) {
                flush(job, &mut plain);
                push(job, "[", dim, p);
                push(job, label, St { link: true, ..st }, p);
                push(job, "](", dim, p);
                push(job, url, dim, p);
                push(job, ")", dim, p);
                i += label.len() + url.len() + 4;
                continue;
            }
        }
        let wrapped: [(&str, fn(St) -> St); 4] = [
            ("**", |s| St { bold: true, ..s }),
            ("~~", |s| St { strike: true, ..s }),
            ("==", |s| St { mark: true, ..s }),
            ("*", |s| St { italic: true, ..s }),
        ];
        if let Some((m, c, f)) = wrapped
            .iter()
            .find_map(|(m, f)| delim(r, m, m).map(|c| (*m, c, f)))
        {
            flush(job, &mut plain);
            push(job, m, dim, p);
            inline(job, c, f(st), p, hide);
            push(job, m, dim, p);
            i += c.len() + 2 * m.len();
            continue;
        }
        let ch = r.chars().next().unwrap();
        plain.push(ch);
        i += ch.len_utf8();
    }
    flush(job, &mut plain);
}

/// `reveal`: char range of the selection; markers on those lines stay visible.
pub fn highlight(text: &str, p: &Palette, reveal: Option<(usize, usize)>) -> LayoutJob {
    let mut job = LayoutJob::default();
    let base = St::default();
    let dim = St { dim: true, ..base };
    let accent = |job: &mut LayoutJob, s: &str| {
        let mut f = format(base, p);
        f.color = p.accent;
        job.append(s, 0.0, f);
    };
    let mut fence = false;
    let mut at = 0;
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            push(&mut job, "\n", base, p);
        }
        let len = line.chars().count();
        let hide = !reveal.is_some_and(|(s, e)| s <= at + len && e >= at);
        at += len + 1;
        let inline =
            |job: &mut LayoutJob, s: &str, st: St, p: &Palette| inline(job, s, st, p, hide);
        if line.starts_with("```") {
            fence = !fence;
            push(&mut job, line, dim, p);
            continue;
        }
        if fence {
            push(&mut job, line, St { code: true, ..base }, p);
            continue;
        }
        let hashes = line.bytes().take_while(|&b| b == b'#').count();
        if (1..=3).contains(&hashes) && line[hashes..].starts_with(' ') {
            let st = St {
                bold: true,
                size: Some([22.0, 19.0, 16.5][hashes - 1]),
                ..base
            };
            push(
                &mut job,
                &line[..hashes + 1],
                St {
                    dim: true,
                    hide,
                    ..st
                },
                p,
            );
            inline(&mut job, &line[hashes + 1..], st, p);
            continue;
        }
        if let Some(rest) = line.strip_prefix("> ") {
            push(&mut job, "> ", dim, p);
            inline(
                &mut job,
                rest,
                St {
                    italic: true,
                    ..base
                },
                p,
            );
            continue;
        }
        if line.len() >= 3 && line.bytes().all(|b| b == b'-') {
            push(&mut job, line, dim, p);
            continue;
        }
        if line.starts_with('|') {
            for (k, cell) in line.split('|').enumerate() {
                if k > 0 {
                    push(&mut job, "|", dim, p);
                }
                inline(&mut job, cell, base, p);
            }
            continue;
        }
        let bullet = ["- ", "* "].iter().find(|b| line.starts_with(**b));
        if let Some(b) = bullet {
            let rest = &line[2..];
            let low = rest.get(..4).map(str::to_lowercase);
            if low.as_deref() == Some("[ ] ") || low.as_deref() == Some("[x] ") {
                push(
                    &mut job,
                    &line[..2],
                    St {
                        dim: true,
                        hide,
                        ..base
                    },
                    p,
                );
                push(
                    &mut job,
                    &line[2..5],
                    St {
                        ghost: true,
                        ..base
                    },
                    p,
                );
                push(&mut job, " ", base, p);
                let done = low.as_deref() == Some("[x] ");
                inline(
                    &mut job,
                    &rest[4..],
                    St {
                        strike: done,
                        dim: done,
                        ..base
                    },
                    p,
                );
            } else {
                accent(&mut job, b);
                inline(&mut job, rest, base, p);
            }
            continue;
        }
        let digits = line.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && line[digits..].starts_with(". ") {
            accent(&mut job, &line[..digits + 2]);
            inline(&mut job, &line[digits + 2..], base, p);
            continue;
        }
        inline(&mut job, line, base, p);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_every_char() {
        let p = crate::theme::palette(false);
        let src = "# Head **b**\n- [x] done *i*\n1. one\n> q ==m==\n| a | `c` |\n```\nx **y**\n```\n[l](https://e.com) [[W]] ~~s~~ **unclosed\nhé ✓";
        assert_eq!(highlight(src, &p, None).text, src);
        assert_eq!(highlight(src, &p, Some((3, 3))).text, src);
    }
}
