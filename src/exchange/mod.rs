//! Exchange adapters and the registry that builds them. Callers select an
//! exchange by the name operators use, through [`lookup`] (the names come
//! from [`name`], for every exchange in [`registered`]); get its connectors
//! from [`connect`]; and use them through the [`crate::ports`] traits. Only
//! this module knows which adapter serves which [`crate::ExchangeId`].
//! Binance Spot is the only exchange today; each lives under its own
//! namespace ([`binance`]) so a second one can be added without
//! restructuring the tree.

pub mod binance;
mod credentials;
mod registry;

pub use credentials::ApiCredentials;
pub use registry::{
    ConnectorConfig, ConnectorFactory, DEFAULT_EXCHANGE, ExchangeConnectors, connect,
    default_base_url, factory, lookup, name, registered,
};
