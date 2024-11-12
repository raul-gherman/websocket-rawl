use std::fmt::{Debug, Formatter};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::{fmt, io};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream as TokioTcpStream;

use crate::Result;

/// A reusable TLS connector for wrapping streams.
#[derive(Clone)]
pub enum Connector {
	/// Plain (non-TLS) connector.
	Plain,
	/// `native-tls` TLS connector.
	NativeTls(native_tls::TlsConnector),
}

impl Debug for Connector {
	fn fmt(
		&self,
		f: &mut Formatter<'_>,
	) -> fmt::Result {
		match self {
			Self::Plain => f.write_str("Connector::Plain"),
			Self::NativeTls(connector) => connector.fmt(f),
		}
	}
}

/// A reusable TLS connector for wrapping streams.
#[derive(Clone)]
pub enum AsyncConnector {
	/// Plain (non-TLS) connector.
	Plain,
	/// `native-tls` async TLS connector.
	NativeTls(tokio_native_tls::TlsConnector),
}

impl Debug for AsyncConnector {
	fn fmt(
		&self,
		f: &mut Formatter<'_>,
	) -> fmt::Result {
		match self {
			Self::Plain => f.write_str("AsyncConnector::Plain"),
			Self::NativeTls(connector) => connector.fmt(f),
		}
	}
}

// #[allow(clippy::large_enum_variant)]
enum AsyncMaybeTlsStreamInner {
	Plain(TokioTcpStream),
	NativeTls(tokio_native_tls::TlsStream<TokioTcpStream>),
}

/// An async stream that might be protected with TLS.
pub struct AsyncMaybeTlsStream {
	inner: AsyncMaybeTlsStreamInner,
}

impl AsyncRead for AsyncMaybeTlsStream {
	fn poll_read(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &mut ReadBuf<'_>,
	) -> Poll<io::Result<()>> {
		match &mut self.get_mut().inner {
			AsyncMaybeTlsStreamInner::Plain(ref mut s) => Pin::new(s).poll_read(cx, buf),
			AsyncMaybeTlsStreamInner::NativeTls(s) => Pin::new(s).poll_read(cx, buf),
		}
	}
}

impl AsyncWrite for AsyncMaybeTlsStream {
	fn poll_write(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<io::Result<usize>> {
		match &mut self.get_mut().inner {
			AsyncMaybeTlsStreamInner::Plain(ref mut s) => Pin::new(s).poll_write(cx, buf),
			AsyncMaybeTlsStreamInner::NativeTls(s) => Pin::new(s).poll_write(cx, buf),
		}
	}

	fn poll_flush(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<io::Result<()>> {
		match &mut self.get_mut().inner {
			AsyncMaybeTlsStreamInner::Plain(ref mut s) => Pin::new(s).poll_flush(cx),
			AsyncMaybeTlsStreamInner::NativeTls(s) => Pin::new(s).poll_flush(cx),
		}
	}

	fn poll_shutdown(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
	) -> Poll<io::Result<()>> {
		match &mut self.get_mut().inner {
			AsyncMaybeTlsStreamInner::Plain(ref mut s) => Pin::new(s).poll_shutdown(cx),
			AsyncMaybeTlsStreamInner::NativeTls(s) => Pin::new(s).poll_shutdown(cx),
		}
	}
}

impl Connector {
	/// Creates a new `Connector` with the underlying TLS library specified in the feature flags.
	/// This method returns an `Err` when creating the underlying TLS connector fails.
	// #[allow(clippy::unnecessary_wraps)]
	pub fn new_with_default_tls_config() -> Result<Self> {
		Ok(Self::NativeTls(
			native_tls::TlsConnector::new()?,
		))
	}
}

impl AsyncConnector {
	/// Creates a new async `Connector` with the underlying TLS library specified in the feature flags.
	/// This method returns an `Err` when creating the underlying TLS connector fails.
	// #[allow(clippy::unnecessary_wraps)]
	pub fn new_with_default_tls_config() -> Result<Self> {
		Ok(Self::NativeTls(
			native_tls::TlsConnector::new()?.into(),
		))
	}

	// #[allow(clippy::match_wildcard_for_single_variants)]
	// #[allow(clippy::unnecessary_wraps)]
	// #[allow(unused_variables)]
	pub(crate) async fn wrap(
		self,
		domain: &str,
		stream: TokioTcpStream,
	) -> Result<AsyncMaybeTlsStream> {
		let inner = match self {
			Self::Plain => AsyncMaybeTlsStreamInner::Plain(stream),
			Self::NativeTls(connector) => AsyncMaybeTlsStreamInner::NativeTls(connector.connect(domain, stream).await?),
		};

		Ok(AsyncMaybeTlsStream { inner })
	}
}
