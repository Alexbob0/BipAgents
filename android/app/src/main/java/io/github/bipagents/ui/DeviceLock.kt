package io.github.bipagents.ui

import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity

/**
 * The phone's own lock (fingerprint, face or code) in front of what an agent holds: its live browser, the vault.
 * Without a screen lock on the phone, nothing opens (as on the iPhone without a passcode).
 */
object DeviceLock {
    private const val ALLOWED = BIOMETRIC_STRONG or DEVICE_CREDENTIAL

    fun available(activity: FragmentActivity) =
        BiometricManager.from(activity).canAuthenticate(ALLOWED) == BiometricManager.BIOMETRIC_SUCCESS

    /** Asks for it; `done(true)` once the person proved it's them. */
    fun ask(activity: FragmentActivity, title: String, done: (Boolean) -> Unit) {
        if (!available(activity)) return done(false)
        val prompt = BiometricPrompt(activity, ContextCompat.getMainExecutor(activity), object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) = done(true)
            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) = done(false)
        })
        prompt.authenticate(BiometricPrompt.PromptInfo.Builder().setTitle(title).setAllowedAuthenticators(ALLOWED).build())
    }
}
