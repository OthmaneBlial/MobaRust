// Accessibility-only control of an explicit native MobaRust demo process.
// Never points at a browser or an arbitrary active application.
import ApplicationServices
import Foundation

let args = CommandLine.arguments
guard args.count >= 4, let pid = Int32(args[1]) else { fatalError("Usage: demo-control PID press|type|set LABEL [VALUE]") }
let app = AXUIElementCreateApplication(pid)
func value(_ element: AXUIElement, _ key: String) -> Any? {
    var result: CFTypeRef?
    AXUIElementCopyAttributeValue(element, key as CFString, &result)
    return result
}
func find(_ element: AXUIElement, label: String, depth: Int = 0) -> AXUIElement? {
    if depth > 24 { return nil }
    let title = value(element, kAXTitleAttribute) as? String ?? ""
    let description = value(element, kAXDescriptionAttribute) as? String ?? ""
    if title.caseInsensitiveCompare(label) == .orderedSame || description.caseInsensitiveCompare(label) == .orderedSame { return element }
    let children = value(element, kAXChildrenAttribute) as? [AXUIElement] ?? []
    for child in label == "Terminal input" ? children.reversed().map({$0}) : children {
        if let match = find(child, label: label, depth: depth + 1) { return match }
    }
    return nil
}
guard let target = find(app, label: args[3]) else { fatalError("Demo control not found: \(args[3])") }
switch args[2] {
case "press":
    let result = AXUIElementPerformAction(target, kAXPressAction as CFString)
    guard result == .success else { fatalError("AXPress failed: \(result)") }
case "set":
    guard args.count == 5 else { fatalError("Missing value") }
    let result = AXUIElementSetAttributeValue(target, kAXValueAttribute as CFString, args[4] as CFString)
    guard result == .success else { fatalError("AXValue failed: \(result)") }
case "type":
    guard args.count == 5 else { fatalError("Missing text") }
    AXUIElementSetAttributeValue(target, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    for character in args[4] {
        let units = Array(String(character).utf16)
        for down in [true, false] {
            let event = CGEvent(keyboardEventSource: nil, virtualKey: character == "\n" ? 36 : 0, keyDown: down)!
            if character != "\n" { event.keyboardSetUnicodeString(stringLength: units.count, unicodeString: units) }
            event.postToPid(pid)
        }
        Thread.sleep(forTimeInterval: 0.04)
    }
default: fatalError("Unknown action")
}
