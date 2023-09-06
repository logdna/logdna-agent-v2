//! This crate provides "rate_limit_macro" procedural macro.
//!
//! Usage:
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
//! - this macro should be used with synchronous blocks of code, block example: {  tracing::error!("this log message can create log flood!");  }
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
            use std::sync::atomic::{AtomicU64, Ordering};
            use std::time::{SystemTime, Duration};

            static STATE: AtomicU64 = AtomicU64::new(0);
            static LAST_CALLED: AtomicU64 = AtomicU64::new(0);

            let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_else(|_| Duration::new(0, 0)).as_secs();
            let elapsed = now - LAST_CALLED.load(Ordering::Relaxed);

            // If the time since the last call is greater than the interval, reset the state
            if elapsed > #interval as u64 {
                STATE.store(0, Ordering::Relaxed);
                LAST_CALLED.store(now, Ordering::Relaxed);
            }

            if STATE.fetch_add(1, Ordering::Relaxed) < #rate as u64 {
                #block
                LAST_CALLED.store(now, Ordering::Relaxed);
            } else {
                // TODO: Action when rate limit is exceeded
            }
        }
    };

    TokenStream::from(expanded)
}
