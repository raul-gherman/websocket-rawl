use crate::close::{
	CloseCode,
	CloseFrame,
};
use crate::frame::FrameHeader;
use crate::mask::Mask;
use crate::opcode::Opcode;
use crate::{
	mask,
	Error,
	Result,
};
use bytes::{
	Buf,
	BufMut,
	Bytes,
	BytesMut,
};
use std::convert::TryFrom;
use std::{
	str,
	usize,
};
use tokio_util::codec::{
	Decoder,
	Encoder,
};

/// A text string, a block of binary data or a WebSocket control frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
	opcode: Opcode,
	data: Bytes,
}

impl Message {
	/// Creates a message from a [`Bytes`] object.
	///
	/// The message can be tagged as text or binary.
	///
	/// # Errors
	///
	/// This function validates the bytes in `data` according to the `opcode` parameter:
	/// - For [`Opcode::Text`] it returns `Err` if the bytes in `data` do not contain valid UTF-8 text.
	/// - For [`Opcode::Close`] it returns `Err` if `data` does not contain a two-byte close code
	///   followed by valid UTF-8 text, unless `data` is empty.
	pub fn new<B: Into<Bytes>>(
		opcode: Opcode,
		data: B,
	) -> Result<Self> {
		let data = data.into();

		match opcode {
			Opcode::Close => match data.len() {
				0 => {}
				1 => return Err("close frames must be at least 2 bytes long".into()),
				_ => {
					str::from_utf8(&data[2..])?;
				}
			},
			Opcode::Text => {
				str::from_utf8(&data)?;
			}
			_ => {}
		}

		Ok(Message { opcode, data })
	}

	/// Creates a text message from a `String`.
	pub fn text<S: Into<String>>(data: S) -> Self {
		Message {
			opcode: Opcode::Text,
			data: data.into().into(),
		}
	}

	/// Creates a binary message from any type that can be converted to [`Bytes`], such as `&[u8]` or `Vec<u8>`.
	pub fn binary<B: Into<Bytes>>(data: B) -> Self {
		Message {
			opcode: Opcode::Binary,
			data: data.into(),
		}
	}

	pub(crate) fn header(
		&self,
		mask: Option<Mask>,
	) -> FrameHeader {
		FrameHeader {
			fin: true,
			rsv: 0,
			opcode: self.opcode.into(),
			mask,
			data_len: self.data.len().into(),
		}
	}

	/// Creates a message that indicates the connection is about to be closed.
	/// The close frame does not contain a reason.
	#[must_use]
	pub fn close() -> Self {
		Message {
			opcode: Opcode::Close,
			data: Bytes::new(),
		}
	}

	/// Creates a message that indicates the connection is about to be closed.
	/// The close frame contains a code and a text reason.
	#[must_use]
	pub fn close_with_reason(
		code: CloseCode,
		mut reason: String,
	) -> Self {
		// Shorten the string so that 2 + reason.len() fits under the limit for control frames
		let reason_len = truncate_floor_char_boundary(&mut reason, 123);

		let mut data = reason.into_bytes();
		data.extend_from_slice(&[0, 0]);
		data.copy_within(0..reason_len, 2);
		data[0..2].copy_from_slice(&u16::from(code).to_be_bytes());

		Message {
			opcode: Opcode::Close,
			data: data.into(),
		}
	}

	/// Creates a message requesting a pong response.
	///
	/// The client can send one of these to request a pong response from the server.
	pub fn ping<B: Into<Bytes>>(data: B) -> Self {
		Message {
			opcode: Opcode::Ping,
			data: data.into(),
		}
	}

	/// Creates a response to a ping message.
	///
	/// The client can send one of these in response to a ping from the server.
	pub fn pong<B: Into<Bytes>>(data: B) -> Self {
		Message {
			opcode: Opcode::Pong,
			data: data.into(),
		}
	}

	/// Returns this message's WebSocket opcode.
	pub fn opcode(&self) -> Opcode {
		self.opcode
	}

	/// Returns a reference to the data held in this message.
	pub fn data(&self) -> &Bytes {
		&self.data
	}

	/// Consumes the message, returning its data.
	pub fn into_data(self) -> Bytes {
		self.data
	}

	/// For messages with opcode [`Opcode::Text`], returns a reference to the text.
	/// Returns `None` otherwise.
	pub fn as_text(&self) -> Option<&str> {
		if self.opcode.is_text() {
			Some(unsafe { str::from_utf8_unchecked(&self.data) })
		} else {
			None
		}
	}

	/// For messages with opcode [`Opcode::Close`], returns the [`CloseFrame`].
	/// Returns `None` otherwise.
	pub fn as_close(&self) -> Option<CloseFrame> {
		if matches!(self.opcode, Opcode::Close) && self.data.len() >= 2 {
			let mut data = self.data.clone();
			let code = data.get_u16();
			Some(CloseFrame {
				code: code.into(),
				reason: data,
			})
		} else {
			None
		}
	}
}

/// Tokio codec for WebSocket messages. This codec can send and receive [`Message`] structs.
#[derive(Clone)]
pub struct MessageCodec {
	interrupted_message: Option<(Opcode, BytesMut)>,
	use_mask: bool,
}

impl MessageCodec {
	/// Creates a `MessageCodec` for a client.
	///
	/// Encoded messages are masked.
	#[must_use]
	pub fn client() -> Self {
		Self::with_masked_encode(true)
	}

