//! Types for holding secrets: passwords, passphrases, private keys and tokens.
//!
//! Re-exported from [`secrecy`] so every crate names them the same way. A secret is read only
//! through [`ExposeSecret::expose_secret`], which makes each read easy to find in review; its
//! `Debug` output is redacted and its memory is zeroed on drop.

pub use secrecy::{ExposeSecret, SecretBox, SecretString};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_output_never_contains_the_secret() {
        let secret = SecretString::from("hunter2-correct-horse");
        let bytes: SecretBox<[u8]> = SecretBox::new(Box::from(&b"hunter2-correct-horse"[..]));

        for rendered in [
            format!("{secret:?}"),
            format!("{secret:#?}"),
            format!("{bytes:?}"),
        ] {
            assert!(
                !rendered.contains("hunter2"),
                "secret leaked through Debug: {rendered}"
            );
        }
    }

    #[test]
    fn expose_secret_returns_the_value() {
        let secret = SecretString::from("s3cret");
        assert_eq!(secret.expose_secret(), "s3cret");
    }
}
