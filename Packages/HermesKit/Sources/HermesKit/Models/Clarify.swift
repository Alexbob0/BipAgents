import Foundation

/// A question the agent asks the user mid-run (`clarify` tool), from a `clarify.request` event.
/// See `docs/hermes-clarify-api.md`.
public struct ClarifyRequest: Sendable, Codable, Hashable, Identifiable {
    public struct Question: Sendable, Codable, Hashable, Identifiable {
        public var id: String
        public var question: String
        /// Empty: an open question.
        public var choices: [String]
        /// A free answer is accepted besides the choices.
        public var allowOther: Bool

        public init(id: String, question: String, choices: [String] = [], allowOther: Bool = true) {
            self.id = id
            self.question = question
            self.choices = choices
            self.allowOther = allowOther
        }
    }

    public var runID: String
    public var requestID: String
    public var questions: [Question]

    public var id: String { "\(runID)/\(requestID)" }

    public init(runID: String, requestID: String, questions: [Question]) {
        self.runID = runID
        self.requestID = requestID
        self.questions = questions
    }
}
