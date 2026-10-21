//! The CSS that text is measured and drawn with. It depends on nothing of
//! the browser, so its tests run off WebAssembly too (`tests/text`).

use nsh_core::ResolvedTextStyle;

/// The system font stack (what `fontFamily` unset means).
const SYSTEM_FONT: &str = "-apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, \
                                      \"Helvetica Neue\", Arial, \"PingFang SC\", \
                                      \"Noto Sans SC\", \"Microsoft YaHei\", sans-serif";

/// The CSS `font` shorthand of a style.
pub(crate) fn css_font(style: &ResolvedTextStyle) -> String {
    let family = match style.font_family.map(|f| family_list(&f.name())) {
        Some(list) if !list.is_empty() => format!("{list}, {SYSTEM_FONT}"),
        _ => SYSTEM_FONT.to_owned(),
    };
    format!(
        "{}{} {}px {family}",
        if style.italic { "italic " } else { "" },
        style.font_weight,
        style.font_size.0
    )
}

/// A `fontFamily` value as a CSS family list: one family or a
/// comma-separated stack. Generic families and the system font keywords
/// stay bare (quoted, `"monospace"` names a font called monospace, which
/// no system has, and the text falls back to the next family); other
/// names are quoted, and names the app quoted keep their quotes.
pub(crate) fn family_list(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    for family in split_families(value) {
        if !out.is_empty() {
            out.push_str(", ");
        }
        if family.starts_with(['"', '\'']) || is_keyword(family) {
            out.push_str(family);
        } else {
            out.push('"');
            for c in family.chars() {
                if c == '"' || c == '\\' {
                    out.push('\\');
                }
                out.push(c);
            }
            out.push('"');
        }
    }
    out
}

/// The families of a list, trimmed, without empty ones; commas inside
/// quotes do not separate.
fn split_families(value: &str) -> impl Iterator<Item = &str> {
    let mut rest = value;
    std::iter::from_fn(move || {
        loop {
            if rest.is_empty() {
                return None;
            }
            let mut quote = None;
            let mut end = rest.len();
            for (i, c) in rest.char_indices() {
                match quote {
                    Some(q) if c == q => quote = None,
                    Some(_) => {}
                    None if c == '"' || c == '\'' => quote = Some(c),
                    None if c == ',' => {
                        end = i;
                        break;
                    }
                    None => {}
                }
            }
            let family = rest[..end].trim();
            rest = rest.get(end + 1..).unwrap_or("");
            if !family.is_empty() {
                return Some(family);
            }
        }
    })
}

/// A CSS generic family (CSS Fonts 4) or a system font keyword: written
/// without quotes. CSS matches them ASCII case-insensitively.
fn is_keyword(family: &str) -> bool {
    const KEYWORDS: [&str; 14] = [
        "serif",
        "sans-serif",
        "monospace",
        "cursive",
        "fantasy",
        "system-ui",
        "ui-serif",
        "ui-sans-serif",
        "ui-monospace",
        "ui-rounded",
        "math",
        "emoji",
        "fangsong",
        "-apple-system",
    ];
    KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(family))
}
