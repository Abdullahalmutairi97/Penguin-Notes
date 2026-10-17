use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Response, Sense,
    Stroke, StrokeKind, TextStyle, Vec2, WidgetInfo, WidgetType,
};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct Palette {
    pub ink: Color32,
    pub ink_2: Color32,
    pub ink_3: Color32,
    pub line: Color32,
    pub paper: Color32,
    pub paper2: Color32,
    pub accent: Color32,
    pub accent2: Color32,
    pub tint: Color32,
}

pub fn palette(dark: bool) -> Palette {
    let c = |h: u32| Color32::from_rgb((h >> 16) as u8, (h >> 8) as u8, h as u8);
    let (ink, ink_2, ink_3, line, paper, paper2) = if dark {
        (
            c(0xf0ece2),
            c(0xc8c1b3),
            c(0x8a8278),
            c(0x3a332e),
            c(0x1a1614),
            c(0x221d1a),
        )
    } else {
        (
            c(0x1a1614),
            c(0x4a423d),
            c(0x8a807a),
            c(0xd9d0c0),
            c(0xf6f1e6),
            c(0xfbf7ec),
        )
    };
    Palette {
        ink,
        ink_2,
        ink_3,
        line,
        paper,
        paper2,
        accent: c(0xd94d2a),
        accent2: c(0xf0c12b),
        tint: Color32::from_rgba_unmultiplied(217, 77, 42, 38),
    }
}

pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

pub fn arabic(bold: bool) -> FontFamily {
    FontFamily::Name(if bold { "ar-bold" } else { "ar" }.into())
}

/// Arabic letters and marks. Arabic-Indic digits are left out: numbers read left to right.
pub fn is_arabic(c: char) -> bool {
    matches!(c as u32, 0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)
        && !matches!(c as u32, 0x0660..=0x0669 | 0x06F0..=0x06F9)
}

/// True when the first letter is Arabic, so the paragraph reads right to left.
pub fn is_rtl(s: &str) -> bool {
    s.chars()
        .find(|c| c.is_alphabetic())
        .is_some_and(is_arabic)
}

/// Splits `s` into (piece, is_arabic). Spaces and punctuation between two Arabic letters
/// stay in the Arabic piece; digits and Latin end it (the shaper would mirror them).
pub fn arabic_runs(s: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let (mut start, mut ar) = (0, false);
    // end of the last strong char of the current piece
    let mut last = 0;
    for (i, c) in s.char_indices() {
        let neutral = !c.is_alphanumeric() && !is_arabic(c);
        if neutral {
            continue;
        }
        let a = is_arabic(c);
        if a != ar {
            let cut = if ar { last } else { i };
            if cut > start {
                out.push((&s[start..cut], ar));
            }
            start = cut;
            ar = a;
        }
        last = i + c.len_utf8();
    }
    if ar && last < s.len() {
        out.push((&s[start..last], true));
        out.push((&s[last..], false));
    } else if start < s.len() {
        out.push((&s[start..], ar));
    }
    out
}

/// Text laid out with Arabic pieces in the Arabic font, for labels and single-line fields.
pub fn ar_job(text: &str, font: FontId, color: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let bold = font.family == self::bold();
    for (piece, ar) in arabic_runs(text) {
        let mut f = egui::TextFormat::simple(font.clone(), color);
        if ar {
            f.font_id.family = arabic(bold);
        }
        job.append(piece, 0.0, f);
    }
    job
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut f = FontDefinitions::default();
    f.font_data.insert(
        "kalam".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/Kalam-Regular.ttf"
        ))),
    );
    f.font_data.insert(
        "kalam-bold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/Kalam-Bold.ttf"
        ))),
    );
    // Arabic: Kalam and JetBrains Mono have no Arabic letters, so each family falls back to Naskh.
    f.font_data.insert(
        "arabic".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/NotoNaskhArabic-Regular.ttf"
        ))),
    );
    f.font_data.insert(
        "arabic-bold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/NotoNaskhArabic-Bold.ttf"
        ))),
    );
    let prop = f.families.get_mut(&FontFamily::Proportional).unwrap();
    prop.insert(0, "kalam".into());
    prop.insert(1, "arabic".into());
    let mut bold = vec!["kalam-bold".to_string(), "arabic-bold".to_string()];
    bold.extend(f.families[&FontFamily::Proportional].iter().cloned());
    f.families.insert(self::bold(), bold);
    // Arabic-first families: a whole Arabic phrase (spaces included) must come from one
    // font so it is shaped as a single right-to-left run, not word by word.
    let prop = f.families[&FontFamily::Proportional].clone();
    f.families
        .insert(arabic(false), [vec!["arabic".to_string()], prop.clone()].concat());
    f.families
        .insert(arabic(true), [vec!["arabic-bold".to_string()], prop].concat());
    let mono: [(&str, &'static [u8]); 4] = [
        (
            "mono",
            include_bytes!("../assets/JetBrainsMono-Regular.ttf"),
        ),
        (
            "mono-bold",
            include_bytes!("../assets/JetBrainsMono-Bold.ttf"),
        ),
        (
            "mono-italic",
            include_bytes!("../assets/JetBrainsMono-Italic.ttf"),
        ),
        (
            "mono-bold-italic",
            include_bytes!("../assets/JetBrainsMono-BoldItalic.ttf"),
        ),
    ];
    let mut fallback = f.families[&FontFamily::Monospace].clone();
    fallback.insert(0, "arabic".into());
    for (name, bytes) in mono {
        f.font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
        let mut fam = vec![name.to_string()];
        fam.extend(fallback.iter().cloned());
        if name == "mono" {
            f.families.insert(FontFamily::Monospace, fam);
        } else {
            f.families.insert(FontFamily::Name(name.into()), fam);
        }
    }
    ctx.set_fonts(f);
}

