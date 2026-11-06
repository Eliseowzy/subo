// Windows 安装包（NSIS）欢迎页、完成页左侧的竖图：深色底上画反白标志和字标。
//   swift scripts/installer-art.swift desktop/windows/sidebar.bmp
// 几何与颜色照 docs/brand/README.md：120 网格，斜杠 (36,30)→(62,88) 线宽 12 圆头，点 (82,86) 半径 8；
// 深底上斜杠 #f7f8f7、点 #6fcf9f。NSIS 要 BMP，按 2 倍画（328×628），它会缩到 164×314 的控件里。
import AppKit

let out = CommandLine.arguments.dropFirst().first ?? "sidebar.bmp"
let (w, h) = (328, 628)

func rgb(_ hex: UInt32) -> NSColor {
  NSColor(srgbRed: CGFloat((hex >> 16) & 0xff) / 255, green: CGFloat((hex >> 8) & 0xff) / 255,
           blue: CGFloat(hex & 0xff) / 255, alpha: 1)
}

let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: w, pixelsHigh: h, bitsPerSample: 8,
                           samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                           colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let cg = NSGraphicsContext.current!.cgContext

// 底色：应用页头的深绿
rgb(0x1f3a33).setFill()
cg.fill(CGRect(x: 0, y: 0, width: w, height: h))

// 标志：120 网格缩放到 168 宽，放在上部居中。CoreGraphics 的 y 轴朝上，网格坐标翻过来
let size: CGFloat = 168
let s = size / 120
let ox = (CGFloat(w) - size) / 2
let oy = CGFloat(h) - 70 - size
func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint { CGPoint(x: ox + x * s, y: oy + (120 - y) * s) }

cg.setLineCap(.round)
cg.setLineWidth(12 * s)
cg.setStrokeColor(rgb(0xf7f8f7).cgColor)
cg.move(to: p(36, 30))
cg.addLine(to: p(62, 88))
cg.strokePath()
cg.setFillColor(rgb(0x6fcf9f).cgColor)
let dot = p(82, 86)
cg.fillEllipse(in: CGRect(x: dot.x - 8 * s, y: dot.y - 8 * s, width: 16 * s, height: 16 * s))

// 字标：素帛 / Subo，标志下方居中
func text(_ str: String, _ font: NSFont, _ color: NSColor, top: CGFloat, kern: CGFloat = 0) {
  let attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color, .kern: kern]
  let a = NSAttributedString(string: str, attributes: attrs)
  let size = a.size()
  a.draw(at: CGPoint(x: (CGFloat(w) - size.width) / 2, y: CGFloat(h) - top - size.height))
}
let han = NSFont(name: "PingFangSC-Semibold", size: 40) ?? NSFont.boldSystemFont(ofSize: 40)
text("素帛", han, rgb(0xeef4f1), top: 70 + size + 28, kern: 6)
text("Subo", NSFont.systemFont(ofSize: 22, weight: .medium), rgb(0x9dc0b4), top: 70 + size + 90, kern: 1)

NSGraphicsContext.restoreGraphicsState()
// 写 24 位 BMP（NSIS 对 24 位最稳）：行自下而上，BGR，每行补齐到 4 字节
let rowBytes = (w * 3 + 3) & ~3
var bmp = Data()
func le<T: FixedWidthInteger>(_ v: T) { withUnsafeBytes(of: v.littleEndian) { bmp.append(contentsOf: $0) } }
bmp.append(contentsOf: [0x42, 0x4d])
le(UInt32(54 + rowBytes * h)); le(UInt32(0)); le(UInt32(54))
le(UInt32(40)); le(Int32(w)); le(Int32(h)); le(UInt16(1)); le(UInt16(24)); le(UInt32(0))
le(UInt32(rowBytes * h)); le(Int32(2835)); le(Int32(2835)); le(UInt32(0)); le(UInt32(0))
let px = rep.bitmapData!
for y in (0..<h).reversed() {
  var row = [UInt8](repeating: 0, count: rowBytes)
  for x in 0..<w {
    let i = y * rep.bytesPerRow + x * 4
    row[x * 3] = px[i + 2]; row[x * 3 + 1] = px[i + 1]; row[x * 3 + 2] = px[i]
  }
  bmp.append(contentsOf: row)
}
try! bmp.write(to: URL(fileURLWithPath: out))
print("写好 \(out)（\(w)×\(h)）")
