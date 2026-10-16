import SwiftUI
import UniformTypeIdentifiers
import UIKit

/// The saved unsent text of a conversation (kept across leaving it); also how a forwarded message reaches the
/// composer of another agent's conversation.
enum ConversationDraft {
    static func key(agentID: UUID, sessionID: String?) -> String { "draft.\(agentID.uuidString).\(sessionID ?? "new")" }
}

extension View {
    /// Long press on a message, as in messaging apps: copy, share, select a passage, forward to another agent.
    func messageActions(_ text: String, in agent: AgentProfile) -> some View {
        modifier(MessageActions(raw: text, agent: agent))
    }
}

private struct MessageActions: ViewModifier {
    var raw: String
    var agent: AgentProfile

    @Environment(AgentStore.self) private var store
    @Environment(Router.self) private var router
    @State private var isSelecting = false

    /// What the reader sees: no markdown marks, no MEDIA lines.
    private var plain: String { ChatText.plain(raw) }

    func body(content: Content) -> some View {
        content
            .contextMenu {
                Button("Copier", systemImage: "doc.on.doc") { ShareText.copy(raw) }
                ShareLink(item: ShareText.whatsapp(raw)) { Label("Partager…", systemImage: "square.and.arrow.up") }
                Button("Sélectionner du texte", systemImage: "character.textbox") { isSelecting = true }
                let others = store.agents.filter { $0.id != agent.id }
                if !others.isEmpty {
                    Menu("Transférer à…", systemImage: "arrowshape.turn.up.right") {
                        ForEach(others) { target in
                            Button(target.name) { forward(to: target) }
                        }
                    }
                }
            }
            .sheet(isPresented: $isSelecting) { SelectableTextSheet(text: plain) }
    }

    /// Opens the other agent's ongoing conversation with the message in its composer, to add a word and send.
    private func forward(to target: AgentProfile) {
        let session = store.mainSessionID(for: target)
        UserDefaults.standard.set(plain, forKey: ConversationDraft.key(agentID: target.id, sessionID: session))
        router.open(.conversation(target, sessionID: session))
    }
}

/// The whole message in a text view where a passage can be selected (handles, Copy, Look Up…), phone numbers,
/// addresses and links tappable.
private struct SelectableTextSheet: View {
    var text: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            SelectableText(text: text)
                .padding(.horizontal, 12)
                .navigationTitle("Sélectionner du texte")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar { ToolbarItem(placement: .confirmationAction) { Button("OK") { dismiss() } } }
                .background(Theme.background)
        }
        .presentationDetents([.medium, .large])
    }
}

private struct SelectableText: UIViewRepresentable {
    var text: String

    func makeUIView(context: Context) -> UITextView {
        let view = UITextView()
        view.isEditable = false
        view.isSelectable = true
        view.dataDetectorTypes = [.phoneNumber, .link, .address]
        view.backgroundColor = .clear
        view.adjustsFontForContentSizeCategory = true
        view.font = UIFont.preferredFont(forTextStyle: .body)
        view.textColor = UIColor(Theme.ink)
        view.textContainerInset = UIEdgeInsets(top: 12, left: 4, bottom: 24, right: 4)
        view.text = text
        // Everything selected at first, handles showing: drag them to the passage wanted.
        DispatchQueue.main.async {
            view.becomeFirstResponder()
            view.selectedRange = NSRange(location: 0, length: (view.text as NSString).length)
        }
        return view
    }

    func updateUIView(_ view: UITextView, context: Context) {
        if view.text != text { view.text = text }
    }
}

/// A reply taken out of the app: copied, shared. Messaging apps format text their own way, so it goes out in
/// WhatsApp's syntax (`*bold*`, `_italic_`, `~struck~`, lists, quotes), which WhatsApp shows formatted and others read
/// cleanly; copying also carries the formatted text (RTF) for the apps that paste it (Notes, Mail). Links are written
/// « label : address » with the real address; tables become lines (« Lundi · 7 h 12 · 82 »).
enum ShareText {
    static func whatsapp(_ text: String) -> String {
        var out: [String] = []
        var inCode = false
        for line in flatTables(ChatText.visible(text)).components(separatedBy: "\n") {
            if line.trimmingCharacters(in: .whitespaces).hasPrefix("```") { inCode.toggle(); out.append(line); continue }
            if inCode { out.append(line); continue }
            var l = links(line)
            l = replace(l, "^#{1,6}\\s+(.+)$", "\u{0}$1\u{0}")
            l = replace(l, "\\*\\*([^*\\n]+?)\\*\\*|__([^_\\n]+?)__", "\u{0}$1$2\u{0}")
            l = replace(l, "(?<![*\\w])\\*([^*\\n]+?)\\*(?![*\\w])|(?<![_\\w])_([^_\\n]+?)_(?![_\\w])", "_$1$2_")
            l = replace(l, "~~([^~\\n]+?)~~", "~$1~")
            l = replace(l.replacingOccurrences(of: "\u{0}", with: "*"), "^(\\s*)[*•]\\s+", "$1- ")
            out.append(l)
        }
        return out.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// WhatsApp's text, plus the formatted text for the apps that take rich text.
    static func copy(_ text: String) {
        var item: [String: Any] = [UTType.utf8PlainText.identifier: whatsapp(text)]
        if let rtf = rich(text) { item[UTType.rtf.identifier] = rtf }
        UIPasteboard.general.setItems([item])
    }

    private static func rich(_ text: String) -> Data? {
        let shown = flatTables(ChatText.visible(text))
        guard let parsed = try? AttributedString(markdown: shown, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))
        else { return nil }
        let body = UIFont.preferredFont(forTextStyle: .body)
        let out = NSMutableAttributedString()
        for run in parsed.runs {
            let intent = run.inlinePresentationIntent ?? []
            var traits: UIFontDescriptor.SymbolicTraits = []
            if intent.contains(.stronglyEmphasized) { traits.insert(.traitBold) }
            if intent.contains(.emphasized) { traits.insert(.traitItalic) }
            if intent.contains(.code) { traits.insert(.traitMonoSpace) }
            let font = body.fontDescriptor.withSymbolicTraits(traits).map { UIFont(descriptor: $0, size: body.pointSize) } ?? body
            var attributes: [NSAttributedString.Key: Any] = [.font: font]
            if intent.contains(.strikethrough) { attributes[.strikethroughStyle] = NSUnderlineStyle.single.rawValue }
            if let link = run.link, safe(link) { attributes[.link] = link }
            out.append(NSAttributedString(string: String(parsed[run.range].characters), attributes: attributes))
        }
        return try? out.data(from: NSRange(location: 0, length: out.length),
                             documentAttributes: [.documentType: NSAttributedString.DocumentType.rtf])
    }

