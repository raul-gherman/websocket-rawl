//! A fast, low-overhead WebSocket client.

mod client;
mod ssl;

pub use crate::client::ClientBuilder;
pub use crate::ssl::{AsyncConnector, AsyncMaybeTlsStream, Connector};
pub use websocket_codec::{CloseCode, CloseFrame, Error, Message, MessageCodec, Opcode, Result};

use tokio_util::codec::Framed;

/// Exposes a `Sink` and a `Stream` for sending and receiving WebSocket messages asynchronously.
pub type AsyncClient<S> = Framed<S, MessageCodec>;