pub fn apply(ctx: &egui::Context, dark: bool) {
    let p = palette(dark);
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.paper;
    v.window_fill = p.paper2;
    v.extreme_bg_color = p.paper;
    v.faint_bg_color = p.paper2;
    v.code_bg_color = p.paper2;
    v.override_text_color = Some(p.ink);
    v.hyperlink_color = p.accent;
    v.window_stroke = Stroke::new(1.6, p.ink);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::epaint::Shadow {
        offset: [4, 4],
        blur: 0,
        spread: 0,
        color: Color32::from_black_alpha(45),
    };
    v.popup_shadow = egui::epaint::Shadow {
        offset: [3, 3],
        blur: 0,
        spread: 0,
        color: Color32::from_black_alpha(35),
    };
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(217, 77, 42, 80);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.text_cursor.stroke = Stroke::new(2.0, p.accent);
    // Panel separator lines and `ui.separator()` use this stroke.
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.2, p.line);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.ink);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.fg_stroke = Stroke::new(1.0, p.ink);
        w.corner_radius = CornerRadius::same(6);
        w.expansion = 0.0;
    }
    v.widgets.inactive.bg_fill = p.paper2;
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.bg_stroke = Stroke::new(1.2, p.ink_3);
    v.widgets.hovered.weak_bg_fill = p.paper2;
    v.widgets.hovered.bg_fill = p.paper2;
    v.widgets.hovered.bg_stroke = Stroke::new(1.2, p.ink);
    v.widgets.active.weak_bg_fill = p.tint;
    v.widgets.active.bg_fill = p.tint;
    v.widgets.active.bg_stroke = Stroke::new(1.2, p.accent);
    v.widgets.open = v.widgets.active;
    let theme = if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    ctx.set_theme(theme);
    ctx.set_visuals_of(theme, v);
    ctx.style_mut_of(theme, |s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.menu_margin = egui::Margin::same(6);
        s.interaction.selectable_labels = false;
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(15.0)),
            (TextStyle::Button, FontId::proportional(15.0)),
            (TextStyle::Heading, FontId::new(22.0, bold())),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
    });
}

pub enum Deco {
    None,
    Dot(Color32),
    Chevron,
}

/// Rounded pill button. Painted by hand so the status dot / chevron sit inside
/// the pill and nothing depends on glyphs the handwriting font lacks.
pub fn pill_ex(ui: &mut egui::Ui, p: &Palette, text: &str, active: bool, deco: Deco) -> Response {
    let font = FontId::proportional(14.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, p.ink);
    let extra = if matches!(deco, Deco::None) {
        0.0
    } else {
        16.0
    };
    let size = Vec2::new(galley.size().x + 26.0 + extra, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, active, text));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if ui.is_rect_visible(rect) {
        let hov = resp.hovered();
        let (fill, stroke) = match (active, hov) {
            (true, _) => (p.tint, p.accent),
            (false, true) => (p.paper2, p.ink),
            (false, false) => (Color32::TRANSPARENT, p.ink_3),
        };
        let painter = ui.painter();
        painter.rect(rect, 14, fill, Stroke::new(1.5, stroke), StrokeKind::Inside);
        let mut x = rect.left() + 13.0;
        if let Deco::Dot(c) = deco {
            painter.circle(
                egui::pos2(x + 4.0, rect.center().y),
                4.0,
                c,
                Stroke::new(1.0, p.ink),
            );
            x += 14.0;
        }
        painter.galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley.clone(),
            p.ink,
        );
        if let Deco::Chevron = deco {
            let c = egui::pos2(rect.right() - 15.0, rect.center().y + 1.0);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    c + Vec2::new(-4.0, -2.5),
                    c + Vec2::new(4.0, -2.5),
                    c + Vec2::new(0.0, 2.5),
                ],
                p.ink_2,
                Stroke::NONE,
            ));
        }
    }
    resp
}

