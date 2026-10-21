package io.github.bipagents.account

import java.math.BigInteger
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.MessageDigest
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.interfaces.ECPrivateKey
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPublicKeySpec
import java.security.spec.PKCS8EncodedKeySpec
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyAgreement
import javax.crypto.Mac
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/** The account's boxes and recovery code (AccountCrypto.swift's twin; the reference and its vectors:
 *  guichet/tests/test_account_crypto.py). A box is sealed by one of the person's devices for another's P-256 key and only
 *  that device opens it: the guichet passes it on without being able to read it.
 *
 *      box     ephemeral public key (65) · nonce (12) · AES-256-GCM ciphertext · tag, base64; its key is HKDF-SHA256
 *              over the ECDH secret, salt = ephemeral key · recipient's key, info « bipagents-box-v1 »
 *      code    24 characters of Crockford's alphabet; the guichet keeps a verifier of it, and the escrow (the places'
 *              backup keys) sealed with another key drawn from it
 *      check   a device's comparison code (deviceCode): the waiting device shows it from its own key, the approving one
 *              from the key the guichet gave; the same on both, or the guichet handed another key */
object AccountCrypto {
    const val ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
    private val random = SecureRandom()

    /** A device's key: its private part (PKCS#8, kept in Secrets) and its public one (uncompressed, base64). */
    class DeviceKey(val private: PrivateKey, val publicRaw: ByteArray) {
        val public: String get() = Base64.getEncoder().encodeToString(publicRaw)
        fun encoded(): String = Base64.getEncoder().encodeToString(private.encoded) + ":" + public

        companion object {
            fun decode(text: String): DeviceKey? = runCatching {
                val (priv, pub) = text.split(":", limit = 2)
                DeviceKey(KeyFactory.getInstance("EC").generatePrivate(PKCS8EncodedKeySpec(Base64.getDecoder().decode(priv))),
                    Base64.getDecoder().decode(pub))
            }.getOrNull()
        }
    }

    fun newDeviceKey(): DeviceKey {
        val pair = KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1"), random) }.generateKeyPair()
        return DeviceKey(pair.private, raw(pair.public as ECPublicKey))
    }

    private fun params(): ECParameterSpec =
        (KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair().public as ECPublicKey).params

    private fun pad32(value: BigInteger): ByteArray {
        val bytes = value.toByteArray().let { if (it.size > 32) it.copyOfRange(it.size - 32, it.size) else it }
        return ByteArray(32 - bytes.size) + bytes
    }

    fun raw(key: ECPublicKey): ByteArray = byteArrayOf(4) + pad32(key.w.affineX) + pad32(key.w.affineY)

