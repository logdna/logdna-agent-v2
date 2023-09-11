# Rate-Limit-Macro

## Introduction

`rate-limit-macro` is a procedural macro that provides a simple way to rate-limit blocks of code. This crate is part of the [logdna-agent-v2](https://github.com/logdna/logdna-agent-v2) project and is located under `common/misc/rate-limit/macro`.

## Installation

Add the following line to your `Cargo.toml` under `[dependencies]`:

```toml
rate-limit-macro = "1"
```

## Usage

### Basic Usage

Here's a simple example:

```rust
use rate_limit_macro::rate_limit;

let mut called_times = 0;

for _ in 0..10 {
    rate_limit!(rate = 5, interval = 1, {
        called_times += 1;
    });
}

// Only 5 calls should have been allowed due to rate limiting.
assert_eq!(called_times, 5);
```

### With Fallback Block

You can also provide an optional fallback block that will be executed when the rate limit is exceeded:

```rust
let mut called_times = 0;
let mut fallback_called_times = 0;

for _ in 0..10 {
    rate_limit!(rate = 5, interval = 1, {
        called_times += 1;
    }, {
        fallback_called_times += 1;
    });
}

// Check that the number of rate-limited calls and fallback calls add up to the total calls.
assert_eq!(called_times + fallback_called_times, 10);
```

## API Documentation

- `rate`: The maximum number of times the block of code can be executed within the specified interval.
- `interval`: The time interval (in seconds) for which the `rate` applies.
- `block`: The block of code to be rate-limited.
- `fallback_block` (Optional): A block of code to be executed when the rate limit is exceeded.

## Notes

- This macro is lockless, so rate limiting is not absolutely exact.

## Related Crates

- `rate-limit-core`: This is a companion library that contains tests and depends on `rate-limit-macro`.

## Source Code

The source code for this crate is part of the [logdna-agent-v2](https://github.com/logdna/logdna-agent-v2/tree/master/common/misc/rate-limit/macro) GitHub repository.

## License

This crate is licensed under the MIT License.

## Authors

- Dmitri Khokhlov <dkhokhlov@gmail.com>

## Contributing

If you'd like to contribute, please fork the repository and use a feature branch. Pull requests are warmly welcome.
