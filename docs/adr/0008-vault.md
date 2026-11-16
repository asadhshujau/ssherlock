# 0008 — The vault: one master key, sealed values, recorded key location

**Status:** accepted · 2026-11-16

## Context
Phase 4 saves passwords, passphrases and private keys (KEY-01, KEY-02, SEC-01). They must never
reach the database or the logs in plaintext, and the app must keep working on Linux sessions
with no Secret Service (headless, minimal window managers, CI).

## Decision
`ssherlock-store::vault` seals each secret with XChaCha20-Poly1305 under one random 256-bit
master key: `0x01 ‖ 24-byte random nonce ‖ ciphertext+tag`, with a fixed AAD. XChaCha's long
nonce makes random nonces safe without a counter. Models carry the result as
`ssherlock_core::Sealed` (opaque bytes, `Debug` shows only the length), so listing, copying and
storing a record never decrypts it; only `Vault::open` does, at the moment of use.

The master key lives in the OS keychain (`keyring`: macOS Keychain, Secret Service on Linux),
one entry per database named after a random vault id. If saving it there fails or doesn't read
back, it goes in `vault.key` beside the database, created `0600` and never overwritten. A
`vault` table records the vault id, which of the two places holds the key, and a sealed check
value.

## Consequences
- A later start uses the recorded place only. A keychain that is locked or briefly missing is
  an error the user sees, never a reason to make a second key that orphans the first.
- A wrong or replaced key is caught by the check value before any secret is opened.
- The key file is weaker than a keychain (anyone who can read the user's files can read it);
  the UI says so when it is in use. A master password (SEC-02) can wrap the key later.
- Tests use `MemoryKeyHolder` and never touch the real keychain.
