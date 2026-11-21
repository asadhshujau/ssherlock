//! Opening a forward's listening sockets on this machine (FWD-08).
//!
//! "Localhost" listens on `127.0.0.1` and, when it can, `::1`, so a browser that tries IPv6
//! first for `localhost` still finds the port. "All interfaces" listens on `0.0.0.0` and, when
//! it can, `::`; on systems where `::` also takes IPv4 (Linux and macOS by default) that second
//! bind finds the port taken, and the IPv4 listener covers both anyway.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

use ssherlock_core::forwarding::{Bind, Listen};
use tokio::net::{TcpListener, TcpStream};

use crate::SshError;

/// A forward's listening sockets: one for IPv4, and one for IPv6 when the system allows it.
#[derive(Debug)]
pub(crate) struct Listeners {
    ipv4: TcpListener,
    ipv6: Option<TcpListener>,
}

impl Listeners {
    /// Listens on `listen`'s port. Only the IPv4 address is required; IPv6 is a bonus.
    ///
    /// # Errors
    ///
    /// [`SshError::Listen`] if the IPv4 address can't be bound (the port is in use, or is a
    /// privileged one).
    pub(crate) async fn bind(listen: Listen) -> Result<Self, SshError> {
        let port = listen.port.get();
        let (ipv4, ipv6) = match listen.bind {
            Bind::Localhost => (Ipv4Addr::LOCALHOST, Ipv6Addr::LOCALHOST),
            Bind::AllInterfaces => (Ipv4Addr::UNSPECIFIED, Ipv6Addr::UNSPECIFIED),
        };
        let ipv4 = SocketAddr::from((ipv4, port));
        let ipv4 = TcpListener::bind(ipv4)
            .await
            .map_err(|source| SshError::Listen {
                address: ipv4.to_string(),
                source,
            })?;
        let ipv6 = match TcpListener::bind(SocketAddr::from((ipv6, port))).await {
            Ok(listener) => Some(listener),
            Err(error) => {
                // IPv4 works, which is what most clients use; this only loses IPv6 clients.
                tracing::debug!(%error, "listening on IPv4 only");
                None
            }
        };
        Ok(Self { ipv4, ipv6 })
    }

    /// The next connection on either socket.
    pub(crate) async fn accept(&self) -> std::io::Result<(TcpStream, SocketAddr)> {
        let ipv6 = async {
            match &self.ipv6 {
                Some(listener) => listener.accept().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            accepted = self.ipv4.accept() => accepted,
            accepted = ipv6 => accepted,
        }
    }

    /// The IPv4 address listened on.
    #[cfg(test)]
    pub(crate) fn ipv4(&self) -> std::io::Result<SocketAddr> {
        self.ipv4.local_addr()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use ssherlock_core::Port;

    use super::*;

    /// A port that was free a moment ago.
    async fn free_port() -> Port {
        let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
        Port::new(probe.local_addr().unwrap().port()).unwrap()
    }

    #[tokio::test]
    async fn localhost_accepts_on_loopback() {
        let port = free_port().await;
        let listeners = Listeners::bind(Listen::localhost(port)).await.unwrap();
        assert_eq!(
            listeners.ipv4().unwrap(),
            SocketAddr::from((Ipv4Addr::LOCALHOST, port.get()))
        );
        let client = TcpStream::connect(("127.0.0.1", port.get()));
        let (accepted, client) = tokio::join!(listeners.accept(), client);
        let (_, peer) = accepted.unwrap();
        assert_eq!(peer, client.unwrap().local_addr().unwrap());
    }

    #[tokio::test]
    async fn all_interfaces_binds_the_unspecified_address() {
        let port = free_port().await;
        let listen = Listen {
            bind: Bind::AllInterfaces,
            port,
        };
        let listeners = Listeners::bind(listen).await.unwrap();
        assert!(listeners.ipv4().unwrap().ip().is_unspecified());
        let (accepted, client) = tokio::join!(
            listeners.accept(),
            TcpStream::connect(("127.0.0.1", port.get()))
        );
        assert!(accepted.is_ok() && client.is_ok());
    }

    #[tokio::test]
    async fn a_taken_port_is_an_error_naming_it() {
        let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = Port::new(taken.local_addr().unwrap().port()).unwrap();
        let error = Listeners::bind(Listen::localhost(port)).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("could not listen on 127.0.0.1:{port}")
        );
    }
}
