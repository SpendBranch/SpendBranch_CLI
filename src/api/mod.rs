//! What the CLI says to the server and what it accepts back: the server address rules, the
//! pairing messages and the failure type. Pure: the network code lives in `infra::http`.
//!
//! Everything the server sends is untrusted text. It is validated here and never echoed.
mod address;
mod failure;
mod pairing;
mod status;
mod upload;

pub use address::*;
pub use failure::*;
pub use pairing::*;
pub use status::*;
pub use upload::*;
