import CryptoKit
import Foundation

/// The account's boxes and recovery code (the reference and its vectors: guichet/tests/test_account_crypto.py; the
/// Android app's AccountCrypto.kt is the same). A box is sealed by one of the person's devices for another's P-256 key,
/// and only that device opens it: the guichet passes it on without being able to read it.
///
///     box     ephemeral public key (65) · nonce (12) · AES-256-GCM ciphertext · tag, base64; its key is HKDF-SHA256
///             over the ECDH secret, salt = ephemeral key · recipient's key, info « bipagents-box-v1 »
///     code    24 characters of Crockford's alphabet, shown in groups of four; the guichet keeps only a verifier of it,
///             and the escrow (the places' backup keys) sealed with another key drawn from it
nonisolated enum AccountCrypto {
    static let alphabet = Array("0123456789ABCDEFGHJKMNPQRSTVWXYZ")

    enum CryptoError: LocalizedError {
        case badBox, badKey
        var errorDescription: String? {
            String(localized: "Ce paquet n’a pas pu être ouvert sur cet appareil.")
        }
    }

    // MARK: this device's key

    /// This device's own key (made once, kept in the Keychain, this device only).
    static func deviceKey() -> P256.KeyAgreement.PrivateKey {
        if let raw = OfferVault.load(Data.self, "accountDeviceKey"),
           let key = try? P256.KeyAgreement.PrivateKey(rawRepresentation: raw) {
            return key
        }
        let key = P256.KeyAgreement.PrivateKey()
        OfferVault.store(key.rawRepresentation, "accountDeviceKey")
        return key
    }

    static func publicKey(of key: P256.KeyAgreement.PrivateKey) -> String {
        key.publicKey.x963Representation.base64EncodedString()
    }

    // MARK: boxes

    static func seal(_ plaintext: Data, for recipient: String) throws -> String {
        guard let raw = Data(base64Encoded: recipient),
              let other = try? P256.KeyAgreement.PublicKey(x963Representation: raw) else { throw CryptoError.badKey }
        let ephemeral = P256.KeyAgreement.PrivateKey()
        let shared = try ephemeral.sharedSecretFromKeyAgreement(with: other)
        let key = boxKey(shared, ephemeral: ephemeral.publicKey.x963Representation, recipient: raw)
        let sealed = try AES.GCM.seal(plaintext, using: key)
        guard let combined = sealed.combined else { throw CryptoError.badBox }
        return (ephemeral.publicKey.x963Representation + combined).base64EncodedString()
    }

    static func open(_ box: String, with key: P256.KeyAgreement.PrivateKey) throws -> Data {
        guard let raw = Data(base64Encoded: box), raw.count > 65 + 12 + 16 else { throw CryptoError.badBox }
        let ephemeralRaw = raw.prefix(65)
        guard let ephemeral = try? P256.KeyAgreement.PublicKey(x963Representation: ephemeralRaw) else {
            throw CryptoError.badBox
        }
        let shared = try key.sharedSecretFromKeyAgreement(with: ephemeral)
        let symmetric = boxKey(shared, ephemeral: Data(ephemeralRaw), recipient: key.publicKey.x963Representation)
        return try AES.GCM.open(try AES.GCM.SealedBox(combined: raw.dropFirst(65)), using: symmetric)
    }

    private static func boxKey(_ shared: SharedSecret, ephemeral: Data, recipient: Data) -> SymmetricKey {
        shared.hkdfDerivedSymmetricKey(using: SHA256.self, salt: ephemeral + recipient,
                                       sharedInfo: Data("bipagents-box-v1".utf8), outputByteCount: 32)
    }

    // MARK: recovery code

    /// A new recovery code: 24 characters (120 bits), shown in six groups of four.
    static func newRecoveryCode() -> String {
        String((0..<24).map { _ in alphabet[Int.random(in: 0..<alphabet.count)] })
    }

    static func shown(_ code: String) -> String {
        stride(from: 0, to: code.count, by: 4).map { start in
            let from = code.index(code.startIndex, offsetBy: start)
            return String(code[from..<code.index(from, offsetBy: min(4, code.count - start))])
        }.joined(separator: "-")
    }

    /// What the person typed, as the code: capitals, O read as 0 and I or L as 1, nothing else.
    static func normalize(_ typed: String) -> String {
        String(typed.uppercased().map { $0 == "O" ? "0" : ($0 == "I" || $0 == "L") ? "1" : $0 }.filter { alphabet.contains($0) })
    }

    private static func drawn(_ code: String, _ info: String) -> SymmetricKey {
        HKDF<SHA256>.deriveKey(inputKeyMaterial: SymmetricKey(data: Data(code.utf8)),
                               salt: Data("bipagents-recovery".utf8), info: Data(info.utf8), outputByteCount: 32)
    }

    /// What the guichet keeps to check the code (never the code).
    static func verifier(_ code: String) -> String {
        drawn(code, "verifier").withUnsafeBytes { Data($0) }.map { String(format: "%02x", $0) }.joined()
    }

    /// The key of the escrow (kept on the person's approved devices, to seal the escrow again when places change).
    static func escrowKey(_ code: String) -> Data {
        drawn(code, "escrow").withUnsafeBytes { Data($0) }
    }

    static func sealEscrow(_ plaintext: Data, key: Data) throws -> String {
        guard let combined = try AES.GCM.seal(plaintext, using: SymmetricKey(data: key)).combined else {
            throw CryptoError.badBox
        }
        return combined.base64EncodedString()
    }

    static func openEscrow(_ escrow: String, key: Data) throws -> Data {
        guard let raw = Data(base64Encoded: escrow) else { throw CryptoError.badBox }
        return try AES.GCM.open(try AES.GCM.SealedBox(combined: raw), using: SymmetricKey(data: key))
    }
}
