package io.github.bipagents.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.github.bipagents.R
import io.github.bipagents.model.Agent
import io.github.bipagents.model.Category
import io.github.bipagents.model.Look

/** The iPhone's custom colours (AgentAppearance.customChoices). */
private val CUSTOM = listOf("#3CC37A", "#F6B826", "#FF7B5C", "#4F7CFF", "#8E6CF5", "#1FB2C1", "#F2649E")

/** « Apparence »: which Bip the agent is, and its colour (its own, or one of seven). Saved for every app. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun LookPicker(agent: Agent, onPick: (Category, String?) -> Unit, onDismiss: () -> Unit) {
    var category by remember { mutableStateOf(agent.look.category) }
    var color by remember { mutableStateOf(agent.color) }
    AlertDialog(onDismissRequest = onDismiss, containerColor = Bip.card,
        title = { Text(stringResource(R.string.appearance), style = Bip.text(20, FontWeight.Black)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp), maxItemsInEachRow = 4) {
                    for (c in Category.entries) {
                        val on = c == category
                        Column(Modifier.background(if (on) Look.of(c, color).tint else Bip.ground, RoundedCornerShape(16.dp))
                            .then(if (on) Modifier.border(2.dp, Look.of(c, color).main, RoundedCornerShape(16.dp)) else Modifier)
                            .tap { category = c }.padding(6.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                            Mascot(Look.of(c, color), Modifier.size(48.dp), animated = false)
                            Text(stringResource(categoryLabel(c)), style = Bip.text(11, FontWeight.ExtraBold, Bip.ink2), textAlign = TextAlign.Center,
                                maxLines = 1, modifier = Modifier.size(56.dp, 16.dp))
                        }
                    }
                }
                Text(stringResource(R.string.color), style = Bip.text(14, FontWeight.ExtraBold, Bip.muted))
                FlowRow(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Dot(category.main, color == null) { color = null }
                    for (hex in CUSTOM) Dot(Color(android.graphics.Color.parseColor(hex)), color == hex) { color = hex }
                }
            }
        },
        confirmButton = { TextButton({ onPick(category, color) }) { Text(stringResource(R.string.save), style = Bip.text(16, FontWeight.ExtraBold, Bip.accent)) } },
        dismissButton = { TextButton(onDismiss) { Text(stringResource(R.string.cancel), style = Bip.text(16, FontWeight.Bold, Bip.ink2)) } })
}

@Composable
private fun Dot(color: Color, on: Boolean, onClick: () -> Unit) {
    Box(Modifier.size(34.dp).background(color, CircleShape).then(if (on) Modifier.border(3.dp, Bip.ink, CircleShape) else Modifier).tap(onClick))
}
