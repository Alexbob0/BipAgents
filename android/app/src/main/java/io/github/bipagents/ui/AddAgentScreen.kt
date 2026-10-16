package io.github.bipagents.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalLifecycleOwner
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import io.github.bipagents.model.Pairing
import kotlinx.coroutines.launch
import java.util.concurrent.Executors

/** « + » › Scan the QR code (App/Features/Agents/AddAgentFlow.swift): an install's code adds all its agents. */
@Composable
fun AddAgentScreen(store: AgentStore, onDone: () -> Unit, onCreate: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    var pasted by remember { mutableStateOf("") }
    val unknown = stringResource(R.string.qr_unknown)
    var granted by remember {
        mutableStateOf(ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED)
    }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it }
    LaunchedEffect(Unit) { if (!granted) ask.launch(Manifest.permission.CAMERA) }

    val familyInvitation = stringResource(R.string.qr_is_family_invitation)
    // One of this phone's servers can create agents: « Create an agent » comes first.
    var creatable by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { creatable = store.creatable(java.util.Locale.getDefault().language) != null }
    fun use(payload: String) {
        if (busy) return
        if (payload.contains("\"kind\"") && payload.contains("\"family\"")) { message = familyInvitation; return }
        val pairing = Pairing.parse(payload) ?: run { message = unknown; return }
        busy = true
        message = null
        scope.launch {
            try {
                val added = store.pair(pairing)
                android.widget.Toast.makeText(context, context.resources.getQuantityString(R.plurals.agents_added, added, added),
                    android.widget.Toast.LENGTH_SHORT).show()
                onDone()
            } catch (error: Throwable) {
                message = describe(context, error)
                busy = false
            }
        }
    }

    Column(Modifier.fillMaxSize().imePadding()) {
        TopBar(stringResource(R.string.add_agent), onDone)
        Column(Modifier.verticalScroll(rememberScrollState()).padding(horizontal = 18.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp)) {
            if (creatable) Row(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(22.dp)).tap(onCreate).padding(16.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("✨", style = Bip.text(24))
                Column(Modifier.weight(1f)) {
                    Text(stringResource(R.string.create_agent), style = Bip.text(17, FontWeight.ExtraBold))
                    Text(stringResource(R.string.create_agent_help), style = Bip.text(14, color = Bip.ink2))
                }
            }
            Text(stringResource(R.string.add_help), style = Bip.text(16, color = Bip.ink2))
            Box(Modifier.fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(28.dp)).background(Bip.ink), contentAlignment = Alignment.Center) {
                if (granted && !busy) Scanner(onCode = ::use)
                if (busy) CircularProgressIndicator(color = Bip.card)
                if (!granted) Text(stringResource(R.string.camera_needed), style = Bip.text(16, FontWeight.Bold, Bip.card), modifier = Modifier.padding(24.dp))
            }
            message?.let { Text(it, style = Bip.text(15, FontWeight.Bold, Bip.danger)) }
            Text(stringResource(R.string.paste_title), style = Bip.text(17, FontWeight.ExtraBold))
            OutlinedTextField(pasted, { pasted = it }, Modifier.fillMaxWidth(), placeholder = { Text("{\"v\":2, \"bridgeURL\": …}") },
                minLines = 2, maxLines = 5, textStyle = Bip.text(14))
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.background(if (pasted.isBlank()) Bip.line else Bip.ink, RoundedCornerShape(24.dp))
                    .tap { if (pasted.isNotBlank()) use(pasted) }.padding(horizontal = 22.dp, vertical = 12.dp)) {
                    Text(stringResource(R.string.add), style = Bip.text(16, FontWeight.ExtraBold, Bip.card))
                }
                val empty = stringResource(R.string.clipboard_empty)
                Pill(stringResource(R.string.paste_clipboard), dark = false) {
                    val clip = context.getSystemService(android.content.ClipboardManager::class.java).primaryClip
                    val text = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString()
                    if (text.isNullOrBlank()) message = empty else { pasted = text; use(text) }
                }
            }
            ManualAgent(store, onDone)
        }
    }
}

/** The camera, reading QR codes with ML Kit (on the device). */
@androidx.annotation.OptIn(markerClass = [androidx.camera.core.ExperimentalGetImage::class])
@Composable
fun Scanner(onCode: (String) -> Unit) {
    val context = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val executor = remember { Executors.newSingleThreadExecutor() }
    val scanner = remember { BarcodeScanning.getClient(BarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build()) }
    DisposableEffect(Unit) { onDispose { executor.shutdown(); scanner.close() } }
    AndroidView(factory = { ctx ->
        val view = PreviewView(ctx)
        val future = ProcessCameraProvider.getInstance(ctx)
        future.addListener({
            val provider = future.get()
            val preview = Preview.Builder().build().also { it.surfaceProvider = view.surfaceProvider }
            val analysis = ImageAnalysis.Builder().setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST).build()
            analysis.setAnalyzer(executor) { proxy ->
                val image = proxy.image
                if (image == null) { proxy.close(); return@setAnalyzer }
                scanner.process(InputImage.fromMediaImage(image, proxy.imageInfo.rotationDegrees))
                    .addOnSuccessListener { codes -> codes.firstNotNullOfOrNull { it.rawValue }?.let { value -> view.post { onCode(value) } } }
                    .addOnCompleteListener { proxy.close() }
            }
            provider.unbindAll()
            provider.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
        }, ContextCompat.getMainExecutor(context))
        view
    }, modifier = Modifier.fillMaxSize())
}

/** By hand: an agent's address and key, and its bridge's (voice, files, notifications). */
@Composable
private fun ManualAgent(store: AgentStore, onDone: () -> Unit) {
    var open by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var address by remember { mutableStateOf("https://") }
    var key by remember { mutableStateOf("") }
    var bridge by remember { mutableStateOf("") }
    var bridgeKey by remember { mutableStateOf("") }
    Text((if (open) "▾  " else "▸  ") + stringResource(R.string.enter_by_hand), style = Bip.text(17, FontWeight.ExtraBold),
        modifier = Modifier.tap { open = !open }.padding(vertical = 4.dp))
    if (!open) return
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Field(name, { name = it }, stringResource(R.string.name))
        Field(address, { address = it.trim() }, stringResource(R.string.agent_address))
        Field(key, { key = it.trim() }, stringResource(R.string.api_key), secret = true)
        Field(bridge, { bridge = it.trim() }, stringResource(R.string.bridge_address))
        Field(bridgeKey, { bridgeKey = it.trim() }, stringResource(R.string.bridge_key), secret = true)
        Text(stringResource(R.string.keys_stay_here), style = Bip.text(13, color = Bip.muted))
        val ready = name.isNotBlank() && Pairing.acceptable(address) && android.net.Uri.parse(address).host != null && key.isNotEmpty() &&
            (bridge.isBlank() || Pairing.acceptable(bridge))
        Box(Modifier.background(if (ready) Bip.ink else Bip.line, RoundedCornerShape(24.dp)).tap {
            if (!ready) return@tap
            store.add(io.github.bipagents.model.Agent(name = name.trim(), baseUrl = address, bridgeUrl = bridge.ifBlank { null },
                category = io.github.bipagents.model.Category.suggest(name).id),
                io.github.bipagents.model.AgentKeys(key, bridgeKey.ifBlank { null }))
            onDone()
        }.padding(horizontal = 22.dp, vertical = 12.dp)) {
            Text(stringResource(R.string.add), style = Bip.text(16, FontWeight.ExtraBold, Bip.card))
        }
    }
}
