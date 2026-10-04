import SwiftUI

/// Builds a `Path` from SVG path data (M L H V C S Q T Z, absolute and relative).
/// Enough for the mascot artwork, which is authored as SVG in the design mockups.
func SVGPath(_ data: String) -> Path {
    var path = Path()
    var scanner = SVGTokenScanner(data)
    var current = CGPoint.zero, start = CGPoint.zero
    var lastControl: CGPoint?, lastCommand: Character = " "

    while let command = scanner.nextCommand(defaultingTo: lastCommand) {
        let relative = command.isLowercase
        func point(_ x: Double, _ y: Double) -> CGPoint {
            relative ? CGPoint(x: current.x + x, y: current.y + y) : CGPoint(x: x, y: y)
        }
        var control: CGPoint?

        switch command.uppercased().first! {
        case "M":
            guard let x = scanner.number(), let y = scanner.number() else { return path }
            current = point(x, y); start = current
            path.move(to: current)
            // Extra coordinate pairs after M are implicit L commands.
            lastCommand = relative ? "l" : "L"
            continue
        case "L":
            guard let x = scanner.number(), let y = scanner.number() else { return path }
            current = point(x, y); path.addLine(to: current)
        case "H":
            guard let x = scanner.number() else { return path }
            current = CGPoint(x: relative ? current.x + x : x, y: current.y); path.addLine(to: current)
        case "V":
            guard let y = scanner.number() else { return path }
            current = CGPoint(x: current.x, y: relative ? current.y + y : y); path.addLine(to: current)
        case "C":
            guard let x1 = scanner.number(), let y1 = scanner.number(), let x2 = scanner.number(),
                  let y2 = scanner.number(), let x = scanner.number(), let y = scanner.number() else { return path }
            let c1 = point(x1, y1), c2 = point(x2, y2), end = point(x, y)
            path.addCurve(to: end, control1: c1, control2: c2)
            current = end; control = c2
        case "S":
            guard let x2 = scanner.number(), let y2 = scanner.number(), let x = scanner.number(), let y = scanner.number() else { return path }
            let c1 = "CcSs".contains(lastCommand) ? reflect(lastControl, around: current) : current
            let c2 = point(x2, y2), end = point(x, y)
            path.addCurve(to: end, control1: c1, control2: c2)
            current = end; control = c2
        case "Q":
            guard let x1 = scanner.number(), let y1 = scanner.number(), let x = scanner.number(), let y = scanner.number() else { return path }
            let c = point(x1, y1), end = point(x, y)
            path.addQuadCurve(to: end, control: c)
            current = end; control = c
        case "T":
            guard let x = scanner.number(), let y = scanner.number() else { return path }
            let c = "QqTt".contains(lastCommand) ? reflect(lastControl, around: current) : current
            let end = point(x, y)
            path.addQuadCurve(to: end, control: c)
            current = end; control = c
        case "Z":
            path.closeSubpath(); current = start
        default:
            return path
        }
        lastControl = control
        lastCommand = command
    }
    return path
}

private func reflect(_ point: CGPoint?, around center: CGPoint) -> CGPoint {
    guard let point else { return center }
    return CGPoint(x: 2 * center.x - point.x, y: 2 * center.y - point.y)
}

private struct SVGTokenScanner {
    private let chars: [Character]
    private var index = 0

    init(_ string: String) { chars = Array(string) }

    private mutating func skipSeparators() {
        while index < chars.count, chars[index] == " " || chars[index] == "," || chars[index].isNewline { index += 1 }
    }

    /// Returns the next explicit command letter, or repeats the previous one when numbers follow.
    mutating func nextCommand(defaultingTo previous: Character) -> Character? {
        skipSeparators()
        guard index < chars.count else { return nil }
        let c = chars[index]
        if c.isLetter { index += 1; return c }
        return previous == " " || previous == "Z" || previous == "z" ? nil : previous
    }

    mutating func number() -> Double? {
        skipSeparators()
        let begin = index
        var seenDot = false, seenExp = false
        while index < chars.count {
            let c = chars[index]
            if c.isNumber {
                index += 1
            } else if (c == "-" || c == "+") && (index == begin || chars[index - 1] == "e" || chars[index - 1] == "E") {
                index += 1
            } else if c == "." && !seenDot && !seenExp {
                seenDot = true; index += 1
            } else if (c == "e" || c == "E") && !seenExp && index > begin {
                seenExp = true; index += 1
            } else {
                break
            }
        }
        guard index > begin else { return nil }
        return Double(String(chars[begin..<index]))
    }
}
