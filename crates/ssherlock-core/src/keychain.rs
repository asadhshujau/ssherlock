//! Saved keys and identities (KEY-01…03, KEY-14).
//!
//! **Model** layer of the keychain feature. A [`Key`] is an SSH key pair whose private half is
//! sealed by the vault; an [`Identity`] is a user name with a password, a key or both, that
//! hosts and groups pick instead of repeating credentials. Nothing here parses or decrypts
//! keys: `ssherlock-ssh` reads key files and `ssherlock-store`'s vault opens secrets.

mod identity;
mod key;
mod usage;

pub use identity::{Identity, IdentityError};
pub use key::{EcdsaCurve, Key, KeyError, KeyKind, KeyKindError, KeyProtection};
pub use usage::{Usage, usage_of_identity, usage_of_key};

use crate::hosts::MAX_NAME_LEN;

/// Checks a key or identity label: not too long, one line.
fn check_label(label: &str) -> Result<(), LabelError> {
    if label.chars().count() > MAX_NAME_LEN {
        return Err(LabelError::TooLong);
    }
    if label.chars().any(char::is_control) {
        return Err(LabelError::Control);
    }
    Ok(())
}

/// What's wrong with a label.
enum LabelError {
    TooLong,
    Control,
}
