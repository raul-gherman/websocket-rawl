# websocket-rawl

[dependencies]
websocket-rawl = {git = "https://github.com/raul-gherman/websocket-rawl.git"}

```rust
use websocket_rawl::{Message, Opcode, Result};

let builder = websocket_rawl::ClientBuilder::new("wss://demo.ctraderapi.com:5035")?;
let mut ws_stream = builder.async_connect().await?;
info!("connected...");
let (w, r) = ws_stream.split();
```