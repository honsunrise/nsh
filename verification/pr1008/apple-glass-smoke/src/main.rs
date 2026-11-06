use nsh_core::prelude::*;
#[cfg(target_os="ios")]
use nsh_platform_ios as platform;
#[cfg(target_os="macos")]
use nsh_platform_macos as platform;
#[component]
fn GlassFixture() -> impl IntoElement {
    let tint=signal(Some(AdaptiveColor::from(rgba(0xff000080))));
    let glass=signal(nsh_core::GlassEffect::Regular);
    view().size_full().p(px(20.)).gap(px(16.)).bg(rgb(0x183858))
        .child(text("Apple glass tint"))
        .child(view().h(px(180.)).w_full().bg(rgb(0x406020))
            .child(view().absolute().left(px(0.)).top(px(0.)).w(px(170.)).h_full().bg(rgb(0xa08040)))
            .child(view().absolute().left(px(16.)).right(px(16.)).top(px(16.)).bottom(px(16.))
                .test_id("glass").rounded(px(18.)).backdrop_blur(px(12.))
                .style_with(move |s| s.glass_effect(glass.get()))
                .glass_tint_color(None).glass_tint_color(move || tint.get())))
        .child(view().flex_row().gap(px(8.)).justify_center()
            .child(nsh_base::button().accessibility_label("Red").w(px(76.)).h(px(36.)).on_press(move || tint.set(Some(rgba(0xff000080).into()))).child(text("Red")))
            .child(nsh_base::button().accessibility_label("Blue").w(px(76.)).h(px(36.)).on_press(move || tint.set(Some(rgba(0x0000ff80).into()))).child(text("Blue")))
            .child(nsh_base::button().accessibility_label("Clear").w(px(76.)).h(px(36.)).on_press(move || tint.set(Some(AdaptiveColor::TRANSPARENT))).child(text("Clear")))
            .child(nsh_base::button().accessibility_label("Default").w(px(76.)).h(px(36.)).on_press(move || tint.set(None)).child(text("Default"))))
        .child(view().flex_row().gap(px(8.)).justify_center()
            .child(nsh_base::button().accessibility_label("Blur").w(px(76.)).h(px(36.)).on_press(move || glass.set(nsh_core::GlassEffect::None)).child(text("Blur")))
            .child(nsh_base::button().accessibility_label("Glass").w(px(76.)).h(px(36.)).on_press(move || glass.set(nsh_core::GlassEffect::Regular)).child(text("Glass"))))
}
#[cfg(target_os="ios")]
mod host;
fn main() {
    platform::set_ui_sets(vec![nsh_base::registry()]);
    #[cfg(target_os="ios")]
    host::run();
    #[cfg(target_os="macos")]
    platform::run_app(platform::AppSource::Rust(platform::Root::of::<GlassFixture>()),platform::AppOptions::default());
}
