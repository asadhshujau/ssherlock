//! Actions of the keychain. Each is bound in `keymap/default.json` under the key context of the
//! view it acts on: `KeyList`, `KeyEditor`, `IdentityList` or `IdentityEditor`. Moving the cursor reuses the hosts page's
//! `SelectNext` and friends, so lists move the same way everywhere.

use gpui::actions;

actions!(
    keychain,
    [
        /// Opens the form for generating a new key.
        GenerateKey,
        /// Opens the form for importing a key from a file or the clipboard.
        ImportKey,
        /// Lists the private keys in `~/.ssh` to import one.
        ImportFromSshDir,
        /// Saves the key under the cursor to a file, in a format to choose.
        ExportKey,
        /// Copies the public key under the cursor to the clipboard.
        CopyPublicKey,
        /// Edits the key under the cursor: its name and whether its passphrase is saved.
        EditKey,
        /// Deletes the key under the cursor, after asking.
        DeleteKey,
        /// Saves the key form.
        SaveKeyEditor,
        /// Closes the key form without saving.
        CancelKeyEditor,
        /// Opens the form for a new identity.
        NewIdentity,
        /// Edits the identity under the cursor.
        EditIdentity,
        /// Deletes the identity under the cursor, after asking.
        DeleteIdentity,
        /// Saves the identity form.
        SaveIdentityEditor,
        /// Closes the identity form without saving.
        CancelIdentityEditor,
    ]
);
