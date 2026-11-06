import AppKit
import Foundation
private var probeTimer: Timer?
private var probeActivity: NSObjectProtocol?
private var recording = false
private var ownedHost: NSWindow?
private func menuEntry(_ menu: NSMenu, _ title: String) -> (NSMenu, Int)? {
    for (index, item) in menu.items.enumerated() {
        if item.title == title { return (menu, index) }
        if let child = item.submenu, let found = menuEntry(child, title) { return found }
    }
    return nil
}
private func snapshot(_ window: NSWindow, _ path: String) throws {
    var rows: [[String: Any]] = []
    let height = window.contentView!.bounds.height
    func visit(_ view: NSView) {
        let frame = view.convert(view.bounds, to: nil)
        var row: [String: Any] = ["class": String(describing: type(of: view)), "x": frame.minX, "y": height - frame.maxY, "width": frame.width, "height": frame.height, "opacity": view.alphaValue, "hidden": view.isHiddenOrHasHiddenAncestor, "id": view.identifier?.rawValue ?? "", "label": view.accessibilityLabel() ?? "", "role": view.accessibilityRole()?.rawValue ?? "", "object": String(describing: Unmanaged.passUnretained(view).toOpaque())]
        if let effect = view as? NSVisualEffectView {
            row["material"] = effect.material.rawValue
            row["blending"] = effect.blendingMode.rawValue
            row["state"] = effect.state.rawValue
        }
        if let cls = NSClassFromString("NSGlassEffectView"), view.isKind(of: cls) {
            row["glassStyle"] = view.value(forKey: "style")
            if let color = (view.value(forKey: "tintColor") as? NSColor)?.usingColorSpace(.sRGB) {
                row["tint"] = [color.redComponent, color.greenComponent, color.blueComponent, color.alphaComponent]
            }
        }
        if let filter = view.layer?.compositingFilter { row["blendFilter"] = String(describing: filter) }
        row["animationKeys"] = view.layer?.animationKeys() ?? []
        if let live = view.layer?.presentation() {
            row["presentation"] = ["x": live.frame.minX, "y": live.frame.minY, "width": live.bounds.width, "height": live.bounds.height, "opacity": Double(live.opacity), "translateX": live.transform.m41, "translateY": live.transform.m42]
        }
        if let field = view as? NSTextField { row["text"] = field.stringValue }
        if let button = view as? NSButton { row["text"] = button.title; row["buttonState"] = button.state.rawValue }
        rows.append(row)
        for child in view.subviews { visit(child) }
    }
    visit(window.contentView!)
    for (name, kind) in [("close", NSWindow.ButtonType.closeButton), ("minimize", .miniaturizeButton), ("zoom", .zoomButton)] {
        if let button = window.standardWindowButton(kind) {
            let frame = button.convert(button.bounds, to: nil)
            rows.append(["class": "TrafficLight", "id": name, "x": frame.minX, "y": height - frame.maxY, "width": frame.width, "height": frame.height, "hidden": button.isHiddenOrHasHiddenAncestor])
        }
    }
    var result: [String: Any] = ["title": window.title, "pid": ProcessInfo.processInfo.processIdentifier, "window": window.windowNumber, "width": window.contentView!.bounds.width, "height": height, "opaque": window.isOpaque, "active": NSApp.isActive, "key": window.isKeyWindow, "views": rows]
    result["backgroundAlpha"] = window.backgroundColor.alphaComponent
    result["appearance"] = window.effectiveAppearance.name.rawValue
    if let responder = window.firstResponder { result["firstResponder"] = String(describing: type(of: responder)) }
    if let editor = window.firstResponder as? NSTextView {
        result["editingText"] = editor.string
        result["selection"] = [editor.selectedRange().location, editor.selectedRange().length]
    }
    try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: path))
}
@_cdecl("rexnsh_final_probe_start") public func startFinalProbe() {
    DispatchQueue.main.async {
        guard let path = ProcessInfo.processInfo.environment["REXNSH_COMPOSITION_COMMAND"] else { return }
        probeActivity = ProcessInfo.processInfo.beginActivity(options: [.userInitiated, .latencyCritical], reason: "Native release verification")
        probeTimer = Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { _ in
            guard !recording, let bytes = FileManager.default.contents(atPath: path),
                  let command = try? JSONSerialization.jsonObject(with: bytes) as? [String: Any],
                  let action = command["action"] as? String else { return }
            if action == "windows" {
                let rows = NSApp.windows.map { ["title": $0.title, "window": $0.windowNumber, "visible": $0.isVisible, "key": $0.isKeyWindow, "miniaturized": $0.isMiniaturized, "class": String(describing: type(of: $0))] as [String: Any] }
                let result: [String: Any] = ["pid": ProcessInfo.processInfo.processIdentifier, "active": NSApp.isActive, "windows": rows]
                try! JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: command["path"] as! String))
                try! FileManager.default.removeItem(atPath: path)
                return
            }
            if action == "menus" {
                var rows: [[String: Any]] = []
                func visit(_ menu: NSMenu, _ parent: String) {
                    for item in menu.items {
                        let label = parent.isEmpty ? item.title : parent + "/" + item.title
                        rows.append(["title": item.title, "path": label, "enabled": item.isEnabled, "key": item.keyEquivalent, "modifiers": item.keyEquivalentModifierMask.rawValue])
                        if let child = item.submenu { visit(child, label) }
                    }
                }
                if let menu = NSApp.mainMenu { visit(menu, "") }
                try! JSONSerialization.data(withJSONObject: rows, options: [.prettyPrinted, .sortedKeys]).write(to: URL(fileURLWithPath: command["path"] as! String))
                try! FileManager.default.removeItem(atPath: path)
                return
            }
            guard let window = NSApp.windows.first(where: { candidate in
                if let number = command["windowNumber"] as? Int { return candidate.windowNumber == number }
                return candidate.isVisible && candidate.contentView != nil && ((command["windowTitle"] as? String).map { candidate.title.contains($0) } ?? candidate.isKeyWindow)
            }) else { return }
            do {
                switch action {
                case "activate":
                    if !NSApp.isActive { NSApp.activate(ignoringOtherApps: true) }
                    if !window.isKeyWindow { window.makeKeyAndOrderFront(nil) }
                case "appearance":
                    window.appearance = NSAppearance(named: command["dark"] as! Bool ? .darkAqua : .aqua)
                case "inactive":
                    window.resignKey(); window.resignMain()
                case "embed":
                    let host = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 640, height: 420), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
                    host.title = "Opaque embedding host"
                    host.isOpaque = true; host.backgroundColor = .blue
                    let content = window.contentView!
                    window.contentView = nil; host.contentView = content
                    window.orderOut(nil); host.makeKeyAndOrderFront(nil); ownedHost = host
                case "inspect":
                    try snapshot(window, command["path"] as! String)
                case "menu":
                    guard let menu = NSApp.mainMenu, let (owner, index) = menuEntry(menu, command["title"] as! String) else { fatalError("Probe menu not found") }
                    owner.performActionForItem(at: index)
                case "close":
                    window.performClose(nil)
                case "resize":
                    window.setContentSize(NSSize(width: command["width"] as! Double, height: command["height"] as! Double))
                case "click":
                    let point = NSPoint(x: command["x"] as! Double, y: window.contentView!.bounds.height - (command["y"] as! Double))
                    for kind in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
                        let event = NSEvent.mouseEvent(with: kind, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: kind == .leftMouseDown ? 1 : 0)!
                        NSApp.sendEvent(event)
                    }
                case "record":
                    let prefix = command["prefix"] as! String
                    try snapshot(window, prefix + "-000.json")
                    recording = true
                    if let title = command["menu"] as? String, let menu = NSApp.mainMenu, let (owner, index) = menuEntry(menu, title) { owner.performActionForItem(at: index) }
                    for step in command["afterActions"] as? [[String: Any]] ?? [] {
                        DispatchQueue.main.asyncAfter(deadline: .now() + (step["delay"] as! Double)) {
                            if let title = step["menu"] as? String, let menu = NSApp.mainMenu, let (owner, index) = menuEntry(menu, title) { owner.performActionForItem(at: index) }
                        }
                    }
                    var frame = 0
                    let samples = command["samples"] as? Int ?? 40
                    let recorder = Timer.scheduledTimer(withTimeInterval: 1.0 / 60.0, repeats: true) { timer in
                        frame += 1
                        try! snapshot(window, prefix + String(format: "-%03d.json", frame))
                        if frame >= samples {
                            timer.invalidate()
                            try! FileManager.default.removeItem(atPath: path)
                            recording = false
                        }
                    }
                    RunLoop.main.add(recorder, forMode: .common)
                    return
                case "mark":
                    let input = window.firstResponder as! NSTextInputClient
                    let text = command["text"] as! String
                    input.setMarkedText(text, selectedRange: NSRange(location: text.utf16.count, length: 0), replacementRange: NSRange(location: NSNotFound, length: 0))
                case "text":
                    guard let input = window.firstResponder as? NSTextInputClient else { fatalError("Probe has no text responder") }
                    input.insertText(command["text"] as! String, replacementRange: NSRange(location: NSNotFound, length: 0))
                case "key":
                    var flags: NSEvent.ModifierFlags = []
                    for flag in command["modifiers"] as? [String] ?? [] {
                        switch flag { case "cmd": flags.insert(.command); case "ctrl": flags.insert(.control); case "shift": flags.insert(.shift); case "alt": flags.insert(.option); default: fatalError("Unknown modifier") }
                    }
                    let chars = command["chars"] as! String
                    let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil, characters: chars, charactersIgnoringModifiers: chars, isARepeat: false, keyCode: UInt16(command["code"] as! Int))!
                    try FileManager.default.removeItem(atPath: path)
                    NSApp.sendEvent(event)
                    return
                default: fatalError("Unknown probe action: \(action)")
                }
                try FileManager.default.removeItem(atPath: path)
            } catch { NSLog("FINAL PROBE ERROR: %@", String(describing: error)) }
        }
        RunLoop.main.add(probeTimer!, forMode: .common)
    }
}
