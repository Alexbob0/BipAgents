import HermesKit
import LocalAuthentication
import SwiftUI
import WebKit

/// « Bureau » : the agent's own browser, live, full screen and in landscape too. Watching sends nothing (one finger
/// moves the zoomed view, two pinch, a double tap zooms); « Prendre la main » lets the person tap and type (to log in
/// to a site, solve what the agent asks), and the agent's browser waits until « Rendre la main ».
struct DesktopView: View {
    let agent: AgentProfile
    let desktop: BridgeClient.Desktop

    @Environment(AgentStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var holding = false
    @State private var isSwitching = false
    @State private var errorMessage: String?
    @State private var page = DesktopPage()
    /// A fresh ticket on every opening (they last ten minutes).
    @State private var current: BridgeClient.Desktop?
    /// A desktop shows logged-in sessions: Face ID (or the passcode) first, as for the vault.
    @State private var unlocked = false

    var body: some View {
        Group {
            if !unlocked {
                VStack(spacing: 14) {
                    Image(systemName: "lock.shield").font(.system(size: 44)).foregroundStyle(.white.opacity(0.8))
                    // White on the desktop's black, icon included (the system style drew it in the tint, unseen).
                    Button { Task { await open() } } label: {
                        Label("Déverrouiller", systemImage: "faceid")
                            .font(Theme.body(16, weight: .heavy))
                            .foregroundStyle(.black)
                            .padding(.horizontal, 24)
                            .frame(height: 48)
                            .background(.white, in: .capsule)
                    }
                    .buttonStyle(.plain)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let current {
                DesktopWebView(url: current.url, page: page).ignoresSafeArea()
            } else {
                ProgressView().tint(.white).frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
            .background(Color.black.ignoresSafeArea())
            .overlay(alignment: .top) { controls }
            .toolbar(.hidden, for: .navigationBar)
            .tabBarHidden(true)
            .persistentSystemOverlays(.hidden)
            .task { await open() }
            .onAppear { OrientationLock.allowLandscape(true) }
            .onDisappear {
                OrientationLock.allowLandscape(false)
                // Leaving the screen hands the desktop back: the agent never stays blocked by a closed view.
                guard holding, let bridge = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) else { return }
                Task { _ = try? await bridge.controlDesktop(of: agent.bridgeName, take: false) }
            }
    }

    /// Floating over the desktop (full screen): back, the agent's name or who holds it, and « Prendre la main ».
    private var controls: some View {
        VStack(spacing: 8) {
            HStack(spacing: 8) {
                Button { dismiss() } label: {
                    Image(systemName: "chevron.left")
                        .font(.system(size: 16, weight: .bold))
                        .frame(width: 38, height: 38)
                        .background(.black.opacity(0.6), in: .circle)
                }
                .accessibilityLabel("Retour")
                Text(holding ? ((current?.pausesAgent ?? true) ? "Tu as la main : \(agent.name) attend." : "Tu as la main.")
                             : "Bureau de \(agent.name)")
                    .font(Theme.body(13, weight: .bold))
                    .lineLimit(1)
                    .padding(.horizontal, 12).frame(height: 38)
                    .background(.black.opacity(0.6), in: .capsule)
                Spacer(minLength: 0)
                if desktop.relayed && unlocked {
                    Button { Task { await toggle() } } label: {
                        Group {
                            if isSwitching { ProgressView().tint(.black) } else { Text(holding ? "Rendre la main" : "Prendre la main") }
                        }
                        .font(Theme.body(14, weight: .heavy))
                        .foregroundStyle(.black)
                        .padding(.horizontal, 14).frame(height: 38)
                        .background(.white, in: .capsule)
                    }
                    .disabled(isSwitching)
                }
            }
            .foregroundStyle(.white)
            .buttonStyle(.plain)
            if let errorMessage {
                NoticeRow(text: errorMessage, isError: true)
            }
        }
        .padding(.horizontal, 12)
        .padding(.top, 4)
    }

    private func open() async {
        guard !unlocked else { return }
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) else {
            errorMessage = String(localized: "Active un code sur l’iPhone pour voir le bureau de tes agents.")
            return
        }
        guard (try? await context.evaluatePolicy(.deviceOwnerAuthentication,
                                                 localizedReason: String(localized: "Voir le bureau de \(agent.name)"))) == true
        else { return }
        unlocked = true
        let bridge = BridgeClient(agent: agent, secrets: store.secrets(for: agent))
        let fresh = (try? await bridge?.desktop(of: agent.bridgeName)) ?? nil
        current = fresh ?? desktop
        holding = current?.youHold ?? false
        page.setViewOnly(!holding)
    }

    private func toggle() async {
        guard let bridge = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) else { return }
        isSwitching = true
        defer { isSwitching = false }
        do {
            holding = try await bridge.controlDesktop(of: agent.bridgeName, take: !holding)
            page.setViewOnly(!holding)
            errorMessage = nil
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }
}

/// The page's switch between watching and acting (noVNC's `viewOnly`), reachable from SwiftUI.
@MainActor
final class DesktopPage {
    weak var webView: WKWebView?
    private var viewOnly = true

    func setViewOnly(_ on: Bool) {
        viewOnly = on
        webView?.evaluateJavaScript("window.setViewOnly && window.setViewOnly(\(on))")
    }

    func pageLoaded() { setViewOnly(viewOnly) }
}

struct DesktopWebView: UIViewRepresentable {
    let url: URL
    let page: DesktopPage

    func makeCoordinator() -> Coordinator { Coordinator(page: page) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.userContentController.add(WeakMessageHandler(context.coordinator), name: "desktop")
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = context.coordinator
        view.scrollView.isScrollEnabled = false
        view.isOpaque = false
        view.backgroundColor = .black
        page.webView = view
        view.load(URLRequest(url: url))
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {}

    final class Coordinator: NSObject, WKNavigationDelegate, WKScriptMessageHandler {
        let page: DesktopPage
        init(page: DesktopPage) { self.page = page }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            Task { @MainActor in page.pageLoaded() }
        }

        /// « Coller » the page could not read the clipboard for: the app types the copied text itself.
        func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
            guard message.body as? String == "paste-refused" else { return }
            Task { @MainActor in
                guard let text = UIPasteboard.general.string, !text.isEmpty,
                      let data = try? JSONEncoder().encode(text), let literal = String(data: data, encoding: .utf8) else { return }
                _ = try? await page.webView?.evaluateJavaScript("window.typeText && window.typeText(\(literal))")
            }
        }
    }

    /// The content controller keeps its handlers: a weak one lets the coordinator go with the view.
    final class WeakMessageHandler: NSObject, WKScriptMessageHandler {
        weak var target: WKScriptMessageHandler?
        init(_ target: WKScriptMessageHandler) { self.target = target }
        func userContentController(_ controller: WKUserContentController, didReceive message: WKScriptMessage) {
            target?.userContentController(controller, didReceive: message)
        }
    }
}
