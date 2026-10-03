pub fn middle_truncate(value: &str, head: usize, tail: usize) -> String {
    let len = value.chars().count();
    if len <= head + tail + 3 {
        return value.to_string();
    }
    let head: String = value.chars().take(head).collect();
    let tail: String = value.chars().skip(len - tail).collect();
    format!("{head}...{tail}")
}

pub fn flatten_whitespace(text: &str) -> String {
    const MAX_CHARS: usize = 200;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_CHARS {
        flat
    } else {
        let mut clipped: String = flat.chars().take(MAX_CHARS).collect();
        clipped.push('…');
        clipped
    }
}
