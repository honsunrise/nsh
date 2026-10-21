//! The web backend's text rules, off WebAssembly: the crate itself is empty
//! there (`lib.rs`), so this binary builds the modules that depend on
//! nothing of the browser from their sources.

#[path = "../../src/text/css.rs"]
#[allow(dead_code)]
mod css;

mod fonts;
