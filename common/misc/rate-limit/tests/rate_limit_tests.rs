use rate_limit_macro::rate_limit;

#[test]
fn test_rate_limit_is_limited() {
    let mut called_times = 0;

    for _ in 0..10 {
        rate_limit!(rate = 5, interval = 1, {
            called_times += 1;
        });
    }

    // Only 5 calls should have been allowed due to rate limiting.
    assert_eq!(called_times, 5);
}

#[test]
fn test_rate_limit_with_fallback() {
    let mut called_times = 0;
    let mut fallback_called_times = 0;
    let total_calls = 20; // Total number of times the macro will be called

    for _ in 0..total_calls {
        rate_limit!(
            rate = 5,
            interval = 1,
            {
                called_times += 1;
            },
            {
                fallback_called_times += 1;
            }
        );
    }

    // Check that the number of rate-limited calls and fallback calls add up to the total calls
    assert_eq!(called_times + fallback_called_times, total_calls);
}
