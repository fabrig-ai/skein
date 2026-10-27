//! Bounded OAuth documents and durable token metadata (oauth.md §§2–5).
//! All clocks and bounds are injected; this crate performs no I/O and owns no
//! provider policy or domain credentials. Its server codecs serve the fake peer.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;
#[rustfmt::skip]
#[path = "generated/v3.rs"]
pub mod v3;
mod common;
mod documents;
mod json;
mod jwt;
mod keys;
mod machine;
#[path = "../patches/mod.rs"]
pub mod patches;
mod pkce;
mod record;
mod redirect;
pub use common::{Check, DecodeError, Failure, Limits, classify, worst_case};
pub use documents::{
    OAuthError, RefreshRequest, TokenResponse, decode_error, decode_request, decode_response, encode_error,
    encode_request, encode_response,
};
pub use json::{Collector, Json};
pub use jwt::{Claims, read_claims};
pub use machine::{
    Client, ClientLimits, Event, HttpEvidence, HttpRequest, HttpResponse, Inadmissible, MAX_OUT, Method, Redirect,
    Registration, Request, WireFormat, client_worst_case,
};
pub use pkce::challenge;
pub use record::{ClaimSelector, RECORD_VERSION, RefreshState, SavedToken, decode_record, encode_record, rotate};
pub use redirect::{OpenId, Parameter, Sent, Terms, missing_scopes, redirect_address, validate_registration};

#[cfg(test)]
mod machine_tests;
#[cfg(test)]
mod tests;

/// The owner of skein's patches, as their headers spell it.
pub const OWNER: &str = "skein";
/// The record's family, as its kept texts spell it.
pub const FAMILY: &str = "skein-oauth";
/// The declaration a kept record is.
pub const ROOT: &str = "SavedToken";
/// The family's layout fingerprint (codec.md, 9), checked by the drift test.
pub const FINGERPRINT: [u8; 32] = [
    41, 62, 146, 237, 7, 170, 232, 22, 146, 49, 19, 74, 222, 68, 228, 174, 213, 32, 183, 205, 88, 142, 62, 177, 6, 115,
    197, 141, 117, 122, 254, 62,
];
