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

