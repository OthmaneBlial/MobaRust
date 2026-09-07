// Generate original typography overlays for the native desktop recording.
// Usage: swift tools/demo-titles.swift <output-directory>
import AppKit
import Foundation

let directory = URL(fileURLWithPath: CommandLine.arguments[1])
try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
func color(_ hex: UInt32) -> NSColor {
    NSColor(srgbRed: CGFloat((hex >> 16) & 255) / 255,
            green: CGFloat((hex >> 8) & 255) / 255,
            blue: CGFloat(hex & 255) / 255, alpha: 1)
}
func text(_ value: String, x: CGFloat, y: CGFloat, size: CGFloat, ink: UInt32, bold: Bool = false) {
    let font = NSFont(name: bold ? "AvenirNext-DemiBold" : "AvenirNext-Regular", size: size)
        ?? NSFont.systemFont(ofSize: size)
    (value as NSString).draw(at: NSPoint(x: x, y: y), withAttributes: [.font: font, .foregroundColor: color(ink)])
}
func canvas(_ name: String, height: Int = 1080, draw: () -> Void) throws {
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1920, pixelsHigh: height,
        bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
        colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: bitmap)
    draw()
    NSGraphicsContext.restoreGraphicsState()
    try bitmap.representation(using: .png, properties: [:])!.write(to: directory.appendingPathComponent(name + ".png"))
}
try canvas("intro") {
    color(0xf3f5ef).setFill(); NSRect(x: 0, y: 0, width: 1920, height: 1080).fill()
    color(0x946015).setFill(); NSRect(x: 112, y: 830, width: 76, height: 6).fill()
    text("MOBARUST  /  DESKTOP WALKTHROUGH", x: 112, y: 862, size: 23, ink: 0x62756a, bold: true)
    text("Your work.", x: 104, y: 625, size: 112, ink: 0x1e2c25, bold: true)
    text("In a clearer workspace.", x: 104, y: 491, size: 112, ink: 0x946015, bold: true)
    text("Real desktop capture. Light first. Dark when you want it.", x: 112, y: 368, size: 34, ink: 0x5f7167)
    text("NATIVE TERMINALS     /     QUICK CONNECT     /     ONE-CLICK THEMES", x: 112, y: 155, size: 23, ink: 0x39744b, bold: true)
    text("MobaRust 0.1.1  ·  macOS", x: 112, y: 102, size: 22, ink: 0x62756a)
}
let chapters: [(String, String, String)] = [
    ("terminal", "01 / A real shell. A clear view.", "Native PTY · light mode"),
    ("split", "02 / Keep work side by side.", "Independent terminal panes"),
    ("connect", "03 / Connections, made explicit.", "SSH setup · no credentials shown"),
    ("tools", "04 / Review before you run.", "Reusable command snippets"),
    ("dark", "05 / Same workspace. Different light.", "One click · terminal colors included")
]
for (name, title, detail) in chapters {
    try canvas(name, height: 104) {
        color(name == "dark" ? 0x101615 : 0xf3f5ef).setFill()
        NSRect(x: 0, y: 0, width: 1920, height: 104).fill()
        text(title, x: 64, y: 40, size: 32, ink: name == "dark" ? 0xdce8dc : 0x1e2c25, bold: true)
        text(detail, x: 1190, y: 44, size: 22, ink: name == "dark" ? 0x9bc48a : 0x62756a)
        color(0x946015).setFill(); NSRect(x: 64, y: 15, width: 80, height: 3).fill()
    }
}
try canvas("outro") {
    color(0xf3f5ef).setFill(); NSRect(x: 0, y: 0, width: 1920, height: 1080).fill()
    text("MobaRust", x: 112, y: 650, size: 128, ink: 0x1e2c25, bold: true)
    text("Make yourself at home.", x: 112, y: 510, size: 66, ink: 0x946015, bold: true)
    text("Free. Open source. Local-first.", x: 112, y: 392, size: 38, ink: 0x5f7167)
    text("DOWNLOAD FOR MAC, WINDOWS & LINUX", x: 112, y: 245, size: 25, ink: 0x39744b, bold: true)
    text("othmaneblial.github.io/MobaRust", x: 112, y: 184, size: 34, ink: 0x1e2c25)
    text("Desktop preview · macOS not notarized · RDP excluded from installers", x: 112, y: 80, size: 22, ink: 0x62756a)
}
