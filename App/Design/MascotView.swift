import SwiftUI

enum MascotBody: Sendable { case blob, squircle, egg, cloud }
enum MascotAccessory: Sendable { case sprout, coin, ears, antenna, tuft, star, roof }

enum MascotMood: Sendable, CaseIterable {
    case happy, listening, thinking, speaking, asking, sleeping
}

/// An agent's mascot, drawn in a 120×120 design space (same geometry as the design mockups).
struct MascotView: View {
    var appearance: AgentAppearance
    var mood: MascotMood = .happy
    var animated = true
    /// 0…1 mouth opening driven by playback level while speaking; nil = automatic chatter.
    var speakingLevel: Double?

    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        TimelineView(.animation(paused: !animated || reduceMotion)) { timeline in
            Canvas { ctx, size in
                let t = animated && !reduceMotion ? timeline.date.timeIntervalSinceReferenceDate : 0
                MascotRenderer(appearance: appearance, mood: mood, time: t, speakingLevel: speakingLevel)
                    .draw(in: &ctx, size: size)
            }
        }
        .aspectRatio(1, contentMode: .fit)
        .accessibilityHidden(true)
    }
}

private struct MascotRenderer {
    let appearance: AgentAppearance
    let mood: MascotMood
    let time: TimeInterval
    let speakingLevel: Double?

    private let inkColor = Color(hex: 0x16181D)
    private var palette: AgentPalette { appearance.palette }
    private var category: AgentCategory { appearance.category }

    func draw(in ctx: inout GraphicsContext, size: CGSize) {
        let scale = min(size.width, size.height) / 120
        ctx.scaleBy(x: scale, y: scale)

        ctx.fill(ellipse(60, 113, 32, 4.5), with: .color(inkColor.opacity(0.1)))

        // Feet stay on the ground while the body bobs above them.
        for x in [45.0, 75.0] {
            ctx.fill(ellipse(x, 106, 10.5, 6.5), with: .color(palette.deep))
            ctx.fill(ellipse(x - 3, 104.5, 3.5, 1.8), with: .color(.white.opacity(0.25)))
        }

        let bobPeriod = mood == .speaking || mood == .listening ? 0.9 : 3.0
        ctx.translateBy(x: 0, y: -2 - 2 * sin(time * 2 * .pi / bobPeriod))

        drawAccessory(in: &ctx, front: false)
        ctx.fill(bodyPath, with: .color(palette.main))
        drawAccessory(in: &ctx, front: true)

        var highlight = ctx
        highlight.translateBy(x: 43, y: 44)
        highlight.rotate(by: .degrees(-30))
        highlight.fill(ellipse(0, 0, 12, 7), with: .color(.white.opacity(0.32)))

        let cheek = Color(hex: 0xFF6F91).opacity(0.5)
        ctx.fill(ellipse(37, 76, 6.5, 4), with: .color(cheek))
        ctx.fill(ellipse(83, 76, 6.5, 4), with: .color(cheek))

        drawFace(in: &ctx)
    }

    // MARK: Body & accessories

    private var bodyPath: Path {
        switch category.bodyShape {
        case .blob: SVGPath("M60 24c25 0 42 17 42 43 0 25-16 39-42 39S18 92 18 67c0-26 17-43 42-43z")
        case .squircle: Path(roundedRect: CGRect(x: 19, y: 28, width: 82, height: 78), cornerRadius: 32, style: .continuous)
        case .egg: SVGPath("M60 20c21 0 38 28 38 52 0 21-16 34-38 34S22 93 22 72c0-24 17-52 38-52z")
        case .cloud: SVGPath("M38 46c2-14 12-22 24-22 13 0 23 9 24 22 10 2 17 11 17 22 0 22-18 38-43 38S17 90 17 68c0-12 8-21 21-22z")
        }
    }

