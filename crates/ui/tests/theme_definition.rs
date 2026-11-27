use ui::theme::ThemeDefinition;

#[test]
fn parse_hex_rejects_non_ascii_digits() {
    assert_eq!(ThemeDefinition::parse_hex("#éa"), None);
    assert_eq!(ThemeDefinition::parse_hex("#éaabbcc"), None);
}

#[test]
fn parse_hex_resolves_three_and_six_digits() {
    assert!(ThemeDefinition::parse_hex("#abc").is_some());
    assert!(ThemeDefinition::parse_hex("#aabbcc").is_some());
    assert_eq!(ThemeDefinition::parse_hex("abc"), None);
    assert_eq!(ThemeDefinition::parse_hex("#ab"), None);
}
