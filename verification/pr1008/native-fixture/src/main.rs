use nsh_core::{prelude::*, BlendMode, GlassEffect};
use nsh_core::element::ImageSrc;
use nsh_core::element::desktop::{self, MenuItem};
use nsh_core::desktop::MenuRole;
#[cfg(feature="materials")]
use nsh_core::{MacOsBackdrop, MacOsBackdropMaterial, MacOsBackdropBlending, MacOsBackdropState};
#[cfg(feature="materials")]
fn backdrop(material: MacOsBackdropMaterial, blending: MacOsBackdropBlending, state: MacOsBackdropState) -> MacOsBackdrop {
    let mut value = MacOsBackdrop::default();
    value.material = material; value.blending = blending; value.state = state; value
}
#[component]
fn Fixture() -> impl IntoElement {
    let blend = signal(BlendMode::Normal);
    let glass = signal(GlassEffect::Regular);
    let tint = signal(0_u8);
    let clicks = signal(0_u32);
    #[cfg(feature="materials")]
    let material = signal(None::<MacOsBackdrop>);
    let modes = [BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen, BlendMode::Overlay,
        BlendMode::Difference, BlendMode::Color, BlendMode::PlusDarker, BlendMode::PlusLighter];
    let mut menus = vec![
        MenuItem::submenu("Smoke", [MenuItem::role(MenuRole::Quit)]),
        MenuItem::submenu("Blend", modes.map(|mode| MenuItem::new(mode.name()).on_select(move || blend.set(mode)))),
        MenuItem::submenu("Effect", [
            MenuItem::new("Glass regular").on_select(move || glass.set(GlassEffect::Regular)),
            MenuItem::new("Glass clear").on_select(move || glass.set(GlassEffect::Clear)),
            MenuItem::new("Glass none").on_select(move || glass.set(GlassEffect::None)),
            MenuItem::new("Tint red").on_select(move || tint.set(1)),
            MenuItem::new("Tint blue").on_select(move || tint.set(2)),
            MenuItem::new("Tint clear").on_select(move || tint.set(0)),
        ])];
    #[cfg(feature="materials")]
    menus.push(MenuItem::submenu("Material", [
        MenuItem::new("HUD behind active").on_select(move || material.set(Some(backdrop(MacOsBackdropMaterial::HudWindow,MacOsBackdropBlending::BehindWindow,MacOsBackdropState::Active)))),
        MenuItem::new("HUD behind follow").on_select(move || material.set(Some(backdrop(MacOsBackdropMaterial::HudWindow,MacOsBackdropBlending::BehindWindow,MacOsBackdropState::FollowsWindowActiveState)))),
        MenuItem::new("HUD behind inactive").on_select(move || material.set(Some(backdrop(MacOsBackdropMaterial::HudWindow,MacOsBackdropBlending::BehindWindow,MacOsBackdropState::Inactive)))),
        MenuItem::new("Sidebar within inactive").on_select(move || material.set(Some(backdrop(MacOsBackdropMaterial::Sidebar,MacOsBackdropBlending::WithinWindow,MacOsBackdropState::Inactive)))),
        MenuItem::new("Remove material").on_select(move || material.set(None)),
    ]));
    desktop::set_application_menu(menus);
    let surface=view().absolute().left(px(24.)).top(px(24.)).w(px(524.)).h(px(132.)).rounded(px(16.)).test_id("material");
    #[cfg(feature="materials")]
    let surface=surface.macos_backdrop(move || material.get());
    let surface=surface.style_with(move |s| {
        let s=s.glass_effect(glass.get()).backdrop_blur(px(12.));
        #[cfg(feature="materials")]
        let s=match tint.get() { 1=>s.glass_tint_color(rgba(0xff000080)),2=>s.glass_tint_color(rgba(0x0000ff80)),_=>s };
        s
    }).child(nsh_base::button().accessibility_label(move || Some(format!("Clicks: {}",clicks.get()).into())).h(px(36.)).w(px(160.))
        .on_press(move || clicks.update(|n|*n+=1)).child(text(move || format!("Clicks: {}",clicks.get()))));
    view().size_full().p(px(20.)).gap(px(20.))
        .child(view().flex_row().gap(px(20.))
            .child(box_case("plain",view().size_full().bg(rgb(0xffffff)).style_with(move |s| s.mix_blend_mode(blend.get()))))
            .child(box_case("text",text("X").size_full().bg(rgb(0xffffff)).text_color(rgb(0x000000)).style_with(move |s| s.mix_blend_mode(blend.get()))))
            .child(box_case("svg",svg(ImageSrc::svg_markup("<svg xmlns='http://www.w3.org/2000/svg' width='128' height='96' viewBox='0 0 128 96'><rect width='128' height='96' fill='white'/></svg>")).size_full().style_with(move |s| s.mix_blend_mode(blend.get()))))
            .child(box_case("clipped",view().size_full().bg(rgb(0xffffff)).overflow_hidden().shadow_sm().style_with(move |s| s.mix_blend_mode(blend.get()))
                .child(view().absolute().left(px(0.)).top(px(0.)).w(px(20.)).h_full().bg(rgb(0x000000)))))
        ).child(view().w(px(572.)).h(px(180.)).bg(rgb(0x2050b0))
            .child(view().absolute().left(px(0.)).top(px(0.)).w(px(286.)).h_full().bg(rgb(0xe08020))).child(surface))
}
fn box_case(label:&'static str, child:impl IntoElement)->View {
    view().w(px(128.)).h(px(96.)).bg(rgb(0xff0000)).test_id(label).child(child)
}
#[cfg(feature="materials")]
fn embedded(opts: nsh_platform_macos::AppOptions) {
    use nsh_platform_macos::{App, AppView, Root, objc2::{MainThreadMarker, MainThreadOnly}, objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSWindow, NSWindowStyleMask, NSBackingStoreType, NSColor}, objc2_foundation::{NSRect, NSPoint, NSSize, NSString}};
    let mtm=MainThreadMarker::new().unwrap();
    let app=NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let window=unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm),NSRect::new(NSPoint::new(100.,100.),NSSize::new(640.,420.)),NSWindowStyleMask::Titled|NSWindowStyleMask::Closable|NSWindowStyleMask::Resizable,NSBackingStoreType::Buffered,false) };
    unsafe { window.setReleasedWhenClosed(false); }
    window.setTitle(&NSString::from_str("Initial opaque embedding host"));
    window.setOpaque(true); window.setBackgroundColor(Some(&NSColor::blueColor()));
    let view=AppView::new(mtm,opts);
    window.setContentView(Some(&view.view()));
    view.load(App::Rust(Root::of::<Fixture>()));
    assert!(window.isOpaque());
    assert_eq!(window.backgroundColor().alphaComponent(),1.);
    window.makeKeyAndOrderFront(None);
    app.run();
    view.stop();
}
fn main() {
    nsh_platform_macos::set_ui_sets(vec![nsh_base::registry()]);
    let mut opts=nsh_platform_macos::AppOptions { title:"NSH PR Material Verification".into(), app_id:"dev.nsh.pr-material-verification".into(),size:(640,420),..Default::default() };
    #[cfg(feature="materials")]
    { opts.transparent=true; }
    #[cfg(feature="materials")]
    if std::env::var_os("NSH_VERIFY_EMBEDDED").is_some() { embedded(opts); return; }
    nsh_platform_macos::run_app(nsh_platform_macos::AppSource::Rust(nsh_platform_macos::Root::of::<Fixture>()),opts);
}