	// /// Creates a `MessageCodec` for a server.
	// ///
	// /// Encoded messages are not masked.
	// #[must_use]
	// pub fn server() -> Self {
	//     Self::with_masked_encode(false)
	// }

	/// Creates a `MessageCodec` while specifying whether to use message masking while encoding.
	#[must_use]
	pub fn with_masked_encode(use_mask: bool) -> Self {
		Self {
			use_mask,
			interrupted_message: None,
		}
	}
}

fn truncate_floor_char_boundary(
	s: &mut String,
	new_len: usize,
) -> usize {
	// TODO call str::floor_char_boundary when stable
	let mut len = s.len();
	if len > new_len {
		len = new_len;

		while !s.is_char_boundary(len) {
			len -= 1;
		}

		s.truncate(len);
	}

	len
}

impl Decoder for MessageCodec {
	type Item = Message;
	type Error = Error;

	fn decode(
		&mut self,
		src: &mut BytesMut,
	) -> Result<Option<Message>> {
		let mut state = self.interrupted_message.take();
		let (opcode, data) = loop {
			let (header, header_len) = if let Some(tuple) = FrameHeader::parse_slice(src) {
				tuple
			} else {
				// The buffer isn't big enough for the frame header.
				// Reserve additional space for a frame header, plus reasonable extensions.
				src.reserve(512);
				self.interrupted_message = state;
				return Ok(None);
			};

			let data_len = usize::try_from(header.data_len)?;
			let frame_len = header_len + data_len;
			if frame_len > src.remaining() {
				// The buffer contains the frame header but it's not big enough for the data.
				// Reserve additional space for the frame data, plus the next frame header.
				// Note that we guard against bad data that indicates an unreasonable frame length.

				// If we reserved buffer space for the entire frame data in a single call, would the buffer exceed usize::MAX bytes in size?
				// On a 64-bit platform we should not reach here as the usize::try_from line above enforces the max payload length detailed in the RFC of 2^63 bytes.
				if frame_len > usize::MAX - src.remaining() {
					return Err(format!(
						"frame is too long: {0} bytes ({0:x})",
						frame_len
					)
					.into());
				}

				// We don't really reserve space for the entire frame data in a single call.
				// If somebody is sending more than a gigabyte of data in a single frame then we'll still try to receive it, we'll just reserve in 1GB chunks.
				src.reserve(frame_len.min(0x4000_0000) + 512);

				self.interrupted_message = state;
				return Ok(None);
			}

			// The buffer contains the frame header and all of the data. We can parse it and return Ok(Some(...)).
			let mut data = src.split_to(frame_len);
			data.advance(header_len);

			let FrameHeader {
				fin,
				rsv,
				opcode,
				mask,
				data_len: _data_len,
			} = header;

			if rsv != 0 {
				return Err(format!(
					"reserved bits are not supported: 0x{:x}",
					rsv
				)
				.into());
			}

			if let Some(mask) = mask {
				// Note: clients never need decode masked messages because masking is only used for client -> server frames.
				// However this code is used to test round tripping of masked messages.
				mask::mask_slice(&mut data, mask);
			};

			let opcode = if opcode == 0 {
				None
			} else {
				let opcode = Opcode::try_from(opcode).ok_or_else(|| {
					format!(
						"opcode {} is not supported",
						opcode
					)
				})?;
				if opcode.is_control() && data_len >= 126 {
					return Err(format!(
						"control frames must be shorter than 126 bytes ({} bytes is too long)",
						data_len
					)
					.into());
				}

				Some(opcode)
			};

			state = if let Some((partial_opcode, mut partial_data)) = state {
				if let Some(opcode) = opcode {
					if fin && opcode.is_control() {
						self.interrupted_message = Some((partial_opcode, partial_data));
						break (opcode, data);
					}

					return Err(format!(
						"continuation frame must have continuation opcode, not {:?}",
						opcode
					)
					.into());
				}

				partial_data.extend_from_slice(&data);

				if fin {
					break (partial_opcode, partial_data);
				}

				Some((partial_opcode, partial_data))
			} else if let Some(opcode) = opcode {
				if fin {
					break (opcode, data);
				}
				if opcode.is_control() {
					return Err("control frames must not be fragmented".into());
				}
				Some((opcode, data))
			} else {
				return Err("continuation must not be first frame".into());
			}
		};

		Ok(Some(Message::new(
			opcode,
			data.freeze(),
		)?))
	}
}

impl Encoder<Message> for MessageCodec {
	type Error = Error;

	fn encode(
		&mut self,
		item: Message,
		dst: &mut BytesMut,
	) -> Result<()> {
		self.encode(&item, dst)
	}
}

impl<'a> Encoder<&'a Message> for MessageCodec {
	type Error = Error;

	fn encode(
		&mut self,
		item: &Message,
		dst: &mut BytesMut,
	) -> Result<()> {
		let mask = if self.use_mask { Some(Mask::new()) } else { None };
		let header = item.header(mask);
		header.write_to_bytes(dst);

		if let Some(mask) = mask {
			let offset = dst.len();
			dst.reserve(item.data.len());

			unsafe {
				dst.set_len(offset + item.data.len());
			}

			mask::mask_slice_copy(
				&mut dst[offset..],
				&item.data,
				mask,
			);
		} else {
			dst.put_slice(&item.data);
		}

		Ok(())
	}
}
