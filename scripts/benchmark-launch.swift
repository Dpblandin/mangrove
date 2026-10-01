import AppKit
import CoreGraphics
import Foundation

// Launch the real bundle through LaunchServices, without any UI automation.
// Standard output contains bounded launch metadata, never repository content.
guard CommandLine.arguments.count >= 2 else { exit(2) }
func emit(_ value: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: value),
          let text = String(data: data, encoding: .utf8) else { exit(6) }
    print(text)
    fflush(stdout)
}
func windowMetadata(_ pid: pid_t) -> [String: Any] {
    let windows = CGWindowListCopyWindowInfo(.optionOnScreenOnly, kCGNullWindowID)
        as? [[String: Any]] ?? []
    let rectangles = windows.compactMap { value -> CGRect? in
        guard (value[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid,
              (value[kCGWindowLayer as String] as? NSNumber)?.intValue == 0,
              let bounds = value[kCGWindowBounds as String] as? [String: Any]
        else { return nil }
        return CGRect(dictionaryRepresentation: bounds as CFDictionary)
    }
    guard let rect = rectangles.max(by: { $0.width * $0.height < $1.width * $1.height })
    else { return ["on_main_display": false] }
    let display = CGMainDisplayID()
    let main = CGDisplayBounds(display)
    let mode = CGDisplayCopyDisplayMode(display)
    return ["x": rect.minX, "y": rect.minY, "width": rect.width, "height": rect.height,
            "on_main_display": main.contains(rect),
            "main_display_width": main.width, "main_display_height": main.height,
            "main_display_pixel_width": mode?.pixelWidth ?? 0,
            "main_display_pixel_height": mode?.pixelHeight ?? 0,
            "main_display_refresh_hz": mode?.refreshRate ?? 0]
}
var launched: NSRunningApplication?
let configuration = NSWorkspace.OpenConfiguration()
configuration.activates = true
configuration.createsNewApplicationInstance = true
configuration.addsToRecentItems = false
configuration.promptsUserIfNeeded = false
configuration.environment = ProcessInfo.processInfo.environment
configuration.arguments = Array(CommandLine.arguments.dropFirst(2))
let bundle = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
DispatchQueue.main.asyncAfter(deadline: .now() + 20) {
    // Only the exact new instance returned by LaunchServices is terminated.
    launched?.forceTerminate()
    exit(3)
}
NSWorkspace.shared.openApplication(at: bundle, configuration: configuration) { app, error in
    guard error == nil, let app else { exit(4) }
    // Activation is asynchronous. Keep the LaunchServices caller alive until
    // AppKit reports the launched application active, rather than assuming that
    // returning a running instance guarantees it has reached the foreground.
    DispatchQueue.main.async {
        launched = app
        emit(["pid": app.processIdentifier, "status": "launched"])
        app.activate(options: [.activateAllWindows])
        Timer.scheduledTimer(withTimeInterval: 0.01, repeats: true) { timer in
            if app.isTerminated { exit(5) }
            if app.isActive {
                let window = windowMetadata(app.processIdentifier)
                // LaunchServices can report activation before WindowServer has
                // registered the app's first visible window.
                guard let width = window["width"] as? CGFloat, width >= 800,
                      let height = window["height"] as? CGFloat, height >= 500,
                      window["on_main_display"] as? Bool == true else { return }
                timer.invalidate()
                emit(["pid": app.processIdentifier, "status": "active",
                      "window": window])
                exit(0)
            }
        }
    }
}
RunLoop.main.run()
