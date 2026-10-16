package io.github.bipagents.ui

/** How the thread groups its lines (ConversationView's workFolds, exchangeOwners, isTurnEnd). */
object ThreadLayout {
    /** The longest text that still counts as commentary between tools: an answer written before the agent goes on
     *  working stays in sight. */
    private const val COMMENTARY = 200

    /** The agent's work, folded by default: item key → the first key of its fold. Work = tools, reasoning, and short
     *  commentary between tools (« Je vais chercher… »): a text followed in its turn by a tool, then by another text or a
     *  tool still running. A file sent along the way stays in sight. */
    fun workFolds(items: List<Item>, running: Boolean): Map<String, String> {
        val folds = HashMap<String, String>()
        var start = 0
        while (start < items.size) {
            var end = start + 1
            while (end < items.size && items[end] !is Item.User) end++
            val turnRunning = end == items.size && running
            var current: String? = null
            for (i in start until end) {
                val item = items[i]
                val later = items.subList(i + 1, end)
                val work = when (item) {
                    is Item.Tools, is Item.Reasoning -> true
                    is Item.Reply -> ChatText.media(item.text).isEmpty() && ChatText.visible(item.text).length <= COMMENTARY &&
                        later.any { it is Item.Tools } && (turnRunning || later.any { it is Item.Reply })
                    else -> false
                }
                if (work) {
                    if (current == null) current = item.key
                    folds[item.key] = current
                } else current = null
            }
            start = end
        }
        return folds
    }

    /** The last reply before the person's next message (or the end): the turn's answer (« Listen », its time). */
    fun turnEnd(items: List<Item>, index: Int): Boolean {
        for (next in items.subList(index + 1, items.size)) when (next) {
            is Item.User -> return true
            is Item.Reply -> return false
            else -> {}
        }
        return true
    }

    /** Exchanges with other agents (Bot Mode), folded: item key → the teammate message opening it. A request from
     *  another agent folds with the whole turn answering it; an answer to this agent's request folds alone. */
    fun exchangeOwners(items: List<Item>): Map<String, String> {
        val owners = HashMap<String, String>()
        var current: String? = null
        items.forEachIndexed { index, item ->
            if (item is Item.User) {
                current = null
                if (ChatText.teammate(item.text) != null) {
                    owners[item.key] = item.key
                    if (isRequest(items, index)) current = item.key
                }
            } else current?.let { owners[item.key] = it }
        }
        return owners
    }

    /** A teammate message asking this agent something (vs. answering what this agent sent with message_agent). */
    fun isRequest(items: List<Item>, index: Int): Boolean {
        val teammate = (items[index] as? Item.User)?.let { ChatText.teammate(it.text) } ?: return false
        return !asked(items, index, teammate.handle)
    }

    /** The agent whose request this reply answers, on the turn's first reply. */
    fun answered(items: List<Item>, index: Int): String? {
        val prompt = (index - 1 downTo 0).firstOrNull { items[it] is Item.User } ?: return null
        val teammate = ChatText.teammate((items[prompt] as Item.User).text) ?: return null
        if (items.subList(prompt + 1, index).any { it is Item.Reply }) return null
        return if (asked(items, prompt, teammate.handle)) null else teammate.name
    }

    private fun asked(items: List<Item>, before: Int, handle: String) = items.subList(maxOf(0, before - 40), before).any { item ->
        item is Item.Tools && item.steps.any { "message_agent" in it.name && (it.preview ?: "").lowercase().contains(handle.lowercase()) }
    }
}
