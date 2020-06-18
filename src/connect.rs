//! Connection helper.
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use tungstenite::client::uri_mode;
use tungstenite::handshake::client::Response;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::Error;

use super::{client_async_with_config, IntoClientRequest, Request, WebSocketStream};

#[cfg(feature = "tls")]
pub(crate) mod encryption {
    use native_tls::TlsConnector;
    use tokio_tls::{TlsConnector as TokioTlsConnector, TlsStream};

    use tokio::io::{AsyncRead, AsyncWrite};

    use tungstenite::stream::Mode;
    use tungstenite::Error;

    use crate::stream::Stream as StreamSwitcher;

    /// A stream that might be protected with TLS.
    pub type MaybeTlsStream<S> = StreamSwitcher<S, TlsStream<S>>;

    pub type AutoStream<S> = MaybeTlsStream<S>;

    pub async fn wrap_stream<S>(
        socket: S,
        domain: String,
        mode: Mode,
        accept_invalid_certs: bool,
    ) -> Result<AutoStream<S>, Error>
    where
        S: 'static + AsyncRead + AsyncWrite + Send + Unpin,
    {
        match mode {
            Mode::Plain => Ok(StreamSwitcher::Plain(socket)),
            Mode::Tls => {
                let try_connector = TlsConnector::builder()
                    .danger_accept_invalid_certs(accept_invalid_certs)
                    .build();

                let connector = try_connector.map_err(Error::Tls)?;
                let stream = TokioTlsConnector::from(connector);
                let connected = stream.connect(&domain, socket).await;
                match connected {
                    Err(e) => Err(Error::Tls(e)),
                    Ok(s) => Ok(StreamSwitcher::Tls(s)),
                }
            }
        }
    }
}

#[cfg(feature = "tls")]
pub use self::encryption::MaybeTlsStream;

#[cfg(not(feature = "tls"))]
pub(crate) mod encryption {
    use tokio::io::{AsyncRead, AsyncWrite};

    use tungstenite::stream::Mode;
    use tungstenite::Error;

    pub type AutoStream<S> = S;

    pub async fn wrap_stream<S>(
        socket: S,
        _domain: String,
        mode: Mode,
        _accept_invalid_certs: bool,
    ) -> Result<AutoStream<S>, Error>
    where
        S: 'static + AsyncRead + AsyncWrite + Send + Unpin,
    {
        match mode {
            Mode::Plain => Ok(socket),
            Mode::Tls => Err(Error::Url("TLS support not compiled in.".into())),
        }
    }
}

use self::encryption::{wrap_stream, AutoStream};

/// Get a domain from an URL.
#[inline]
fn domain(request: &Request) -> Result<String, Error> {
    match request.uri().host() {
        Some(d) => Ok(d.to_string()),
        None => Err(Error::Url("no host name in the url".into())),
    }
}

/// Creates a WebSocket handshake from a request and a stream,
/// upgrading the stream to TLS if required.
pub async fn client_async_tls<R, S>(
    request: R,
    stream: S,
    accept_invalid_certs: bool,
) -> Result<(WebSocketStream<AutoStream<S>>, Response), Error>
where
    R: IntoClientRequest + Unpin,
    S: 'static + AsyncRead + AsyncWrite + Send + Unpin,
    AutoStream<S>: Unpin,
{
    client_async_tls_with_config(request, stream, None, accept_invalid_certs).await
}

/// The same as `client_async_tls()` but the one can specify a websocket configuration.
/// Please refer to `client_async_tls()` for more details.
pub async fn client_async_tls_with_config<R, S>(
    request: R,
    stream: S,
    config: Option<WebSocketConfig>,
    accept_invalid_certs: bool,
) -> Result<(WebSocketStream<AutoStream<S>>, Response), Error>
where
    R: IntoClientRequest + Unpin,
    S: 'static + AsyncRead + AsyncWrite + Send + Unpin,
    AutoStream<S>: Unpin,
{
    let request = request.into_client_request()?;

    let domain = domain(&request)?;

    // Make sure we check domain and mode first. URL must be valid.
    let mode = uri_mode(&request.uri())?;

    let stream = wrap_stream(stream, domain, mode, accept_invalid_certs).await?;
    client_async_with_config(request, stream, config).await
}

/// Connect to a given URL.
pub async fn connect_async<R>(
    request: R,
) -> Result<(WebSocketStream<AutoStream<TcpStream>>, Response), Error>
where
    R: IntoClientRequest + Unpin,
{
    connect_async_with_config(request, None).await
}

/// The same as `connect_async()` but the one can specify a websocket configuration.
/// Please refer to `connect_async()` for more details.
pub async fn connect_async_with_config<R>(
    request: R,
    config: Option<WebSocketConfig>,
) -> Result<(WebSocketStream<AutoStream<TcpStream>>, Response), Error>
where
    R: IntoClientRequest + Unpin,
{
    let request = request.into_client_request()?;

    let domain = domain(&request)?;
    let port = request
        .uri()
        .port_u16()
        .or_else(|| match request.uri().scheme_str() {
            Some("wss") => Some(443),
            Some("ws") => Some(80),
            _ => None,
        })
        .ok_or_else(|| Error::Url("Url scheme not supported".into()))?;

    let addr = format!("{}:{}", domain, port);
    let try_socket = TcpStream::connect(addr).await;
    let socket = try_socket.map_err(Error::Io)?;
    client_async_tls_with_config(request, socket, config, false).await
}
