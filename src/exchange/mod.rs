//! Exchange adapters and the registry that builds them. Callers get an
//! exchange's connectors from [`connect`] and use them through the
//! [`crate::ports`] traits; only this module knows which adapter serves
//! which [`crate::ExchangeId`]. Binance Spot is the only exchange today; each
//! lives under its own namespace ([`binance`]) so a second one can be added
//! without restructuring the tree.

pub mod binance;
mod credentials;
mod registry;

pub use credentials::ApiCredentials;
pub use registry::{ConnectorConfig, ConnectorFactory, ExchangeConnectors, connect, factory};
