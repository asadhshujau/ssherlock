//! The server side of a SOCKS5 handshake (RFC 1928), for dynamic forwards (FWD-03).
//!
//! Only what `ssh -D` needs: no authentication and the `CONNECT` command, to an IPv4 address,
//! an IPv6 address or a host name. Names are passed on unresolved, so the server resolves them,
//! as a browser's "proxy DNS when using SOCKS v5" expects.

use std::net::{Ipv4Addr, Ipv6Addr};

use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::SshError;

const VERSION: u8 = 5;
const NO_AUTHENTICATION: u8 = 0x00;
const NO_ACCEPTABLE_METHOD: u8 = 0xff;
const CONNECT: u8 = 0x01;
const ATYP_IPV4: u8 = 0x01;
const ATYP_NAME: u8 = 0x03;
const ATYP_IPV6: u8 = 0x04;

/// Where a SOCKS client asked to connect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    /// An IP address as text (IPv6 without brackets) or a host name.
    pub host: String,
    /// The port, never 0.
    pub port: u16,
}

/// The answer to a `CONNECT` request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reply {
    /// Connected; the stream now carries the forwarded connection.
    Succeeded = 0x00,
    /// The server couldn't open the connection, for a reason SOCKS has no code for.
    GeneralFailure = 0x01,
    /// The command isn't `CONNECT`.
    CommandNotSupported = 0x07,
    /// The address type isn't one SOCKS5 defines.
    AddressTypeNotSupported = 0x08,
}

fn refused(reason: &'static str) -> SshError {
    SshError::SocksRequest { reason }
}

fn io(error: std::io::Error) -> SshError {
    SshError::ForwardIo {
        what: "read the SOCKS request",
        source: error,
    }
}

/// Reads a client's greeting and `CONNECT` request and returns where it wants to go. The
/// caller then answers with [`reply`]. Requests that can't be served are answered here, as
/// the protocol asks, before the error is returned.
///
/// # Errors
///
/// [`SshError::SocksRequest`] for anything but a SOCKS5 `CONNECT` without authentication,
/// [`SshError::ForwardIo`] if the client goes away.
pub(crate) async fn handshake<S>(stream: &mut S) -> Result<Target, SshError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut header = [0_u8; 2];
    stream.read_exact(&mut header).await.map_err(io)?;
    let [version, method_count] = header;
    if version == 4 {
        return Err(refused("SOCKS4 is not supported; use SOCKS5"));
    }
    if version != VERSION {
        return Err(refused("not a SOCKS5 greeting"));
    }
    let mut methods = vec![0_u8; usize::from(method_count)];
    stream.read_exact(&mut methods).await.map_err(io)?;
    if !methods.contains(&NO_AUTHENTICATION) {
        // Answered, so the client can tell the user why, and then closed.
        let _ = stream.write_all(&[VERSION, NO_ACCEPTABLE_METHOD]).await;
        return Err(refused("the client requires authentication"));
    }
    stream
        .write_all(&[VERSION, NO_AUTHENTICATION])
        .await
        .map_err(io)?;

    let mut request = [0_u8; 4];
    stream.read_exact(&mut request).await.map_err(io)?;
    let [version, command, _reserved, address_type] = request;
    if version != VERSION {
        return Err(refused("not a SOCKS5 request"));
    }
    if command != CONNECT {
        let _ = reply(stream, Reply::CommandNotSupported).await;
        return Err(refused("only CONNECT is supported"));
    }
    let host = match address_type {
        ATYP_IPV4 => {
            let mut octets = [0_u8; 4];
            stream.read_exact(&mut octets).await.map_err(io)?;
            Ipv4Addr::from(octets).to_string()
        }
        ATYP_IPV6 => {
            let mut octets = [0_u8; 16];
            stream.read_exact(&mut octets).await.map_err(io)?;
            Ipv6Addr::from(octets).to_string()
        }
        ATYP_NAME => {
            let length = stream.read_u8().await.map_err(io)?;
            let mut name = vec![0_u8; usize::from(length)];
            stream.read_exact(&mut name).await.map_err(io)?;
            match String::from_utf8(name) {
                Ok(name) if !name.is_empty() => name,
                _ => {
                    let _ = reply(stream, Reply::GeneralFailure).await;
                    return Err(refused("the host name is empty or not text"));
                }
            }
        }
        _ => {
            let _ = reply(stream, Reply::AddressTypeNotSupported).await;
            return Err(refused("unknown address type"));
        }
    };
    let port = stream.read_u16().await.map_err(io)?;
    if port == 0 {
        let _ = reply(stream, Reply::GeneralFailure).await;
        return Err(refused("port 0 is not a destination"));
    }
    Ok(Target { host, port })
}

