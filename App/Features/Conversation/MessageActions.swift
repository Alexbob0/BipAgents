import SwiftUI
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
                Button("Copier", systemImage: "doc.on.doc") { UIPasteboard.general.string = plain }
                ShareLink(item: plain) { Label("Partager…", systemImage: "square.and.arrow.up") }
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
