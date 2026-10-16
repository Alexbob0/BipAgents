package io.github.bipagents

import android.app.Application
import io.github.bipagents.model.AgentStore
import io.github.bipagents.push.Push

class BipApp : Application() {
    lateinit var store: AgentStore
        private set
    lateinit var inbox: io.github.bipagents.model.Inbox
        private set

    override fun onCreate() {
        super.onCreate()
        io.github.bipagents.ui.Appearance.load(this)
        store = AgentStore(this)
        inbox = io.github.bipagents.model.Inbox(this)
        Push.start(this)
    }
}
