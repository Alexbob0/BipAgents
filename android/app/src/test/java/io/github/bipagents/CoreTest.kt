package io.github.bipagents

import io.github.bipagents.model.Category
import io.github.bipagents.model.Pairing
import io.github.bipagents.net.Message
import io.github.bipagents.net.RunEvent
import io.github.bipagents.net.ServerEvent
import io.github.bipagents.net.Session
import io.github.bipagents.ui.Block
import io.github.bipagents.ui.ChatModel
import io.github.bipagents.ui.Item
import io.github.bipagents.ui.Markdown
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The contract with the bridge and Hermes, as the iPhone app reads it (HermesKit's tests). */
class CoreTest {
    @Test fun anInstallCodeAndAnAgentCode() {
        val install = Pairing.parse("""{"v":2,"bridgeURL":"https://box.tail1234.ts.net:9203","bridgeKey":"k"}""")
        assertEquals(Pairing.Install("https://box.tail1234.ts.net:9203", "k"), install)
        val single = Pairing.parse("""{"name":"Vie","baseURL":"https://h.ts.net:8644","apiKey":"a","bridgeURL":"https://h.ts.net:8643","bridgeKey":"b","agent":"Vie"}""")
        assertTrue(single is Pairing.Single)
        single as Pairing.Single
        assertEquals("vie", single.agent.bridgeAgent)
        assertEquals("b", single.keys.bridgeKey)
        assertNull(Pairing.parse("""{"v":2,"bridgeURL":"ftp://x","bridgeKey":"k"}"""))
        assertNull(Pairing.parse("not json"))
    }

    @Test fun runEventsAsHermesSendsThem() {
        fun event(type: String, data: String) = RunEvent.of(ServerEvent.of(type, data)!!)
        assertEquals(RunEvent.Delta("Hi"), event("message.delta", """{"delta":"Hi"}"""))
        assertEquals(RunEvent.Delta("Hi"), event("message", """{"type":"assistant.delta","delta":{"content":"Hi"}}"""))
        val approval = event("approval.request", """{"run_id":"r1","request_id":"q","command":"rm -rf x","choices":["once","session","always","deny"]}""")
        assertEquals(RunEvent.Approval("r1", "q", "rm -rf x", null, listOf("once", "session", "always", "deny")), approval)
        // An approval's own fields may carry a plain `type` (a command category): the dotted SSE name wins.
        assertTrue(event("approval.request", """{"type":"shell","run_id":"r1"}""") is RunEvent.Approval)
        val clarify = event("clarify.request", """{"run_id":"r1","clarify_id":"c1","question":"Which?","choices":["A",{"label":"B"}]}""") as RunEvent.Clarify
        assertTrue(clarify.perQuestion)
        assertEquals(listOf("A", "B"), clarify.questions.single().choices)
        assertEquals(RunEvent.Finished(true, "boom", null), event("run.failed", """{"error":"boom"}"""))
        assertNull(ServerEvent.of(null, "[DONE]"))
    }

    @Test fun repliesCutIntoBlocks() {
        val blocks = Markdown.blocks("Hello **you**\n\n| A | B |\n|---|:-:|\n| 1 | 2 |\n\n- one\n- two\n\n```\ncode | not a table\n```\nMEDIA:/tmp/a.mp3")
        assertEquals(Block.Paragraph("Hello **you**"), blocks[0])
        val table = blocks[1] as Block.Table
        assertEquals(listOf("A", "B"), table.header)
        assertEquals(listOf(listOf("1", "2")), table.rows)
        assertEquals(Block.Items(false, listOf("one", "two")), blocks[2])
        assertEquals(Block.Code("code | not a table"), blocks[3])
        assertEquals(4, blocks.size)   // the MEDIA line is not shown as text
    }

    @Test fun theThreadFoldsToolsUnderReplies() {
        val items = ChatModel.thread(listOf(
            Message("1", "user", "Sleep?", null, false, emptyList()),
            Message("2", "assistant", "", null, false, listOf("oura_sleep")),
            Message("3", "tool", "{}", null, false, emptyList()),
            Message("4", "assistant", "7 h", null, false, emptyList()),
        ))
        assertEquals(2, items.size)
        assertEquals(listOf("oura_sleep"), (items[1] as Item.Reply).tools)
    }

    @Test fun theMainThreadAndCategories() {
        val sessions = listOf(Session("cron_1", "Morning", 300, null, "cron"), Session("s2", "Plan", 200, null, null),
            Session("b", "Bot Chat", 100, null, null))
        assertEquals("b", Session.main(sessions)?.id)
        assertEquals("s2", Session.main(sessions.dropLast(1))?.id)
        assertEquals(Category.WELLNESS, Category.suggest("Wellness"))
        assertEquals(Category.DAILY, Category.suggest("Zorglub"))
    }
}
