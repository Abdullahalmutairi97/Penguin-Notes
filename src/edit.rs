//! Text-selection edit operations for the toolbar. Indices are char indices.

fn slice(c: &[char], a: usize, b: usize) -> String {
    c[a.min(c.len())..b.min(c.len())].iter().collect()
}

pub struct Edit {
    pub text: String,
    pub sel: (usize, usize),
}

pub fn wrap(v: &str, s: usize, e: usize, a: &str, b: &str) -> Edit {
    let c: Vec<char> = v.chars().collect();
    let (s, e) = (s.min(c.len()), e.min(c.len()));
    let (al, bl) = (a.chars().count(), b.chars().count());
    let mid = slice(&c, s, e);
    if s >= al && slice(&c, s - al, s) == a && slice(&c, e, e + bl) == b {
        let text = format!(
            "{}{}{}",
            slice(&c, 0, s - al),
            mid,
            slice(&c, e + bl, c.len())
        );
        return Edit {
            text,
            sel: (s - al, e - al),
        };
    }
    let ml = mid.chars().count();
    if mid.starts_with(a) && mid.ends_with(b) && ml >= al + bl {
        let inner = slice(&mid.chars().collect::<Vec<_>>(), al, ml - bl);
        let text = format!("{}{}{}", slice(&c, 0, s), inner, slice(&c, e, c.len()));
        return Edit {
            text,
            sel: (s, e - al - bl),
        };
    }
    let text = format!("{}{a}{mid}{b}{}", slice(&c, 0, s), slice(&c, e, c.len()));
    Edit {
        text,
        sel: (s + al, e + al),
    }
}

pub fn prefix(v: &str, s: usize, e: usize, p: &str) -> Edit {
    let c: Vec<char> = v.chars().collect();
    let s = s.min(c.len());
    let e = e.min(c.len());
    let ls = c[..s].iter().rposition(|&x| x == '\n').map_or(0, |i| i + 1);
    let pl = p.chars().count();
    if slice(&c, ls, ls + pl) == p {
        let text = format!("{}{}", slice(&c, 0, ls), slice(&c, ls + pl, c.len()));
        return Edit {
            text,
            sel: (ls.max(s.saturating_sub(pl)), ls.max(e.saturating_sub(pl))),
        };
    }
    let text = format!("{}{p}{}", slice(&c, 0, ls), slice(&c, ls, c.len()));
    Edit {
        text,
        sel: (s + pl, e + pl),
    }
}

pub fn insert(v: &str, s: usize, e: usize, t: &str) -> Edit {
    let c: Vec<char> = v.chars().collect();
    let (s, e) = (s.min(c.len()), e.min(c.len()));
    let text = format!("{}{t}{}", slice(&c, 0, s), slice(&c, e, c.len()));
    let p = s + t.chars().count();
    Edit { text, sel: (p, p) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_toggles() {
        let a = wrap("hello world", 0, 5, "**", "**");
        assert_eq!(a.text, "**hello** world");
        assert_eq!(a.sel, (2, 7));
        let b = wrap(&a.text, a.sel.0, a.sel.1, "**", "**");
        assert_eq!(b.text, "hello world");
    }

    #[test]
    fn prefix_toggles() {
        let a = prefix("one\ntwo", 5, 5, "- ");
        assert_eq!(a.text, "one\n- two");
        assert_eq!(prefix(&a.text, a.sel.0, a.sel.1, "- ").text, "one\ntwo");
    }

    #[test]
    fn insert_replaces_selection() {
        let a = insert("héllo", 1, 3, "XY");
        assert_eq!(a.text, "hXYlo");
        assert_eq!(a.sel, (3, 3));
    }
}
