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
    color(0x101b18).setFill(); NSRect(x: 0, y: 0, width: 1920, height: 1080).fill()
    color(0xc99870).setFill(); NSRect(x: 112, y: 830, width: 76, height: 6).fill()
    text("MOBARUST  /  DESKTOP WALKTHROUGH", x: 112, y: 862, size: 23, ink: 0xa8b9af, bold: true)
    text("Servers. Shells. Files.", x: 104, y: 625, size: 112, ink: 0xf2f0e8, bold: true)
    text("One place to work.", x: 104, y: 491, size: 112, ink: 0xc99870, bold: true)
    text("Real native app. Disposable SSH, SFTP and tunnel fixtures.", x: 112, y: 368, size: 34, ink: 0xa8b9af)
    text("NATIVE TERMINALS     /     SSH + FILES     /     SERVICE TUNNELS", x: 112, y: 155, size: 23, ink: 0x98bca2, bold: true)
    text("MobaRust 0.1.17  ·  macOS", x: 112, y: 102, size: 22, ink: 0xa8b9af)
}
let chapters: [(String, String, String)] = [
    ("terminal", "01 / Your shell, without the clutter.", "Real native PTY · macOS ARM64"),
    ("split", "02 / Keep two tasks in view.", "Independent split panes"),
    ("connect", "03 / A real SSH session.", "Disposable loopback server"),
    ("files", "04 / Browse, edit and transfer.", "Real SFTP · generated files"),
    ("tunnel", "05 / Bring a service closer.", "SSH local forward · HTTP health"),
    ("tools", "06 / Review a reusable command.", "Snippets · explicit execution"),
    ("dark", "07 / Choose your working light.", "Dark and light themes")
]
for (name, title, detail) in chapters {
    try canvas(name, height: 104) {
        color(0x101b18).setFill()
        NSRect(x: 0, y: 0, width: 1920, height: 104).fill()
        text(title, x: 64, y: 40, size: 32, ink: 0xf2f0e8, bold: true)
        text(detail, x: 1190, y: 44, size: 22, ink: 0xa8b9af)
        color(0xc99870).setFill(); NSRect(x: 64, y: 15, width: 80, height: 3).fill()
    }
}
try canvas("outro") {
    color(0x101b18).setFill(); NSRect(x: 0, y: 0, width: 1920, height: 1080).fill()
    text("MobaRust", x: 112, y: 650, size: 128, ink: 0xf2f0e8, bold: true)
    text("Your next workspace.", x: 112, y: 510, size: 66, ink: 0xc99870, bold: true)
    text("Free. Open source. Local-first.", x: 112, y: 392, size: 38, ink: 0xa8b9af)
    text("DOWNLOAD FOR MAC, WINDOWS & LINUX", x: 112, y: 245, size: 25, ink: 0x98bca2, bold: true)
    text("othmaneblial.github.io/MobaRust", x: 112, y: 184, size: 34, ink: 0xf2f0e8)
    text("Mac v0.1.17 preview · Windows/Linux v0.1.12 · no publisher signing · RDP excluded", x: 112, y: 80, size: 22, ink: 0xa8b9af)
}
