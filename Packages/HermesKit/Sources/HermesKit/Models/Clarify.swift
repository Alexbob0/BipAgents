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
        /// Several choices can be picked (sent as a JSON array of labels).
        public var multiSelect: Bool

        public init(id: String, question: String, choices: [String] = [], allowOther: Bool = true, multiSelect: Bool = false) {
            self.id = id
            self.question = question
            self.choices = choices
            self.allowOther = allowOther
            self.multiSelect = multiSelect
        }
    }

    public var runID: String
    public var requestID: String
    public var questions: [Question]
    /// Hermes' upstream contract (one question per `clarify.request`, identified by `clarify_id`, answered with
    /// `{"clarify_id", "response"}`), rather than BipAgents' patch (a batch under `request_id`).
    public var perQuestion: Bool = false

    public var id: String { "\(runID)/\(requestID)" }

    public init(runID: String, requestID: String, questions: [Question], perQuestion: Bool = false) {
        self.runID = runID
        self.requestID = requestID
        self.questions = questions
        self.perQuestion = perQuestion
    }
}
