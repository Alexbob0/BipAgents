import SwiftUI

/// The Hermes style's mascots: a pixel character per category, in the spirit of Hermes Agent's own sprite (Vie wears
/// its winged helmet). 16 × 18 pixels; the agent's colors paint its clothes and accessories, so a custom color shows.
struct PixelMascot: View {
    var appearance: AgentAppearance
    @Environment(\.displayScale) private var displayScale

    var body: some View {
        let sprite = Self.sprite(for: appearance.category)
        let palette = appearance.palette
        let scale = displayScale
        Canvas { ctx, size in
            // Whole device pixels per sprite pixel, so edges stay sharp at any size.
            let fit = min(size.width / CGFloat(Sprite.width), size.height / CGFloat(Sprite.height))
            let cell = max(1, (fit * scale).rounded(.down)) / scale
            let origin = CGPoint(x: ((size.width - cell * CGFloat(Sprite.width)) / 2).rounded(),
                                 y: ((size.height - cell * CGFloat(Sprite.height)) / 2).rounded())
            for (y, row) in sprite.rows.enumerated() {
                for (x, letter) in row.enumerated() where letter != "." {
                    let color: Color = sprite.main.contains(letter) ? palette.main
                        : sprite.deep.contains(letter) ? palette.deep
                        : sprite.colors[letter].map { Color(hex: $0) } ?? .clear
                    ctx.fill(Path(CGRect(x: origin.x + CGFloat(x) * cell, y: origin.y + CGFloat(y) * cell, width: cell, height: cell)),
                             with: .color(color))
                }
            }
        }
        .aspectRatio(CGFloat(Sprite.width) / CGFloat(Sprite.height), contentMode: .fit)
        .accessibilityHidden(true)
    }

    struct Sprite {
        static let width = 16, height = 18
        /// One string per row, one letter per pixel ("." is empty).
        var rows: [[Character]]
        var colors: [Character: UInt32]
        /// Letters painted with the agent's main and deep colors.
        var main: Set<Character>
        var deep: Set<Character>

        init(rows: [String], colors: [Character: UInt32], main: String, deep: String) {
            self.rows = rows.map(Array.init)
            self.colors = colors
            self.main = Set(main)
            self.deep = Set(deep)
        }
    }