/// Answers a `CONNECT` request. The bound address is reported as `0.0.0.0:0`: the real one is
/// on the server, and clients don't use it.
///
/// # Errors
///
/// [`SshError::ForwardIo`] if the client has gone away.
pub(crate) async fn reply<S>(stream: &mut S, reply: Reply) -> Result<(), SshError>
where
    S: AsyncWrite + Unpin,
{
    stream
        .write_all(&[VERSION, reply as u8, 0, ATYP_IPV4, 0, 0, 0, 0, 0, 0])
        .await
        .map_err(|source| SshError::ForwardIo {
            what: "answer the SOCKS request",
            source,
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use pretty_assertions::assert_eq;
    use tokio::io::duplex;

    use super::*;

    /// Runs the handshake on `input`; returns its outcome and what it wrote back.
    async fn run(input: &[u8]) -> (Result<Target, String>, Vec<u8>) {
        let (mut client, mut server) = duplex(1024);
        client.write_all(input).await.unwrap();
        // Ends the client's writing, so a short request fails instead of waiting.
        client.shutdown().await.unwrap();
        let outcome = handshake(&mut server).await.map_err(|e| e.to_string());
        drop(server);
        let mut written = Vec::new();
        client.read_to_end(&mut written).await.unwrap();
        (outcome, written)
    }

    const ACCEPT: [u8; 2] = [5, 0];

    /// Name, bytes from the client, outcome, bytes written back.
    type Case = (&'static str, Vec<u8>, Result<Target, String>, Vec<u8>);

    fn failure(code: u8) -> Vec<u8> {
        let mut bytes = ACCEPT.to_vec();
        bytes.extend([5, code, 0, 1, 0, 0, 0, 0, 0, 0]);
        bytes
    }

    #[tokio::test]
    async fn handshakes() {
        let target = |host: &str, port| {
            Ok(Target {
                host: host.to_owned(),
                port,
            })
        };
        let cases: Vec<Case> = vec![
            (
                "IPv4",
                vec![5, 1, 0, 5, 1, 0, 1, 10, 0, 0, 5, 0x15, 0x38],
                target("10.0.0.5", 5432),
                ACCEPT.to_vec(),
            ),
            (
                "IPv6, with other methods offered too",
                [
                    vec![5, 2, 2, 0, 5, 1, 0, 4],
                    Ipv6Addr::LOCALHOST.octets().to_vec(),
                    vec![0, 80],
                ]
                .concat(),
                target("::1", 80),
                ACCEPT.to_vec(),
            ),
            (
                "host name",
                [
                    vec![5, 1, 0, 5, 1, 0, 3, 11],
                    b"example.com".to_vec(),
                    vec![1, 187],
                ]
                .concat(),
                target("example.com", 443),
                ACCEPT.to_vec(),
            ),
            (
                "SOCKS4",
                vec![4, 1, 0, 80, 127, 0, 0, 1, 0],
                Err("refused a SOCKS request: SOCKS4 is not supported; use SOCKS5".into()),
                vec![],
            ),
            (
                "not SOCKS at all",
                b"GET / HTTP/1.1\r\n".to_vec(),
                Err("refused a SOCKS request: not a SOCKS5 greeting".into()),
                vec![],
            ),
            (
                "authentication only",
                vec![5, 1, 2],
                Err("refused a SOCKS request: the client requires authentication".into()),
                vec![5, 0xff],
            ),
            (
                "BIND",
                vec![5, 1, 0, 5, 2, 0, 1, 10, 0, 0, 5, 0, 80],
                Err("refused a SOCKS request: only CONNECT is supported".into()),
                failure(7),
            ),
            (
                "unknown address type",
                vec![5, 1, 0, 5, 1, 0, 9],
                Err("refused a SOCKS request: unknown address type".into()),
                failure(8),
            ),
            (
                "empty host name",
                vec![5, 1, 0, 5, 1, 0, 3, 0, 0, 80],
                Err("refused a SOCKS request: the host name is empty or not text".into()),
                failure(1),
            ),
            (
                "port 0",
                vec![5, 1, 0, 5, 1, 0, 1, 10, 0, 0, 5, 0, 0],
                Err("refused a SOCKS request: port 0 is not a destination".into()),
                failure(1),
            ),
            (
                "cut short",
                vec![5, 1, 0, 5, 1, 0, 1, 10],
                Err("could not read the SOCKS request".into()),
                ACCEPT.to_vec(),
            ),
        ];
        for (name, input, expected, written) in cases {
            let (outcome, got) = run(&input).await;
            assert_eq!(outcome, expected, "{name}");
            assert_eq!(got, written, "{name}");
        }
    }

    #[tokio::test]
    async fn replies() {
        let mut out = Vec::new();
        reply(&mut out, Reply::Succeeded).await.unwrap();
        assert_eq!(out, [5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    }
}