    private static func flatTables(_ text: String) -> String {
        ChatText.blocks(text).map { block in
            switch block {
            case .text(let paragraph): paragraph
            case .table(let table): ([table.header] + table.rows).map { $0.joined(separator: " · ") }.joined(separator: "\n")
            }
        }.joined(separator: "\n\n")
    }

    private static func safe(_ url: URL) -> Bool { ["http", "https", "mailto", "tel"].contains(url.scheme?.lowercased() ?? "") }

    /// `[label](address)` → « label : address » (just the address when the label is it).
    private static func links(_ line: String) -> String {
        guard let regex = try? NSRegularExpression(pattern: "\\[([^\\]\\n]+)\\]\\(([^)\\s]+)\\)") else { return line }
        var result = line
        for match in regex.matches(in: line, range: NSRange(line.startIndex..., in: line)).reversed() {
            guard let whole = Range(match.range, in: line), let l = Range(match.range(at: 1), in: line),
                  let a = Range(match.range(at: 2), in: line) else { continue }
            let label = String(line[l]), address = String(line[a])
            let shown: String
            if let url = URL(string: address), safe(url) {
                shown = label == address || "https://" + label == address ? address : "\(label) : \(address)"
            } else {
                shown = label
            }
            result.replaceSubrange(Range(match.range, in: result) ?? whole, with: shown)
        }
        return result
    }

    private static func replace(_ text: String, _ pattern: String, _ template: String) -> String {
        guard let regex = try? NSRegularExpression(pattern: pattern, options: [.anchorsMatchLines]) else { return text }
        return regex.stringByReplacingMatches(in: text, range: NSRange(text.startIndex..., in: text), withTemplate: template)
    }
}

extension ChatText {
    /// The message as read: markdown marks and MEDIA lines removed (for copy, share, select, forward).
    static func plain(_ text: String) -> String {
        let shown = visible(text)
        let parsed = try? AttributedString(markdown: shown, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))
        return parsed.map { String($0.characters) } ?? shown
    }

    private static let contactDetector = try? NSDataDetector(
        types: NSTextCheckingResult.CheckingType.phoneNumber.rawValue | NSTextCheckingResult.CheckingType.address.rawValue
            | NSTextCheckingResult.CheckingType.link.rawValue)

    /// Phone numbers, e-mail addresses and postal addresses become links (call, Mail, Plans), outside code and
    /// existing links. Web addresses are left to the caller (they get their own short label and site icon).
    static func linkContacts(in text: inout AttributedString) {
        guard let detector = contactDetector else { return }
        let plain = String(text.characters)
        for match in detector.matches(in: plain, range: NSRange(plain.startIndex..., in: plain)).reversed() {
            let url: URL?
            switch match.resultType {
            case .phoneNumber:
                let digits = (match.phoneNumber ?? "").filter { $0.isNumber || $0 == "+" }
                url = digits.filter(\.isNumber).count >= 8 ? URL(string: "tel:\(digits)") : nil // not a range or a code
            case .address:
                let query = (plain as NSString).substring(with: match.range)
                url = query.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed).flatMap { URL(string: "maps://?q=\($0)") }
            case .link where match.url?.scheme == "mailto":
                url = match.url
            default:
                url = nil
            }
            guard let url, let range = Range(match.range, in: plain),
                  let lower = AttributedString.Index(range.lowerBound, within: text),
                  let upper = AttributedString.Index(range.upperBound, within: text) else { continue }
            let span = lower..<upper
            guard text[span].runs.allSatisfy({ $0.link == nil && !($0.inlinePresentationIntent ?? []).contains(.code) }) else { continue }
            text[span].link = url
        }
    }

    /// Links that act on the phone (call, e-mail, map), shown in blue like in Messages.
    static func isContactLink(_ url: URL) -> Bool {
        ["tel", "mailto", "maps", "sms", "facetime"].contains(url.scheme?.lowercased() ?? "")
    }
}
