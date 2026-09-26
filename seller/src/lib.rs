//! x402 seller for the Countersign demo: a `batch-settlement` endpoint that screens its payers
//! and cashes vouchers in through Coinbase's deployed escrow.
pub mod chain;
pub mod channels;
pub mod env;
pub mod screen;
pub mod server;
