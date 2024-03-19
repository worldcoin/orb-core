//! Orb core procedural macros.

#![warn(clippy::pedantic)]

extern crate proc_macro;

mod broker;

use proc_macro::TokenStream;

#[proc_macro_derive(Broker, attributes(agent))]
pub fn derive_broker(input: TokenStream) -> TokenStream {
    broker::proc_macro_derive(input)
}
