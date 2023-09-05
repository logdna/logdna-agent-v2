//! This crate provides "rate_limit_macro" procedural macro.
//!
//! Usage:
//! The following dependencies are needed:
//!
//! [dependency]
//! rate_limit_macro = "0.1.0"
//! lazy_static = "1.4"
//! once_cell = "1.8"
//!
//! Here is code example:
//!
//! use rate_limit_macro::rate_limit;
//!
//! let mut called_times = 0;
//!
//! for _ in 0..10 {
//!   rate_limit!(rate = 5, interval = 1, {
//!     called_times += 1;
//!   });
//! }
//!
//! // Only 5 calls should have been allowed due to rate limiting.
//! assert_eq!(called_times, 5);
//!
//! Notes:
//! - this macro shoudl be used with synchronous blocks of code, block example: {  tracing::error!("this log message can create log flood!");  }
//!
extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse::Parse, parse::ParseStream, Block, Expr, Token};

// Struct to represent the parsed input of the macro.
struct RateLimitInput {
    rate: Expr,
    interval: Expr,
    block: Block,
}

impl Parse for RateLimitInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let rate_ident: syn::Ident = input.parse()?;
        assert_eq!(rate_ident.to_string(), "rate");
        let _: Token![=] = input.parse()?;
        let rate: Expr = input.parse()?;
        let _: Token![,] = input.parse()?;

        let interval_ident: syn::Ident = input.parse()?;
        assert_eq!(interval_ident.to_string(), "interval");
        let _: Token![=] = input.parse()?;
        let interval: Expr = input.parse()?;
        let _: Token![,] = input.parse()?;

        let block: Block = input.parse()?;

        Ok(RateLimitInput {
            rate,
            interval,
            block,
        })
    }
}

/// Rate-limits the execution of a block of code.
///
/// This macro introduces a rate-limiting mechanism that allows a block of code
/// to be executed at a defined maximum rate. If the code is called more frequently
/// than the specified rate, the exceeding calls will trigger an action (e.g., printing a warning).
///
/// # Parameters
///
/// - `rate`: The maximum number of times the block of code can be executed within the specified interval.
/// - `interval`: The time interval (in seconds) for which the `rate` applies.
/// - `block`: The block of code to be rate-limited.
///
#[proc_macro]
pub fn rate_limit(item: TokenStream) -> TokenStream {
    let input: RateLimitInput = syn::parse_macro_input!(item as RateLimitInput);

    let rate = &input.rate;
    let interval = &input.interval;
    let block = &input.block;

    let expanded = quote! {
        {
            use std::sync::{Mutex, Arc};
            use std::time::{Duration, Instant};
            use std::thread::sleep;
            use once_cell::sync::Lazy;

            static STATE: Lazy<Mutex<i32>> = Lazy::new(|| Mutex::new(0));
            static LAST_CALLED: Lazy<Mutex<Instant>> = Lazy::new(|| Mutex::new(Instant::now() - Duration::from_secs(10)));

            let mut state = STATE.lock().unwrap();
            let mut last_called = LAST_CALLED.lock().unwrap();
            let now = Instant::now();
            let elapsed = now.duration_since(*last_called).as_secs();

            // If the time since the last call is greater than the interval, reset the state
            if elapsed > #interval as u64 {
                *state = 0;
                *last_called = now;
            }

            if *state < #rate {
                #block
                *state += 1;
            } else {
                // TODO: Action when rate limit is exceeded
            }
        }
    };

    TokenStream::from(expanded)
}
