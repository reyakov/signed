/// `[head chars]...[tail chars]` middle truncation.
///
/// Values too short for the ellipsis to save space are left alone.
pub fn middle_truncate(value: &str, head: usize, tail: usize) -> String {
    let len = value.chars().count();
    if len <= head + tail + 3 {
        return value.to_string();
    }
    let head: String = value.chars().take(head).collect();
    let tail: String = value.chars().skip(len - tail).collect();
    format!("{head}...{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_truncates_long_values_only() {
        assert_eq!(
            middle_truncate(
                "a008def15796fba9a0d6fab04e8fd57089285d9fd505da5a83fe8aad57a3564d",
                10,
                10,
            ),
            "a008def157...ad57a3564d"
        );
        assert_eq!(
            middle_truncate(
                "30617:a008def15796fba9a0d6fab04e8fd57089285d9fd505da5a83fe8aad57a3564d:ngit",
                10,
                10
            ),
            "30617:a008...3564d:ngit"
        );
        assert_eq!(middle_truncate("short", 10, 10), "short");
    }
}
