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
//! - this macro is lockless, the rate limiting may not be 100% accurate.
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
    fallback_block: Option<Block>,
}

impl Parse for RateLimitInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let _: syn::Ident = input.parse()?;
        let _: Token![=] = input.parse()?;
        let rate: Expr = input.parse()?;
        let _: Token![,] = input.parse()?;

        let _: syn::Ident = input.parse()?;
        let _: Token![=] = input.parse()?;
        let interval: Expr = input.parse()?;
        let _: Token![,] = input.parse()?;

        let block: Block = input.parse()?;

        let fallback_block: Option<Block> = if input.is_empty() {
            None
        } else {
            let _: Token![,] = input.parse()?;
            Some(input.parse()?)
        };

        Ok(RateLimitInput {
            rate,
            interval,
            block,
            fallback_block,
        })
    }
}

/// Rate-limits the execution of a block of code.
///
/// # Parameters
///
/// - `rate`: The maximum number of times the block of code can be executed within the specified interval.
/// - `interval`: The time interval (in seconds) for which the `rate` applies.
/// - `block`: The block of code to be rate-limited.
/// - `fallback_block` (Optional): A block of code to be executed when the rate limit is exceeded.
///
#[proc_macro]
pub fn rate_limit(item: TokenStream) -> TokenStream {
    let input: RateLimitInput = syn::parse_macro_input!(item as RateLimitInput);

    let rate = &input.rate;
    let interval = &input.interval;
    let block = &input.block;
    let fallback_block = &input.fallback_block.unwrap_or_else(|| {
        syn::parse2(quote!({})).unwrap_or_else(|_| panic!("Failed to parse an empty block"))
    });

    let expanded = quote! {
        {
            use std::sync::atomic::{AtomicU64, Ordering};
            use std::time::{SystemTime, Duration};

            static STATE: AtomicU64 = AtomicU64::new(0);
            static LAST_CALLED: AtomicU64 = AtomicU64::new(0);

            let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_else(|_| Duration::new(0, 0)).as_secs();
            let elapsed = now - LAST_CALLED.load(Ordering::Relaxed);

            if elapsed > #interval as u64 {
                STATE.store(0, Ordering::Relaxed);
                LAST_CALLED.store(now, Ordering::Relaxed);
            }

            if STATE.fetch_add(1, Ordering::Relaxed) < #rate as u64 {
                #block
                LAST_CALLED.store(now, Ordering::Relaxed);
            } else {
                #fallback_block
            }
        }
    };

    TokenStream::from(expanded)
}
