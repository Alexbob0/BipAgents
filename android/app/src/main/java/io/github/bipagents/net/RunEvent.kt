package io.github.bipagents.net

import kotlinx.serialization.json.JsonObject

/** A run's event, as HermesKit's HermesEvent maps it (only what the app shows). */
sealed interface RunEvent {
    data class Delta(val text: String) : RunEvent
    data class Interim(val text: String, val alreadyStreamed: Boolean) : RunEvent
    data class Tool(val name: String, val started: Boolean, val failed: Boolean) : RunEvent
    data class Approval(val run: String, val requestId: String?, val command: String?, val description: String?,
                        val choices: List<String>) : RunEvent
    data object ApprovalResolved : RunEvent
    data class Clarify(val run: String, val requestId: String, val questions: List<Question>, val perQuestion: Boolean) : RunEvent {
        data class Question(val id: String, val text: String, val choices: List<String>, val allowOther: Boolean, val multiSelect: Boolean)
    }
    data object ClarifyResolved : RunEvent
    /** `run.completed|failed|cancelled|interrupted`. */
    data class Finished(val failed: Boolean, val error: String?, val output: String?) : RunEvent

    companion object {
        fun of(event: ServerEvent): RunEvent? {
            val f = event.data
            val run = f.str("run_id", "runId") ?: ""
            return when (event.type) {
                "assistant.delta", "message.delta", "response.output_text.delta" -> Delta(text(f, "delta", "text", "content") ?: "")
                "assistant.commentary", "message.interim" -> Interim(text(f, "text", "content", "delta") ?: "", f.bool("already_streamed") ?: false)
                "tool.started", "tool.start", "tool.completed", "tool.complete", "tool.failed" -> Tool(
                    f.str("tool", "name", "tool_name") ?: "tool", event.type.endsWith("start") || event.type.endsWith("started"),
                    event.type == "tool.failed" || f.bool("error") == true)
                "approval.request", "approval.required" -> Approval(run, f.str("request_id", "approval_id"), f.str("command"),
                    f.str("description", "reason"),
                    f["choices"].arr()?.mapNotNull { it.text()?.lowercase() }?.ifEmpty { null } ?: listOf("once", "deny"))
                "approval.responded", "approval.resolved" -> ApprovalResolved
                "clarify.request", "clarify.required" -> clarify(f, run)
                "clarify.responded", "clarify.resolved", "clarify.cancelled", "clarify.canceled", "clarify.expired", "clarify.retired" -> ClarifyResolved
                "run.completed" -> Finished(false, null, text(f, "output", "final_response"))
                "run.failed" -> Finished(true, f.str("error") ?: f["error"].obj().str("message"), null)
                "run.cancelled", "run.canceled", "run.interrupted" -> Finished(false, null, null)
                else -> null
            }
        }

        private fun text(f: JsonObject, vararg keys: String): String? {
            for (key in keys) {
                val v = f[key] ?: continue
                v.text()?.let { return it }
                v.obj()?.str("content", "text", "delta")?.let { return it }
            }
            return null
        }

        private fun clarify(f: JsonObject, run: String): Clarify {
            fun question(q: JsonObject?, fallback: String): Clarify.Question? {
                val text = q.str("question", "text", "prompt") ?: return null
                val choices = (q?.get("choices") ?: q?.get("options")).arr()?.mapNotNull { it.text() ?: it.obj().str("label") } ?: emptyList()
                return Clarify.Question(q.str("id", "question_id") ?: fallback, text, choices,
                    q.bool("allow_other", "allow_free_text") ?: true, q.bool("multi_select") ?: false)
            }
            var questions = f["questions"].arr()?.mapIndexedNotNull { i, q -> question(q.obj(), "q${i + 1}") } ?: emptyList()
            if (questions.isEmpty()) questions = listOfNotNull(question(f, "q1"))
            val perQuestion = f.str("request_id") == null && f.str("clarify_id") != null
            return Clarify(run, f.str("request_id", "clarify_id", "id") ?: "", questions, perQuestion)
        }
    }
}