    private fun publicKey(raw: ByteArray): ECPublicKey {
        require(raw.size == 65 && raw[0] == 4.toByte()) { "not a P-256 key" }
        val point = ECPoint(BigInteger(1, raw.copyOfRange(1, 33)), BigInteger(1, raw.copyOfRange(33, 65)))
        return KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(point, params())) as ECPublicKey
    }

    private fun hkdf(ikm: ByteArray, salt: ByteArray, info: ByteArray): ByteArray {
        val extract = Mac.getInstance("HmacSHA256").apply { init(SecretKeySpec(salt, "HmacSHA256")) }.doFinal(ikm)
        return Mac.getInstance("HmacSHA256").apply { init(SecretKeySpec(extract, "HmacSHA256")) }.doFinal(info + byteArrayOf(1))
    }

    private fun ecdh(private: PrivateKey, public: ECPublicKey): ByteArray =
        KeyAgreement.getInstance("ECDH").apply { init(private); doPhase(public, true) }.generateSecret()

    private fun gcm(mode: Int, key: ByteArray, nonce: ByteArray, data: ByteArray): ByteArray =
        Cipher.getInstance("AES/GCM/NoPadding").apply { init(mode, SecretKeySpec(key, "AES"), GCMParameterSpec(128, nonce)) }
            .doFinal(data)

    fun seal(plaintext: ByteArray, recipient: String): String {
        val recipientRaw = Base64.getDecoder().decode(recipient)
        val ephemeral = newDeviceKey()
        val key = hkdf(ecdh(ephemeral.private, publicKey(recipientRaw)), ephemeral.publicRaw + recipientRaw, "bipagents-box-v1".toByteArray())
        val nonce = ByteArray(12).also { random.nextBytes(it) }
        return Base64.getEncoder().encodeToString(ephemeral.publicRaw + nonce + gcm(Cipher.ENCRYPT_MODE, key, nonce, plaintext))
    }

    fun open(box: String, mine: DeviceKey): ByteArray {
        val raw = Base64.getDecoder().decode(box)
        require(raw.size > 65 + 12 + 16) { "not a box" }
        val ephemeral = raw.copyOfRange(0, 65)
        val key = hkdf(ecdh(mine.private, publicKey(ephemeral)), ephemeral + mine.publicRaw, "bipagents-box-v1".toByteArray())
        return gcm(Cipher.DECRYPT_MODE, key, raw.copyOfRange(65, 77), raw.copyOfRange(77, raw.size))
    }

    /** A key made from a private scalar (the tests' vectors). */
    fun keyOf(privateHex: String, publicB64: String): DeviceKey {
        val spec = java.security.spec.ECPrivateKeySpec(BigInteger(privateHex, 16), params())
        val private = KeyFactory.getInstance("EC").generatePrivate(spec) as ECPrivateKey
        return DeviceKey(private, Base64.getDecoder().decode(publicB64))
    }

    // -- the comparison code -----------------------------------------------------------------------------------------

    private const val CODE_ROUNDS = 10_000

    /** The code a person compares between their waiting device and the one approving it, like messaging apps' safety
     *  numbers: SHA-256 over « bipagents-device-code-v1 » · 0x00 · the device's id at the guichet (UTF-8) · 0x00 · its
     *  public key (65 bytes, uncompressed), hashed again 9,999 times (SHA-256 of the previous 32 bytes); the first 8
     *  bytes, big-endian, modulo 10¹² give 12 digits, shown « 123 456 789 012 ». Twelve digits and the repeated hash,
     *  because a dishonest guichet knows the real key and could otherwise try keys of its own until one gives the same
     *  code. Null for a key that is not one. */
    fun deviceCode(device: String, key: String): String? {
        val raw = runCatching { Base64.getDecoder().decode(key) }.getOrNull()
            ?.takeIf { it.size == 65 && it[0] == 4.toByte() } ?: return null
        val sha = MessageDigest.getInstance("SHA-256")
        var digest = sha.digest("bipagents-device-code-v1".toByteArray() + byteArrayOf(0) + device.toByteArray() + byteArrayOf(0) + raw)
        repeat(CODE_ROUNDS - 1) { digest = sha.digest(digest) }
        val number = digest.copyOfRange(0, 8).fold(BigInteger.ZERO) { acc, b -> acc.shiftLeft(8).or(BigInteger.valueOf((b.toInt() and 0xff).toLong())) }
            .mod(BigInteger.TEN.pow(12))
        return number.toString().padStart(12, '0').chunked(3).joinToString(" ")
    }

    // -- the recovery code --------------------------------------------------------------------------------------------

    fun newRecoveryCode(): String = (1..24).map { ALPHABET[random.nextInt(ALPHABET.length)] }.joinToString("")

    fun shown(code: String): String = code.chunked(4).joinToString("-")

    /** What the person typed, as the code: capitals, O read as 0 and I or L as 1, nothing else. */
    fun normalize(typed: String): String = typed.uppercase()
        .map { when (it) { 'O' -> '0'; 'I', 'L' -> '1'; else -> it } }.filter { it in ALPHABET }.joinToString("")

    private fun drawn(code: String, info: String) = hkdf(code.toByteArray(), "bipagents-recovery".toByteArray(), info.toByteArray())

    /** What the guichet keeps to check the code (never the code). */
    fun verifier(code: String): String = drawn(code, "verifier").joinToString("") { "%02x".format(it) }

    /** The escrow's key (kept on the person's approved devices, to seal the escrow again when places change). */
    fun escrowKey(code: String): ByteArray = drawn(code, "escrow")

    fun sealEscrow(plaintext: ByteArray, key: ByteArray): String {
        val nonce = ByteArray(12).also { random.nextBytes(it) }
        return Base64.getEncoder().encodeToString(nonce + gcm(Cipher.ENCRYPT_MODE, key, nonce, plaintext))
    }

    fun openEscrow(escrow: String, key: ByteArray): ByteArray {
        val raw = Base64.getDecoder().decode(escrow)
        return gcm(Cipher.DECRYPT_MODE, key, raw.copyOfRange(0, 12), raw.copyOfRange(12, raw.size))
    }
}
