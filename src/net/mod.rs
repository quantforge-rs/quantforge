//! QuantForge instances talking to each other as clients: [`client`]
//! speaks a server's routes, in the wire format of
//! [`protocol`](crate::protocol). Like the server, it uses the protocol and
//! nothing else of the crate, so what it sends and reads is exactly what
//! the protocol specifies.

pub mod client;
