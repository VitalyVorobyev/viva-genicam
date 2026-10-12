//! A GenTL producer over `viva-genicam`: GigE Vision cameras for any GenTL
//! consumer, such as Harvesters.
//!
//! Built as a `cdylib` and renamed `viva_gentl.cti` (`scripts/build-cti.sh`),
//! this crate exports the 64 functions of the EMVA GenICam GenTL Standard
//! v1.6 C interface. The design, and which functions do what, is
//! [ADR-0031](https://github.com/VitalyVorobyev/viva-genicam/blob/main/docs/adrs/adr0031-gentl-producer.md).
//!
//! # Safety of the C interface
//!
//! Every exported function is `unsafe`: its pointer arguments must be null or
//! valid for the access the standard describes for them (§6.1.3 and the
//! function's own section), and a buffer announced with `DSAnnounceBuffer`
//! must stay valid until it is revoked (§6.3.5.2). The producer checks every
//! pointer for null before it uses it, never dereferences a handle — a handle
//! is a key into a registry of open modules, and an unknown one is
//! `GC_ERR_INVALID_HANDLE` — and lets no panic unwind into the caller.
//!
//! With `RUST_LOG` set when `GCInitLib` runs, the producer logs to standard
//! error.
//!
//! The Rust crate (`rlib`) exists so the integration tests can call the
//! exported functions directly; [`abi`] is the vocabulary they share with C.

#[cfg(target_arch = "x86")]
compile_error!(
    "GenTL uses stdcall on 32-bit x86 (GC_CALLTYPE); viva-gentl is built for 64-bit targets only"
);

pub mod abi;
mod device;
mod error;
mod exports;
mod interface;
mod libpath;
mod library;
mod nic;
mod out;
mod port;
mod regmap;
mod stream;
mod system;

pub use exports::*;

/// A custom `TL_INFO_CMD` (custom IDs start at 1000, §6.4.1.3) that debug
/// builds answer by panicking, so the tests can exercise the guard every
/// export runs in. A release build answers it `GC_ERR_NOT_IMPLEMENTED`, as
/// any command it does not know.
pub const PANIC_PROBE: abi::TlInfoCmd = 1000 + 0x0BAD;
