# rate-limit-core

## Description

This is a companion crate to [`rate-limit-macro`](https://github.com/logdna/logdna-agent-v2/tree/master/common/misc/rate-limit/macro). The primary purpose of `rate-limit-core` is to provide tests for the `rate-limit-macro` crate.

## Features

- Includes tests for `rate-limit-macro`.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
rate-limit-core = "1"
```

## License

This project is licensed under the MIT License.

## Authors

- Dmitri Khokhlov <dkhokhlov@gmail.com>

## Repository

The source code repository is located at [https://github.com/logdna/logdna-agent-v2/tree/master/common/misc/rate-limit](https://github.com/logdna/logdna-agent-v2/tree/master/common/misc/rate-limit).

## Tests

This crate contains tests that verify the functionality of `rate-limit-macro`. To run the tests, use the following command:

```bash
cargo test
```

## See Also

- [`rate-limit-macro`](https://github.com/logdna/logdna-agent-v2/tree/master/common/misc/rate-limit/macro) - The macro crate that this crate tests.