    private func drawAccessory(in ctx: inout GraphicsContext, front: Bool) {
        let main = palette.main, deep = palette.deep
        let gold = Color(hex: 0xFFD84D)
        switch (category.accessory, front) {
        case (.sprout, false):
            ctx.stroke(SVGPath("M60 27c0-7 1-13 3-17"), with: .color(deep), style: round(4))
            ctx.fill(SVGPath("M63 12c5-8 15-9 21-4-5 7-14 9-21 4z"), with: .color(deep))
            let leaf = SVGPath("M61 15c-5-7-14-8-19-3 5 6 13 8 19 3z")
            ctx.fill(leaf, with: .color(main))
            ctx.stroke(leaf, with: .color(deep), lineWidth: 2)
        case (.coin, false):
            var coin = ctx
            let phase = sin(time * 2 * .pi / 2.4)
            coin.translateBy(x: 60, y: 12 - 1.5 - 1.5 * phase)
            coin.rotate(by: .degrees(6 + 6 * phase))
            coin.fill(ellipse(0, 0, 9, 9), with: .color(gold))
            coin.stroke(ellipse(0, 0, 9, 9), with: .color(deep), lineWidth: 3)
            coin.stroke(SVGPath("M0 -4.5v9"), with: .color(deep), style: round(2.6))
            ctx.stroke(SVGPath("M42 10l-4-3M78 10l4-3"), with: .color(deep), style: round(2.5))
        case (.ears, false):
            for x in [35.0, 85.0] {
                ctx.fill(ellipse(x, 36, 12, 12), with: .color(main))
                ctx.fill(ellipse(x, 36, 5.5, 5.5), with: .color(Color(hex: 0xFFB3C1)))
            }
        case (.antenna, false):
            ctx.stroke(SVGPath("M60 30V13"), with: .color(deep), style: round(3.5))
            let glow = 0.775 + 0.225 * cos(time * 2 * .pi / 1.6)
            ctx.fill(ellipse(60, 10, 6.5, 6.5), with: .color(main.opacity(glow)))
            ctx.stroke(ellipse(60, 10, 6.5, 6.5), with: .color(deep), lineWidth: 3)
        case (.tuft, false):
            ctx.fill(SVGPath("M50 28c-2-8 2-14 8-17-1 6 2 9 4 12 1-6 5-10 11-11-3 5-3 10-2 16z"), with: .color(deep))
        case (.star, true):
            let star = SVGPath("M60 3l3.6 7.4 8.1 1.1-5.9 5.6 1.5 8-7.3-3.9-7.3 3.9 1.5-8-5.9-5.6 8.1-1.1z")
            ctx.fill(star, with: .color(gold))
            ctx.stroke(star, with: .color(deep), style: StrokeStyle(lineWidth: 2.2, lineJoin: .round))
        case (.roof, true):
            ctx.stroke(SVGPath("M33 35L60 11l27 24"), with: .color(deep),
                       style: StrokeStyle(lineWidth: 6, lineCap: .round, lineJoin: .round))
        default:
            break
        }
    }

    // MARK: Face

    private var blinkScale: CGFloat {
        // Offset per category so several mascots on screen don't blink in sync.
        let offset = Double(AgentCategory.allCases.firstIndex(of: category) ?? 0) * 1.37
        let phase = (time + offset).truncatingRemainder(dividingBy: 4.6) / 4.6
        guard phase > 0.93 else { return 1 }
        let local = (phase - 0.93) / 0.07 // 0…1
        return CGFloat(max(0.1, abs(local * 2 - 1)))
    }

    private func drawEyes(in ctx: inout GraphicsContext, rx: CGFloat, ry: CGFloat, y: CGFloat, xs: (CGFloat, CGFloat), glint: CGFloat, blink: Bool = true) {
        let s = blink ? blinkScale : 1
        for x in [xs.0, xs.1] {
            ctx.fill(ellipse(x, y, rx, ry * s), with: .color(inkColor))
            if s > 0.6 {
                ctx.fill(ellipse(x + rx * 0.38, y - ry * 0.4, glint, glint), with: .color(.white))
            }
        }
    }

