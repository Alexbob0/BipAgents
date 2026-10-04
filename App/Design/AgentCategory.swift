import SwiftUI

/// What an agent is for. Drives its mascot (body shape + accessory) and its colors.
enum AgentCategory: String, Codable, CaseIterable, Identifiable, Sendable {
    case wellness, finance, daily, work, learning, home, creative, tech

    var id: String { rawValue }

    var label: LocalizedStringKey {
        switch self {
        case .wellness: "Bien-être"
        case .finance: "Finance"
        case .daily: "Quotidien"
        case .work: "Travail"
        case .learning: "Apprendre"
        case .home: "Maison"
        case .creative: "Créatif"
        case .tech: "Tech"
        }
    }

    /// Main body color, a deeper shade for text/icons on tints, and a pale tint for surfaces.
    var palette: AgentPalette {
        switch self {
        case .wellness: AgentPalette(main: 0x3CC37A, deep: 0x1D8A4F, tint: 0xE2F5EA)
        case .finance: AgentPalette(main: 0xF6B826, deep: 0x9A6A00, tint: 0xFFF3D1)
        case .daily: AgentPalette(main: 0xFF7B5C, deep: 0xC9472A, tint: 0xFFE8E0)
        case .work: AgentPalette(main: 0x4F7CFF, deep: 0x2B55D4, tint: 0xE4EBFF)
        case .learning: AgentPalette(main: 0x8E6CF5, deep: 0x6142D2, tint: 0xEDE7FF)
        case .home: AgentPalette(main: 0x1FB2C1, deep: 0x0D7F8B, tint: 0xDDF4F6)
        case .creative: AgentPalette(main: 0xF2649E, deep: 0xC23774, tint: 0xFDE4EE)
        case .tech: AgentPalette(main: 0x6B7489, deep: 0x3F4659, tint: 0xE7E9EE)
        }
    }

    var bodyShape: MascotBody {
        switch self {
        case .wellness, .creative: .blob
        case .finance, .work, .tech: .squircle
        case .daily, .home: .egg
        case .learning: .cloud
        }
    }

    var accessory: MascotAccessory {
        switch self {
        case .wellness: .sprout
        case .finance: .coin
        case .daily: .ears
        case .work, .tech: .antenna
        case .learning: .tuft
        case .home: .roof
        case .creative: .star
        }
    }

    private var keywords: [String] {
        switch self {
        case .wellness: ["well", "santé", "sante", "health", "sommeil", "sleep", "sport", "fitness", "bien-être", "bien etre", "médit", "nutrition", "coach"]
        case .finance: ["financ", "budget", "banque", "bank", "argent", "money", "invest", "compta", "impôt", "impot", "tax"]
        case .daily: ["vie", "life", "perso", "agenda", "quotidien", "organis", "assistant", "planning", "famille"]
        case .work: ["travail", "work", "pro", "bureau", "client", "projet", "business", "mail"]
        case .learning: ["appren", "learn", "étude", "etude", "langue", "cours", "school", "prof", "tutor"]
        case .home: ["maison", "home", "domot", "jardin", "cuisine", "courses", "bricol"]
        case .creative: ["créa", "crea", "écri", "ecri", "write", "musique", "music", "design", "photo", "art"]
        case .tech: ["tech", "dev", "code", "serveur", "server", "infra", "ops", "admin", "linux"]
        }
    }

    /// Best guess from an agent's name and description; `.daily` when nothing matches.
    static func suggest(name: String, description: String = "") -> AgentCategory {
        let text = (name + " " + description).lowercased()
        let scored = allCases.map { category in
            (category, category.keywords.filter { text.contains($0) }.count)
        }
        guard let best = scored.max(by: { $0.1 < $1.1 }), best.1 > 0 else { return .daily }
        return best.0
    }
}

struct AgentPalette: Sendable, Equatable {
    var main: Color
    var deep: Color
    var tint: Color

    init(main: UInt32, deep: UInt32, tint: UInt32) {
        self.main = Color(hex: main)
        self.deep = Color(hex: deep)
        // Tints are pale surfaces: in dark mode use a low-opacity wash of the main color instead.
        self.tint = Color(light: tint, dark: Self.darkTint(main))
    }

    init(main: Color, deep: Color, tint: Color) {
        self.main = main
        self.deep = deep
        self.tint = tint
    }

    /// Palette derived from a user-picked color.
    init(custom hex: UInt32) {
        let c = Color(hex: hex)
        let resolved = UIColor(c)
        var h: CGFloat = 0, s: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        resolved.getHue(&h, saturation: &s, brightness: &b, alpha: &a)
        main = c
        deep = Color(hue: h, saturation: min(1, s * 1.05), brightness: b * 0.62)
        let light = UIColor(hue: h, saturation: s * 0.16, brightness: 0.98, alpha: 1)
        let dark = UIColor(hue: h, saturation: s * 0.45, brightness: 0.22, alpha: 1)
        tint = Color(uiColor: .dynamic(light: light, dark: dark))
    }

    private static func darkTint(_ main: UInt32) -> UInt32 {
        // Mix 18% of the main color into the dark card color 0x1A1D24.
        func mix(_ shift: UInt32) -> UInt32 {
            let m = Double((main >> shift) & 0xFF), base = Double((0x1A1D24 >> shift) & 0xFF)
            return UInt32(base + (m - base) * 0.18) << shift
        }
        return mix(16) | mix(8) | mix(0)
    }
}

/// How an agent looks: a category, optionally overridden by a custom color.
struct AgentAppearance: Codable, Hashable, Sendable {
    var category: AgentCategory
    var customColorHex: UInt32?

    var palette: AgentPalette {
        customColorHex.map(AgentPalette.init(custom:)) ?? category.palette
    }

    static let customChoices: [UInt32] = [0x3CC37A, 0xF6B826, 0xFF7B5C, 0x4F7CFF, 0x8E6CF5, 0x1FB2C1, 0xF2649E]
}
