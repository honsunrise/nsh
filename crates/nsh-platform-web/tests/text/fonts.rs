//! The CSS `font` of text styles (`css::css_font`).

use nsh_core::{FontFamily, Points, ResolvedTextStyle};

use crate::css::{css_font, family_list};

fn style(family: Option<&str>) -> ResolvedTextStyle {
    ResolvedTextStyle {
        font_family: family.map(FontFamily::new),
        font_size: Points(14.0),
        font_weight: 400,
        ..ResolvedTextStyle::default()
    }
}

#[test]
fn generic_families_stay_unquoted() {
    for generic in [
        "monospace",
        "ui-monospace",
        "serif",
        "sans-serif",
        "system-ui",
        "cursive",
    ] {
        assert_eq!(family_list(generic), generic);
    }
    // CSS matches them ASCII case-insensitively.
    assert_eq!(family_list("Monospace"), "Monospace");
    assert_eq!(family_list("-apple-system"), "-apple-system");
}

#[test]
fn font_names_are_quoted() {
    assert_eq!(family_list("Fira Code"), "\"Fira Code\"");
    assert_eq!(family_list("Pacifico"), "\"Pacifico\"");
    // A name that happens to contain a keyword is still a name.
    assert_eq!(family_list("monospace2"), "\"monospace2\"");
    assert_eq!(family_list("a\"b"), "\"a\\\"b\"");
}

#[test]
fn stacks_keep_each_family_as_written() {
    assert_eq!(
        family_list("\"Fira Code\", ui-monospace,monospace"),
        "\"Fira Code\", ui-monospace, monospace"
    );
    assert_eq!(
        family_list("Menlo, 'DejaVu Sans Mono', monospace"),
        "\"Menlo\", 'DejaVu Sans Mono', monospace"
    );
    // A comma inside quotes is part of the name.
    assert_eq!(family_list("\"A, B\", serif"), "\"A, B\", serif");
    assert_eq!(family_list(" , "), "");
}

#[test]
fn the_shorthand_puts_the_family_before_the_system_stack() {
    let mono = css_font(&style(Some("monospace")));
    assert!(
        mono.starts_with("400 14px monospace, -apple-system, "),
        "{mono}"
    );
    assert!(!mono.contains("\"monospace\""), "{mono}");
    let named = css_font(&style(Some("Fira Code")));
    assert!(
        named.starts_with("400 14px \"Fira Code\", -apple-system, "),
        "{named}"
    );
    let system = css_font(&style(None));
    assert!(system.starts_with("400 14px -apple-system, "), "{system}");
    assert!(system.ends_with(", sans-serif"), "{system}");
    let italic = ResolvedTextStyle {
        italic: true,
        font_weight: 700,
        ..style(None)
    };
    assert!(css_font(&italic).starts_with("italic 700 14px "));
    assert_eq!(css_font(&style(Some(" "))), system);
}
