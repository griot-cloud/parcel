//! parcel-engine: a reference executor for parcel contracts on DataFusion.
//!
//! It writes data under a contract (flags, layout, manifest), validates it, and
//! answers SQL in which every table is a contract. It follows peQL's design
//! closely enough to prove parcel's artifacts end to end; peQL is the
//! production engine.

pub mod binding;
pub mod budget;
pub mod bundle;
pub mod differential;
pub mod engine;
pub mod error;
pub mod manifest;
pub mod shape;

pub use engine::{Engine, Envelope, QueryResult, Resolution, Verdict, WriteMode, WriteReport};
pub use error::{EngineError, Result};
pub use manifest::Manifest;
pub use parcel_runtime::Caller;
