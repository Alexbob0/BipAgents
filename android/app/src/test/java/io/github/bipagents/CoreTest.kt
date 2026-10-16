package io.github.bipagents

import io.github.bipagents.model.Category
import io.github.bipagents.model.Pairing
import io.github.bipagents.net.Message
import io.github.bipagents.net.ToolStep
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

    @Test fun repliesAsTheIPhoneShowsThem() {
        // Only tables are cut out; the rest stays as written (inline Markdown is styled when drawn).
        val blocks = Markdown.blocks("Hello **you**\n\n| A | B |\n|---|:-:|\n| 1 | 2 |\n\n- one\n- two\n\n```\ncode | not a table\n|---|\n```\nMEDIA:/tmp/a.mp3")
        assertEquals(Block.Paragraph("Hello **you**"), blocks[0])
        val table = blocks[1] as Block.Table
        assertEquals(listOf("A", "B"), table.header)
        assertEquals(listOf(listOf("1", "2")), table.rows)
        assertEquals(Block.Paragraph("- one\n- two\n\n```\ncode | not a table\n|---|\n```"), blocks[2])
        assertEquals(3, blocks.size)   // the MEDIA line is not shown as text
    }

    @Test fun theThreadFoldsToolsUnderReplies() {
        val items = ChatModel.thread(listOf(
            Message("1", "user", "Sleep?", null, false, emptyList()),
            Message("2", "assistant", "Je regarde.", null, false, listOf(ToolStep("oura_sleep", "{\"query\": \"nights\"}", callId = "c1"))),
            Message("3", "tool", "{}", null, false, listOf(ToolStep("oura_sleep", "{}", callId = "c1"))),
            Message("4", "assistant", "7 h", null, false, emptyList()),
        ))
        assertEquals(listOf("User", "Tools", "Reply", "Reply"), items.map { it::class.simpleName })
        assertEquals(1, (items[1] as Item.Tools).steps.size)   // the result completes the call, no second line
        // The commentary before a tool and the tool fold together; the final answer stays out.
        val folds = io.github.bipagents.ui.ThreadLayout.workFolds(items, running = false)
        assertEquals(setOf(items[1].key), folds.keys)
        assertEquals(false, io.github.bipagents.ui.ThreadLayout.turnEnd(items, 2))
        assertEquals(true, io.github.bipagents.ui.ThreadLayout.turnEnd(items, 3))
        assertEquals("nights", io.github.bipagents.ui.ChatText.toolSummary("{\"query\": \"nights\"}"))
        assertEquals("ls -la", io.github.bipagents.ui.ChatText.toolSummary("{\"command\": \"ls -la\", \"timeout\": 3"))
    }

    @Test fun liveToolEventsCompleteTheirCall() {
        var items = ChatModel.upsert(emptyList(), ToolStep("terminal", "ls", ToolStep.Status.STARTED))
        items = ChatModel.upsert(items, ToolStep("terminal", null, ToolStep.Status.DONE, 0.4))
        val step = (items.single() as Item.Tools).steps.single()
        assertEquals(ToolStep.Status.DONE, step.status)
        assertEquals("ls", step.preview)
    }

    @Test fun quickRepliesAndTeammates() {
        assertEquals(listOf("Le matin", "Le soir"), io.github.bipagents.ui.ChatText.choices("Tu préfères quand ?\n1. Le matin\n2. **Le soir**"))
        assertEquals(emptyList<String>(), io.github.bipagents.ui.ChatText.choices("Conseils :\n1. Dormir\n2. Boire"))
        val t = io.github.bipagents.ui.ChatText.teammate("Message from 🤖 Vie (@vie): Salut")!!
        assertEquals(Triple("Vie", "vie", "Salut"), Triple(t.name, t.handle, t.body))
        val items = listOf(Item.User("a", "Message from 🤖 Vie (@vie): Tu as l'heure ?", false, null), Item.Reply("b", "Oui", null))
        assertEquals(mapOf("a" to "a", "b" to "a"), io.github.bipagents.ui.ThreadLayout.exchangeOwners(items))
        assertEquals("Vie", io.github.bipagents.ui.ThreadLayout.answered(items, 1))
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

class ShareTextTest {
    @Test fun repliesGoOutInWhatsAppSyntax() {
        val text = "## Plan\nVoici **le plan** et *une idée* ~~barré~~.\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n* un\n* deux"
        assertEquals("*Plan*\nVoici *le plan* et _une idée_ ~barré~.\n\nA · B\n1 · 2\n\n- un\n- deux",
            io.github.bipagents.ui.ShareText.whatsapp(text))
    }
}

class RoutineScheduleTest {
    @Test fun schedulesReadAndWriteAsHermesStoresThem() {
        val s = io.github.bipagents.ui.RoutineSchedule
        assertEquals(io.github.bipagents.ui.RoutineSchedule.Weekdays(8, 0), s.of("0 8 * * 1-5"))
        assertEquals(io.github.bipagents.ui.RoutineSchedule.Weekly(7, 18, 30), s.of("30 18 * * 0"))
        assertEquals(io.github.bipagents.ui.RoutineSchedule.Monthly(15, 9, 5), s.of("5 9 15 * *"))
        assertEquals(io.github.bipagents.ui.RoutineSchedule.EveryHours(2), s.of("every 120m"))
        assertEquals("every 3h", io.github.bipagents.ui.RoutineSchedule.EveryHours(3).hermes)
        assertEquals("30 18 * * 0", io.github.bipagents.ui.RoutineSchedule.Weekly(7, 18, 30).hermes)
        assertEquals(io.github.bipagents.ui.RoutineSchedule.Custom("*/5 * * * *"), s.of("*/5 * * * *"))
        val once = s.of("2026-10-12T08:00:00+02:00") as io.github.bipagents.ui.RoutineSchedule.Once
        assertEquals(1791784800000L, once.at)
    }
}

class LanDoorTest {
    @Test fun anInstallCodeCarriesItsLocalDoor() {
        val fp = "15".repeat(32)
        val p = io.github.bipagents.model.Pairing.parse("""{"v":"2","bridgeURL":"https://box.ts.net:8643","bridgeKey":"k","lan":{"url":"https://192.168.1.20:8650","fingerprint":"${fp.uppercase()}"}}""")
            as io.github.bipagents.model.Pairing.Install
        assertEquals("https://192.168.1.20:8650", p.lanUrl)
        assertEquals(fp, p.lanFingerprint)
        // No fingerprint, no door: a certificate can't be trusted without it.
        val none = io.github.bipagents.model.Pairing.parse("""{"v":"2","bridgeURL":"https://box.ts.net:8643","bridgeKey":"k","lan":{"url":"https://192.168.1.20:8650"}}""")
            as io.github.bipagents.model.Pairing.Install
        assertNull(none.lanUrl)
    }
}