pub fn pill(ui: &mut egui::Ui, p: &Palette, text: &str, active: bool) -> Response {
    pill_ex(ui, p, text, active, Deco::None)
}

pub fn pill_width(ui: &egui::Ui, text: &str, deco: &Deco) -> f32 {
    let w = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::proportional(14.5), Color32::WHITE)
        .size()
        .x;
    w + 26.0
        + if matches!(deco, Deco::None) {
            0.0
        } else {
            16.0
        }
}

/// Filled accent button (Send, Test connection, + New).
pub fn primary(ui: &mut egui::Ui, p: &Palette, text: &str) -> Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(Color32::WHITE).size(14.5))
            .fill(p.accent)
            .stroke(Stroke::new(1.5, p.ink))
            .corner_radius(14)
            .min_size(Vec2::new(0.0, 28.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Joined segmented control; returns the index that was clicked.
pub fn segmented(
    ui: &mut egui::Ui,
    p: &Palette,
    items: &[&str],
    selected: Option<usize>,
) -> Option<usize> {
    let font = FontId::proportional(14.5);
    let galleys: Vec<_> = items
        .iter()
        .map(|t| {
            ui.painter()
                .layout_no_wrap(t.to_string(), font.clone(), p.ink)
        })
        .collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + 26.0).collect();
    let (outer, _) = ui.allocate_exact_size(Vec2::new(widths.iter().sum(), 28.0), Sense::hover());
    let mut clicked = None;
    let mut x = outer.left();
    let painter = ui.painter().clone();
    painter.rect_filled(outer, 14, p.paper);
    for (i, g) in galleys.into_iter().enumerate() {
        let r = egui::Rect::from_min_size(egui::pos2(x, outer.top()), Vec2::new(widths[i], 28.0));
        let resp = ui
            .interact(r, ui.id().with(("seg", items[i])), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        resp.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, true, selected == Some(i), items[i])
        });
        let cr = match i {
            0 => CornerRadius {
                nw: 14,
                sw: 14,
                ne: 0,
                se: 0,
            },
            _ if i == items.len() - 1 => CornerRadius {
                nw: 0,
                sw: 0,
                ne: 14,
                se: 14,
            },
            _ => CornerRadius::ZERO,
        };
        if selected == Some(i) {
            painter.rect_filled(r, cr, p.tint);
        } else if resp.hovered() {
            painter.rect_filled(r, cr, p.paper2);
        }
        if i > 0 {
            painter.line_segment(
                [
                    r.left_top() + Vec2::new(0.0, 5.0),
                    r.left_bottom() - Vec2::new(0.0, 5.0),
                ],
                Stroke::new(1.0, p.line),
            );
        }
        painter.galley(r.center() - g.size() / 2.0, g, p.ink);
        if resp.clicked() {
            clicked = Some(i);
        }
        x += widths[i];
    }
    painter.rect_stroke(outer, 14, Stroke::new(1.5, p.ink), StrokeKind::Inside);
    clicked
}

/// Small square-ish formatting button used in the editor toolbar.
pub fn tool(ui: &mut egui::Ui, p: &Palette, label: &str, tip: &str) -> Response {
    let rt = match label {
        "B" => egui::RichText::new(label).family(bold()).size(15.0),
        "I" => egui::RichText::new(label).italics().size(15.0),
        "S" => egui::RichText::new(label).strikethrough().size(15.0),
        _ => egui::RichText::new(label).monospace().size(12.5),
    };
    ui.add(
        egui::Button::new(rt.color(p.ink))
            .min_size(Vec2::new(28.0, 26.0))
            .frame_when_inactive(false),
    )
    .on_hover_text(tip)
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn caps(t: &str, p: &Palette, size: f32) -> egui::RichText {
    egui::RichText::new(t.to_uppercase())
        .monospace()
        .size(size)
        .color(p.ink_3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_arabic_runs() {
        assert_eq!(
            arabic_runs("abc مرحبا بالعالم. 12"),
            vec![("abc ", false), ("مرحبا بالعالم", true), (". 12", false)]
        );
        assert_eq!(arabic_runs("plain text"), vec![("plain text", false)]);
        assert_eq!(arabic_runs("شراء الخبز"), vec![("شراء الخبز", true)]);
        assert!(is_rtl("  - مرحبا hello") && !is_rtl("hi مرحبا"));
    }
}
