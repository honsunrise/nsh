import UIKit
import Foundation
private var timer: Timer?
private func views(_ root: UIView) -> [UIView] { [root] + root.subviews.flatMap(views) }
private func snapshot(_ window: UIWindow) -> [String: Any] {
    let glassClass: AnyClass?=NSClassFromString("UIGlassEffect")
    let rows: [[String: Any]]=views(window).map { view in
        var row: [String: Any]=["class":String(describing:type(of:view)),"id":view.accessibilityIdentifier ?? "","label":view.accessibilityLabel ?? "","object":String(describing:Unmanaged.passUnretained(view).toOpaque()),"width":view.bounds.width,"height":view.bounds.height]
        if let effectView=view as? UIVisualEffectView, let effect=effectView.effect {
            row["effectClass"]=String(describing:type(of:effect))
            row["glass"]=glassClass.map { effect.isKind(of:$0) } ?? false
            if row["glass"] as? Bool == true {
                row["tint"]=NSNull()
                if let color=effect.value(forKey:"tintColor") as? UIColor {
                    var r:CGFloat=0,g:CGFloat=0,b:CGFloat=0,a:CGFloat=0
                    if color.getRed(&r,green:&g,blue:&b,alpha:&a) { row["tint"]=[r,g,b,a] }
                }
            }
        }
        return row
    }
    return ["pid":ProcessInfo.processInfo.processIdentifier,"style":window.traitCollection.userInterfaceStyle.rawValue,"glassAvailable":glassClass != nil,"views":rows]
}
@_cdecl("apple_glass_probe_start") public func appleGlassProbeStart() {
    DispatchQueue.main.async {
        guard let path=ProcessInfo.processInfo.environment["APPLE_GLASS_COMMAND"] else { return }
        timer=Timer.scheduledTimer(withTimeInterval:0.05,repeats:true) { _ in
            guard let bytes=FileManager.default.contents(atPath:path),let command=(try? JSONSerialization.jsonObject(with:bytes)) as? [String:Any],let action=command["action"] as? String else { return }
            let windows=UIApplication.shared.connectedScenes.compactMap {$0 as? UIWindowScene}.flatMap {$0.windows}
            guard let window=windows.first(where:{!$0.isHidden}) else { return }
            var result:[String:Any]
            switch action {
            case "inspect": result=snapshot(window)
            case "appearance":
                window.overrideUserInterfaceStyle = command["dark"] as? Bool == true ? .dark : .light
                result=["style":window.traitCollection.userInterfaceStyle.rawValue]
            case "press":
                let target=views(window).first {$0.accessibilityLabel == command["label"] as? String}
                result=["activated":target?.accessibilityActivate() ?? false]
            default: result=["error":"unknown action"]
            }
            if let out=command["path"] as? String { try! JSONSerialization.data(withJSONObject:result,options:[.prettyPrinted,.sortedKeys]).write(to:URL(fileURLWithPath:out)) }
            try! FileManager.default.removeItem(atPath:path)
        }
        RunLoop.main.add(timer!,forMode:.common)
    }
}