    // Generated from the design canvas's pixel sheets (« D · Personnages pixel »).
    static func sprite(for category: AgentCategory) -> Sprite {
        switch category {
        case .daily:
            Sprite(rows: [
            "ak............ka",
            "aa............aa",
            "qa.kkkkkkkkkk.aq",
            "aakaawwaaaaaakaa",
            "aqkaaaaaaaaaakqa",
            "kakaaaaggaaaakak",
            ".kkaaaaaaaaaakk.",
            "..gggggggggggg..",
            "..kskwsssskwsk..",
            "..kskksssskksk..",
            "..kskksssskksk..",
            "..kpsssmmssspk..",
            "...kssssssssk...",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck.."
            ], colors: ["a": 0xE8E6E1, "g": 0xE6B422, "h": 0x141414, "k": 0x1A1A2E, "m": 0x9C4A44, "p": 0xF2A0A0, "q": 0xA9A49A, "s": 0xF4D3B2, "w": 0xFFFFFF], main: "c", deep: "")
        case .wellness:
            Sprite(rows: [
            "................",
            "................",
            "................",
            "....kkkkkkkk..a.",
            "...hhhhhhhhhha..",
            "..kaaaaaaaaaak..",
            "..khhhhhhhhhhk..",
            "..khhshhhhshhk..",
            "..hskwsssskwsh..",
            ".khskksssskkshk.",
            ".khskksssskkshk.",
            ".khpsssmmsssphk.",
            ".khksssssssskhk.",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck.."
            ], colors: ["h": 0x141414, "k": 0x1A1A2E, "m": 0x9C4A44, "p": 0xF2A0A0, "s": 0xF4D3B2, "w": 0xFFFFFF], main: "ca", deep: "")
        case .finance:
            Sprite(rows: [
            "................",
            "................",
            "................",
            "....kkkkkkkk....",
            "...hhhhhhhhhh...",
            "..khehhhhhhhhk..",
            "..khhhhhhhhhhk..",
            "..kffffshffffk..",
            "..kfkwffffkwfk..",
            "..kfkkfssfkkfk..",
            "..kfkkfssfkkfk..",
            "..kffffmmffffk..",
            "...kssssssssk...",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kcckaakcck...",
            "..kccccaacccck..",
            "..kccccaacccck.."
            ], colors: ["a": 0xE6B422, "c": 0x2B2D35, "e": 0x7A5A40, "f": 0xE6B422, "h": 0x3B2A1E, "k": 0x1A1A2E, "m": 0x9C4A44, "p": 0xF2A0A0, "s": 0xF4D3B2, "w": 0xFFFFFF], main: "", deep: "")
        case .work:
            Sprite(rows: [
            "................",
            ".....h...hh.....",
            "....h.h.hh.h....",
            "....kkkkkkkk....",
            "..aaaaaaaaaaaa..",
            "..khhhhhhhhhhk..",
            "..khhhhhhhhhhk..",
            ".aahhhsshshhhaa.",
            ".aaskwsssskwsaa.",
            ".aaskksssskksaa.",
            ".aaskksssskksaa.",
            "..kpsssmmssspk..",
            "...aaassssssk...",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck.."
            ], colors: ["h": 0x141414, "k": 0x1A1A2E, "m": 0x9C4A44, "p": 0xF2A0A0, "s": 0xF4D3B2, "w": 0xFFFFFF], main: "ca", deep: "")
        case .learning:
            Sprite(rows: [
            "......hehh......",
            ".....khhhhk.....",
            "......hhhh......",
            "....kkkkkkkk....",
            "...hhhhhhhhhh...",
            "..khhhhhhhhhhk..",
            "..khhhhhhhhhhk..",
            "..kffffssffffk..",
            "..hfkwffffkwfh..",
            "..hfkkfssfkkfh..",
            "..hfkkfssfkkfh..",
            "..kffffmmffffk..",
            "...kssssssssk...",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck.."
            ], colors: ["e": 0x6E5040, "h": 0x4A3426, "k": 0x1A1A2E, "m": 0x9C4A44, "p": 0xF2A0A0, "s": 0xF4D3B2, "w": 0xFFFFFF], main: "cf", deep: "")
        case .home:
            Sprite(rows: [
            ".......ww.......",
            "....kkkkkkkk....",
            "...aaaaaaaaaa...",
            "..kaaaaaaaaaak..",
            "..kaaaaaaaaaak..",
            "..kaaaaaaaaaak..",
            "..bbababbababb..",
            "..bbbbbbbababb..",
            "..kssssssssssk..",
            "..kskksssskksk..",
            "..kssssssssssk..",
            "..kpsssmmssspk..",
            "...kssssssssk...",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck.."
            ], colors: ["h": 0x141414, "k": 0x1A1A2E, "m": 0x9C4A44, "p": 0xF2A0A0, "s": 0xF4D3B2, "w": 0xFFFFFF], main: "ca", deep: "b")
        case .creative:
            Sprite(rows: [
            "........k.......",
            "........a.......",
            "..aaaaaaaaaaaa..",
            ".aaaaaaaaaaaaa..",
            "...hhhhhhhhhh...",
            "..khhhhhhhhhhk..",
            "..khhhhhhhhhhk..",
            "..khhsssssshhk..",
            "..hskwsssskwsh..",
            ".khskksssskkshk.",
            ".khskksssskkshk.",
            ".khpsssmmsssphk.",
            ".khksssssssskhk.",
            ".kh.kkkkkkkk.hk.",
            ".kh....ss....hk.",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck.."
            ], colors: ["h": 0x141414, "k": 0x1A1A2E, "m": 0x9C4A44, "p": 0xF2A0A0, "s": 0xF4D3B2, "w": 0xFFFFFF], main: "ca", deep: "")
        case .tech:
            Sprite(rows: [
            "................",
            "................",
            "...kkkkkkkkkk...",
            "..kaaaaaaaaaak..",
            "..kaaaaaaaaaak..",
            "..kaaaaaaaaaak..",
            "..kaaaaaaaaaak..",
            ".kassssssssssak.",
            ".kakkkkkkkkkkak.",
            ".kakvwvvvvwvkak.",
            ".kakkkkkkkkkkak.",
            ".kapsssmmssspak.",
            ".kaksssssssskak.",
            ".ka.kkkkkkkk.ak.",
            ".ka....ss....ak.",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck.."
            ], colors: ["a": 0x4A5063, "c": 0x4A5063, "h": 0x141414, "k": 0x1A1A2E, "m": 0x9C4A44, "p": 0xF2A0A0, "s": 0xF4D3B2, "v": 0x5DE2E7, "w": 0xFFFFFF], main: "", deep: "")
        }
    }
}
