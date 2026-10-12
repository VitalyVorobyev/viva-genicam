//! With `RUST_LOG` set, the producer logs to standard error itself: a GenTL
//! consumer is rarely a Rust program, and installs no `tracing` subscriber
//! for the facade's logs to reach.
//!
//! A test binary of its own, with one test: the subscriber is global to the
//! process, and the environment is set before anything reads it.

use viva_gentl::abi::GC_ERR_SUCCESS;
use viva_gentl::{GCCloseLib, GCInitLib};

#[test]
fn rust_log_makes_the_producer_log() {
    assert!(!tracing::dispatcher::has_been_set(), "nothing logs yet");
    // SAFETY: the only test in this binary sets the variable before it
    // starts a thread that could read the environment.
    unsafe { std::env::set_var("RUST_LOG", "viva_gentl=debug") };
    assert_eq!(GCInitLib(), GC_ERR_SUCCESS);
    assert!(
        tracing::dispatcher::has_been_set(),
        "GCInitLib installed a subscriber"
    );
    assert_eq!(GCCloseLib(), GC_ERR_SUCCESS);
}
