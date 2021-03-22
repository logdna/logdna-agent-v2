use proptest::collection::vec;
use proptest::prelude::Strategy;
use proptest::string::string_regex;

pub fn random_line_string_vec(
    min_size: usize,
    max_size: usize,
) -> impl Strategy<Value = Vec<String>> {
    vec(
        (min_size..max_size)
            .prop_flat_map(|i| string_regex(&format!("[a-zA-Z0-9]{{1,64}}{}", i)).unwrap()),
        min_size..max_size,
    )
}
