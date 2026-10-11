import HermesKit
import LocalAuthentication
import SwiftUI
import WebKit

/// « Bureau » : the agent's own browser, live. Watching sends nothing; « Prendre la main » lets the person tap
/// and type (to log in to a site, solve what the agent asks), and the agent's browser waits until « Rendre la main ».
struct DesktopView: View {
    let agent: AgentProfile
    let desktop: BridgeClient.Desktop

    @Environment(AgentStore.self) private var store
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
                    Button("Déverrouiller", systemImage: "faceid") { Task { await open() } }
                        .buttonStyle(.borderedProminent)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let current {
                DesktopWebView(url: current.url, page: page)
            } else {
                ProgressView().tint(.white).frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
            .ignoresSafeArea(edges: .bottom)
            .background(Color.black)
            .overlay(alignment: .top) {
                if let errorMessage {
                    NoticeRow(text: errorMessage, isError: true).padding()
                } else if holding {
                    Text((current?.pausesAgent ?? true) ? "Tu as la main : \(agent.name) attend." : "Tu as la main.")
                        .font(Theme.body(13, weight: .bold)).foregroundStyle(.white)
                        .padding(.horizontal, 12).padding(.vertical, 6)
                        .background(.black.opacity(0.6), in: .capsule).padding(.top, 8)
                }
            }
            .navigationTitle("Bureau de \(agent.name)")
            .navigationBarTitleDisplayMode(.inline)
            .tabBarHidden(true)
            .toolbar {
                if desktop.relayed && unlocked {
                    ToolbarItem(placement: .topBarTrailing) {
                        if isSwitching {
                            ProgressView()
                        } else {
                            Button(holding ? "Rendre la main" : "Prendre la main") { Task { await toggle() } }
                                .fontWeight(.bold)
                        }
                    }
                }
            }
            .task { await open() }
            .onDisappear {
                // Leaving the screen hands the desktop back: the agent never stays blocked by a closed view.
                guard holding, let bridge = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) else { return }
                Task { _ = try? await bridge.controlDesktop(of: agent.bridgeName, take: false) }
            }
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

    final class Coordinator: NSObject, WKNavigationDelegate {
        let page: DesktopPage
        init(page: DesktopPage) { self.page = page }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            Task { @MainActor in page.pageLoaded() }
        }
    }
}
