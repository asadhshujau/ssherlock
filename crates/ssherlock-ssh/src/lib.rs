//! SSH sessions: connection, authentication, channels, port forwarding and SFTP.
//!
//! This is a **service** crate. It talks to the UI only through events sent over channels.
#![forbid(unsafe_code)]
