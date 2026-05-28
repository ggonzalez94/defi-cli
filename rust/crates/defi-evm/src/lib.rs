//! alloy wrappers: address parse/validate, ABI encode, RPC client, signing.
//!
//! Wraps the `alloy` stack to provide the EVM primitives the Go tree used from
//! go-ethereum (abi, rlp, crypto, types, ethclient).
#![allow(dead_code, unused)]

pub mod abi;
pub mod address;
pub mod rpc;
pub mod signer;
