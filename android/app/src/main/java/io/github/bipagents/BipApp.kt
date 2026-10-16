package io.github.bipagents

import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.launch

import android.app.Application
import io.github.bipagents.model.AgentStore
import io.github.bipagents.push.Push

@OptIn(kotlinx.coroutines.FlowPreview::class)
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
        // The home-screen widgets follow the agents' last messages and looks.
        kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.SupervisorJob() + kotlinx.coroutines.Dispatchers.Default).launch {
            kotlinx.coroutines.flow.combine(store.latest, store.agents) { _, _ -> }.debounce(1500).collect {
                io.github.bipagents.widget.AgentWidget.refresh(this@BipApp)
            }
        }
    }
}
