use std::net::SocketAddr;
use std::{fmt, mem, result, str};

use base64::Engine;
use futures_util::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream as TokioTcpStream;
use tokio_util::codec::{Decoder, Framed};
use url::Url;
use websocket_codec::UpgradeCodec;

use crate::{AsyncClient, AsyncConnector, AsyncMaybeTlsStream, Connector, MessageCodec, Result};

fn replace_codec<T, C1, C2>(
	framed: Framed<T, C1>,
	codec: C2,
) -> Framed<T, C2>
where
	T: AsyncRead + AsyncWrite,
{
	// TODO improve this? https://github.com/tokio-rs/tokio/issues/717
	let parts1 = framed.into_parts();
	let mut parts2 = Framed::new(parts1.io, codec).into_parts();
	parts2.read_buf = parts1.read_buf;
	parts2.write_buf = parts1.write_buf;
	Framed::from_parts(parts2)
}

macro_rules! writeok {
    ($dst:expr, $($arg:tt)*) => {
        let _ = fmt::Write::write_fmt(&mut $dst, format_args!($($arg)*));
    }
}

fn resolve(url: &Url) -> Result<SocketAddr> {
	url.socket_addrs(|| None)?
		.into_iter()
		.next()
		.ok_or_else(|| "can't resolve host".to_owned().into())
}

fn make_key(
	key: Option<[u8; 16]>,
	key_base64: &mut [u8; 24],
) -> &str {
	let key_bytes = key.unwrap_or_else(rand::random);
	assert_eq!(
		24,
		base64::engine::GeneralPurpose::encode_slice(
			&base64::engine::general_purpose::STANDARD,
			&key_bytes,
			key_base64
		)
		.unwrap()
	);

	str::from_utf8(key_base64).unwrap()
}

fn build_request(
	url: &Url,
	key: &str,
	headers: &[(String, String)],
) -> String {
	let mut s = String::new();
	writeok!(
		s,
		"GET {path}",
		path = url.path()
	);
	if let Some(query) = url.query() {
		writeok!(s, "?{query}", query = query);
	}

	s += " HTTP/1.1\r\n";

	if let Some(host) = url.host() {
		writeok!(s, "Host: {host}", host = host);
		if let Some(port) = url.port_or_known_default() {
			writeok!(s, ":{port}", port = port);
		}

		s += "\r\n";
	}

	writeok!(
		s,
		"Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n",
		key = key
	);

	for (name, value) in headers {
		writeok!(
			s,
			"{name}: {value}\r\n",
			name = name,
			value = value
		);
	}

	writeok!(s, "\r\n");
	s
}

/// Establishes a WebSocket connection.
/// `ws://...` and `wss://...` URLs are supported.
pub struct ClientBuilder {
	url: Url,
	connector: Option<Connector>,
	async_connector: Option<AsyncConnector>,
	key: Option<[u8; 16]>,
	headers: Vec<(String, String)>,
}

impl ClientBuilder {
	/// Creates a `ClientBuilder` that connects to a given WebSocket URL.
	/// This method returns an `Err` result if URL parsing fails.
	pub fn new(url: &str) -> result::Result<Self, url::ParseError> {
		Ok(Self::from_url(Url::parse(
			url,
		)?))
	}

	/// Creates a `ClientBuilder` that connects to a given WebSocket URL.
	/// This method never fails as the URL has already been parsed.
	#[must_use]
	pub fn from_url(url: Url) -> Self {
		ClientBuilder {
			url,
			connector: None,
			async_connector: None,
			key: None,
			headers: Vec::new(),
		}
	}

	/// Sets the SSL connector for the `connect` method.
	/// By default, the client will create a new one for each connection instead of reusing one.
	pub fn set_connector(
		&mut self,
		connector: Connector,
	) -> Option<Connector> {
		mem::replace(
			&mut self.connector,
			Some(connector),
		)
	}

	/// Sets the SSL connector for the `async_connect` method.
	/// By default, the client will create a new one for each connection instead of reusing one.
	pub fn set_async_connector(
		&mut self,
		connector: AsyncConnector,
	) -> Option<AsyncConnector> {
		mem::replace(
			&mut self.async_connector,
			Some(connector),
		)
	}

	/// Adds an extra HTTP header for the client
	pub fn add_header(
		&mut self,
		name: String,
		value: String,
	) {
		self.headers.push((name, value));
	}

	/// Establishes a connection to the WebSocket server.
	///
	/// `wss://...` URLs are not supported by this method. Use `async_connect` if you need to be able to handle
	/// both `ws://...` and `wss://...` URLs.
	/// This method returns an `Err` result if connecting to the server fails.
	pub async fn async_connect_insecure(self) -> Result<AsyncClient<TokioTcpStream>> {
		let addr = resolve(&self.url)?;
		let stream = TokioTcpStream::connect(&addr).await?;
		self.async_connect_on(stream).await
	}

	/// Establishes a connection to the WebSocket server.
	/// This method returns an `Err` result if connecting to the server fails.
	pub async fn async_connect(mut self) -> Result<AsyncClient<AsyncMaybeTlsStream>> {
		let addr = resolve(&self.url)?;
		let stream = TokioTcpStream::connect(&addr).await?;

		let connector = if let Some(connector) = self.async_connector.take() {
			connector
		} else if self.url.scheme() == "wss" {
			AsyncConnector::new_with_default_tls_config()?
		} else {
			AsyncConnector::Plain
		};

		let domain = self.url.domain().unwrap_or("");
		let stream = connector.wrap(domain, stream).await?;

		self.async_connect_on(stream).await
	}

	/// Takes over an already established stream and uses it to send and receive WebSocket messages.
	///
	/// This method assumes that the TLS connection has already been established, if needed. It sends an HTTP
	/// `Connection: Upgrade` request and waits for an HTTP OK response before proceeding.
	/// This method returns an `Err` result if writing or reading from the stream fails.
	pub async fn async_connect_on<S: AsyncRead + AsyncWrite + Unpin>(
		self,
		mut stream: S,
	) -> Result<AsyncClient<S>> {
		let mut key_base64 = [0; 24];
		let key = make_key(self.key, &mut key_base64);
		let upgrade_codec = UpgradeCodec::new(key);
		let request = build_request(&self.url, key, &self.headers);
		AsyncWriteExt::write_all(
			&mut stream,
			request.as_bytes(),
		)
		.await?;

		let (opt, framed) = upgrade_codec.framed(stream).into_future().await;
		opt.ok_or_else(|| "no HTTP Upgrade response".to_owned())??;
		Ok(replace_codec(
			framed,
			MessageCodec::client(),
		))
	}
}
