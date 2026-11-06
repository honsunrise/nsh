use std::cell::RefCell;
use nsh_platform_ios::{AppView, Root, SurfaceOptions};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class, rc::Retained};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};
use objc2_ui_kit::{UIApplication, UIApplicationDelegate, UIResponder, UIScene, UISceneConfiguration, UISceneConnectionOptions, UISceneDelegate, UISceneSession, UIViewController, UIWindow, UIWindowScene, UIWindowSceneDelegate};
thread_local! {
    static HOST: RefCell<Option<(Retained<UIWindow>, AppView)>> = const { RefCell::new(None) };
}
define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    struct GlassAppDelegate;
    unsafe impl NSObjectProtocol for GlassAppDelegate {}
    unsafe impl UIApplicationDelegate for GlassAppDelegate {
        #[unsafe(method_id(application:configurationForConnectingSceneSession:options:))]
        fn configuration(&self, _app: &UIApplication, session: &UISceneSession, _options: &UISceneConnectionOptions) -> Retained<UISceneConfiguration> {
            let config = UISceneConfiguration::initWithName_sessionRole(UISceneConfiguration::alloc(self.mtm()), None, &session.role());
            unsafe { config.setDelegateClass(Some(GlassSceneDelegate::class())) };
            config
        }
    }
);
define_class!(
    #[unsafe(super(UIResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    struct GlassSceneDelegate;
    unsafe impl NSObjectProtocol for GlassSceneDelegate {}
    unsafe impl UISceneDelegate for GlassSceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn connect(&self, scene: &UIScene, _session: &UISceneSession, _options: &UISceneConnectionOptions) {
            let scene = scene.downcast_ref::<UIWindowScene>().expect("window scene");
            let mtm = self.mtm();
            if let Some(restrictions) = scene.sizeRestrictions() {
                let size = objc2_core_foundation::CGSize::new(430.0, 500.0);
                restrictions.setMinimumSize(size);
                restrictions.setMaximumSize(size);
            }
            let window = UIWindow::initWithWindowScene(UIWindow::alloc(mtm), scene);
            let controller = UIViewController::new(mtm);
            let parent = controller.view().expect("root view");
            window.setRootViewController(Some(&controller));
            window.makeKeyAndVisible();
            let app = AppView::new(mtm, parent.bounds(), SurfaceOptions::default());
            parent.addSubview(app.view());
            app.run(Root::of::<super::GlassFixture>());
            HOST.with(|host| *host.borrow_mut() = Some((window, app)));
        }
    }
    unsafe impl UIWindowSceneDelegate for GlassSceneDelegate {}
);
pub fn run() -> ! {
    let mtm = MainThreadMarker::new().expect("main thread");
    let delegate = NSString::from_str(GlassAppDelegate::class().name().to_str().unwrap());
    UIApplication::main(None, Some(&delegate), mtm)
}
