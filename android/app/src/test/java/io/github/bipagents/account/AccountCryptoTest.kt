package io.github.bipagents.account

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

/** The reference vectors of guichet/tests/test_account_crypto.py: a box sealed there opens here, the code gives the
 *  same verifier and escrow key (AccountCrypto.swift is checked against the same); and the comparison code's. */
class AccountCryptoTest {
    private val mine = AccountCrypto.keyOf("02a1f3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f80",
        "BHeqNMs8tXzTJn4Kw8cAyxqlV64VcJz2XVJu2fKK4sZVkUzGEnG+4Vu/NIcI5+OUSaiJKrA19Uc8DQa+uka81Tw=")
    private val code = "K7QF9XZ2MA4D0N8HPQRS2345"

    @Test fun aBoxSealedByTheReferenceOpens() {
        val box = "BEDwgBlrHuZ8TnvKhh2U7SDIrwGHrpCAMy9wJd/TETDOR6Lwf60909X2mrl9/uxW2azK9e+1jVzp9frL/Qjf7WYAAQIDBAUGBwgJCgsvf1t4" +
            "CYfAtlMeq62JwgAiOSxQXoWSpMh1qJOndNQg/aQzWA=="
        assertEquals("""{"v":1,"places":[]}""", String(AccountCrypto.open(box, mine)))
    }

    @Test fun aBoxSealedHereOpensWithItsKeyOnly() {
        val sealed = AccountCrypto.seal("round trip".toByteArray(), mine.public)
        assertEquals("round trip", String(AccountCrypto.open(sealed, mine)))
        val other = AccountCrypto.newDeviceKey()
        assertEquals(true, runCatching { AccountCrypto.open(sealed, other) }.isFailure)
        assertEquals(other.public, AccountCrypto.DeviceKey.decode(other.encoded())!!.public)
    }

    @Test fun theRecoveryCodeGivesTheReferenceVerifierAndEscrowKey() {
        assertEquals("2988e87e594af57dbf98f9306da0406e6c6461cb381bb892b0377f4c123fbe4c", AccountCrypto.verifier(code))
        val escrow = "AAECAwQFBgcICQoLQFNTFAVmloWu1m/HwN3fn4Dy23WGaejdvzXmnP0D4xS9AfU="
        assertEquals("""{"v":1,"places":[]}""", String(AccountCrypto.openEscrow(escrow, AccountCrypto.escrowKey(code))))
        assertEquals(code, AccountCrypto.normalize("k7qf-9xz2-ma4d-on8h-pqrs-2345"))
        assertEquals("K7QF-9XZ2-MA4D-0N8H-PQRS-2345", AccountCrypto.shown(code))
        assertEquals(24, AccountCrypto.newRecoveryCode().length)
    }

    /** The comparison code's vector (AccountCrypto.swift asserts the same in debug builds). */
    @Test fun theComparisonCodeGivesTheReferenceDigits() {
        assertEquals("559 268 487 813", AccountCrypto.deviceCode("dev-0123456789abcdef", mine.public))
        assertNotEquals("559 268 487 813", AccountCrypto.deviceCode("dev-0123456789abcdeg", mine.public))
        assertEquals(null, AccountCrypto.deviceCode("dev-0123456789abcdef", "AAAA"))
    }
}
