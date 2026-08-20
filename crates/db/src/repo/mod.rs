//! Repository queries, split by data domain. The file a query lives in is the
//! domain it belongs to, so reviewing what a domain grant exposes means reading
//! one file rather than grepping. Task 8 turns these free functions into
//! methods on `Scoped`; the split lands first so that change is reviewable.

pub mod breaches;
pub mod imports;
pub mod market_data;
pub mod nav;
pub mod positions;
pub mod reference;
pub mod shareholders;
pub mod transactions;

pub use breaches::*;
pub use imports::*;
pub use market_data::*;
pub use nav::*;
pub use positions::*;
pub use reference::*;
pub use shareholders::*;
pub use transactions::*;