    private func drawFace(in ctx: inout GraphicsContext) {
        let line = StrokeStyle(lineWidth: 3, lineCap: .round)
        switch mood {
        case .happy:
            drawEyes(in: &ctx, rx: 5.5, ry: 7.5, y: 64, xs: (47, 73), glint: 2)
            ctx.stroke(SVGPath("M53 78q7 6 14 0"), with: .color(inkColor), style: line)
        case .listening:
            drawEyes(in: &ctx, rx: 7, ry: 9, y: 63, xs: (46, 74), glint: 2.6)
            ctx.fill(ellipse(60, 81, 3.6, 4.2), with: .color(inkColor))
        case .thinking:
            drawEyes(in: &ctx, rx: 5, ry: 6.5, y: 61, xs: (49, 75), glint: 1.8, blink: false)
            ctx.stroke(SVGPath("M54 80h12"), with: .color(inkColor), style: line)
            let pulse = 0.25 + 0.2 * (1 + sin(time * 2 * .pi / 1.4))
            for (x, y, r) in [(93.0, 30.0, 3.0), (101, 21, 4.4), (110, 10, 6)] {
                ctx.fill(ellipse(x, y, r, r), with: .color(palette.deep.opacity(pulse)))
            }
        case .speaking:
            drawEyes(in: &ctx, rx: 5.5, ry: 7.5, y: 64, xs: (47, 73), glint: 2)
            let open = speakingLevel.map { 0.3 + 0.7 * min(1, max(0, $0)) }
                ?? 0.3 + 0.7 * (0.5 + 0.5 * sin(time * 2 * .pi / 0.56))
            var mouth = ctx
            mouth.translateBy(x: 60, y: 81)
            mouth.scaleBy(x: 1, y: open)
            mouth.fill(ellipse(0, -1, 8, 7), with: .color(Color(hex: 0x2B1A1F)))
            mouth.fill(ellipse(0, 3, 5, 2.6), with: .color(Color(hex: 0xFF7A93)))
        case .asking:
            drawEyes(in: &ctx, rx: 5.5, ry: 7.5, y: 64, xs: (47, 73), glint: 2)
            ctx.stroke(SVGPath("M39 54l11-4M81 54l-11-4"), with: .color(inkColor), style: line)
            ctx.stroke(SVGPath("M53 81q3.5-3 7 0t7 0"), with: .color(inkColor), style: line)
        case .sleeping:
            ctx.stroke(SVGPath("M41 65q6 5 12 0M67 65q6 5 12 0"), with: .color(inkColor), style: line)
            ctx.fill(ellipse(60, 80, 3, 2.2), with: .color(inkColor))
        }
    }

    // MARK: Helpers

    private func ellipse(_ cx: CGFloat, _ cy: CGFloat, _ rx: CGFloat, _ ry: CGFloat) -> Path {
        Path(ellipseIn: CGRect(x: cx - rx, y: cy - ry, width: rx * 2, height: ry * 2))
    }

    private func round(_ width: CGFloat) -> StrokeStyle {
        StrokeStyle(lineWidth: width, lineCap: .round, lineJoin: .round)
    }
}

#Preview("Toutes les mascottes") {
    ScrollView {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 110))], spacing: 16) {
            ForEach(AgentCategory.allCases) { category in
                VStack {
                    MascotView(appearance: AgentAppearance(category: category))
                        .frame(width: 90, height: 90)
                    Text(category.label).font(Theme.body(13, weight: .heavy))
                }
                .padding(12)
                .background(category.palette.tint, in: .rect(cornerRadius: 20))
            }
            ForEach(MascotMood.allCases, id: \.self) { mood in
                MascotView(appearance: AgentAppearance(category: .daily), mood: mood)
                    .frame(width: 90, height: 90)
            }
        }
        .padding()
    }
}
