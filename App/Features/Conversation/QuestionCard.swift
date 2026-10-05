import HermesKit
import SwiftUI

/// The agent asks something mid-task (Hermes `clarify`): one button per choice — a single question is
/// answered in one tap, like Hermes Desktop — plus « Autre réponse… » when a free answer is allowed.
struct QuestionCard: View {
    var request: ClarifyRequest
    var state: QuestionState
    var agent: AgentProfile
    /// `nil`: skip (the agent goes on without an answer).
    var onAnswer: ([String: String]?) -> Void

    @State private var picked: [String: String] = [:]
    @State private var writing: String?
    @State private var draft = ""

    private var palette: AgentPalette { agent.appearance.palette }
    private var isPending: Bool { state == .pending }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 12) {
                MascotView(appearance: agent.appearance, mood: isPending ? .asking : .happy)
                    .padding(4)
                    .frame(width: 52, height: 52)
                    .background(palette.tint, in: .rect(cornerRadius: 18, style: .continuous))
                Text(header).font(Theme.title(17))
            }
            ForEach(request.questions) { question in
                VStack(alignment: .leading, spacing: 8) {
                    Text(question.question).font(Theme.body(16, weight: .bold)).foregroundStyle(Theme.ink)
                    switch state {
                    case .pending: choices(for: question)
                    case .answered(let answers):
                        if let answer = answers[question.id] {
                            Label(answer, systemImage: "checkmark").font(Theme.body(15, weight: .heavy)).foregroundStyle(palette.deep)
                        }
                    case .closed:
                        Text("Sans réponse").font(Theme.body(14, weight: .bold)).foregroundStyle(Theme.muted)
                    }
                }
            }
            if isPending && request.questions.count > 1 {
                Button("Envoyer") { onAnswer(picked) }
                    .buttonStyle(.pill(.primary, height: 46))
                    .disabled(picked.count < request.questions.count)
            }
            if isPending {
                Button("Passer") { onAnswer(nil) }
                    .font(Theme.body(14, weight: .bold))
                    .foregroundStyle(Theme.muted)
                    .buttonStyle(.plain)
            }
        }
        .padding(16)
        .background(Theme.card, in: .rect(cornerRadius: 28, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 28, style: .continuous).stroke(isPending ? palette.main : Theme.line, lineWidth: 2))
    }

    private var header: String {
        switch state {
        case .pending: request.questions.count > 1 ? String(localized: "\(agent.name) a quelques questions") : String(localized: "\(agent.name) te pose une question")
        case .answered: String(localized: "Réponse envoyée")
        case .closed: String(localized: "Question close")
        }
    }

    @ViewBuilder
    private func choices(for question: ClarifyRequest.Question) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(question.choices, id: \.self) { choice in
                Button { choose(choice, for: question) } label: {
                    HStack {
                        Text(choice).multilineTextAlignment(.leading)
                        Spacer(minLength: 0)
                        if picked[question.id] == choice { Image(systemName: "checkmark") }
                    }
                    .font(Theme.body(15, weight: .heavy))
                    .foregroundStyle(picked[question.id] == choice ? Theme.onInk : palette.deep)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 12)
                    .background(picked[question.id] == choice ? AnyShapeStyle(Theme.ink) : AnyShapeStyle(palette.tint),
                                in: .rect(cornerRadius: 18, style: .continuous))
                }
                .buttonStyle(.plain)
            }
            if question.allowOther || question.choices.isEmpty {
                if writing == question.id || question.choices.isEmpty {
                    HStack(spacing: 8) {
                        TextField("Ta réponse", text: $draft, axis: .vertical)
                            .font(Theme.body(15))
                            .padding(.horizontal, 14)
                            .padding(.vertical, 10)
                            .background(Theme.field, in: .rect(cornerRadius: 16, style: .continuous))
                        Button {
                            let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
                            guard !text.isEmpty else { return }
                            choose(text, for: question)
                        } label: {
                            Image(systemName: "arrow.up")
                                .font(.system(size: 15, weight: .black))
                                .foregroundStyle(.white)
                                .frame(width: 40, height: 40)
                                .background(palette.deep, in: .circle)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Envoyer la réponse")
                    }
                } else {
                    Button("Autre réponse…") { writing = question.id; draft = "" }
                        .font(Theme.body(14, weight: .bold))
                        .foregroundStyle(palette.deep)
                        .buttonStyle(.plain)
                }
            }
        }
    }

    private func choose(_ answer: String, for question: ClarifyRequest.Question) {
        picked[question.id] = answer
        writing = nil
        // One question: one tap answers, like Hermes Desktop. Several: « Envoyer » once all are picked.
        if request.questions.count == 1 { onAnswer(picked) }
    }
}
