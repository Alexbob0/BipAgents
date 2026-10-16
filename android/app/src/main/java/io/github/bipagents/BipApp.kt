package io.github.bipagents

import android.app.Application
import io.github.bipagents.model.AgentStore
import io.github.bipagents.push.Push

class BipApp : Application() {
    lateinit var store: AgentStore
        private set

    override fun onCreate() {
        super.onCreate()
        store = AgentStore(this)
        Push.start(this)
    }
}
