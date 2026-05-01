fn is_handle_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')
}

#[cfg(test)]
pub(super) fn extract_handles(body: &str) -> Vec<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut handles: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let starts_mention = chars[i] == '@' && (i == 0 || !is_handle_char(chars[i - 1]));
        if starts_mention {
            let mut h = String::new();
            let mut j = i + 1;
            while j < chars.len() && is_handle_char(chars[j]) {
                h.push(chars[j]);
                j += 1;
            }
            if !h.is_empty() && !handles.contains(&h) {
                handles.push(h);
            }
            i = j;
            continue;
        }
        i += 1;
    }
    handles
}

pub(super) fn extract_dispatch_handles(body: &str) -> Vec<String> {
    let first_line = body.split_once('\n').map_or(body, |(line, _)| line);
    let chars: Vec<char> = first_line.chars().collect();
    let mut handles = Vec::new();
    let mut i = 0;

    loop {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() || chars[i] != '@' {
            break;
        }

        let mut handle = String::new();
        let mut j = i + 1;
        while j < chars.len() && is_handle_char(chars[j]) {
            handle.push(chars[j]);
            j += 1;
        }
        if handle.is_empty() {
            break;
        }
        if !handles.contains(&handle) {
            handles.push(handle);
        }
        i = j;
    }

    handles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_handles_parses_valid_at_mentions() {
        assert_eq!(extract_handles("hi @alice and @bob"), vec!["alice", "bob"]);
        assert_eq!(extract_handles("no mentions here"), Vec::<String>::new());
        assert_eq!(extract_handles("@x @x @x"), vec!["x"]);
        assert_eq!(extract_handles("foo@example.com"), Vec::<String>::new());
    }

    #[test]
    fn extract_dispatch_handles_uses_leading_first_line_mentions_only() {
        assert_eq!(
            extract_dispatch_handles("@alice @bob please\n@ghost later"),
            vec!["alice", "bob"]
        );
        assert_eq!(
            extract_dispatch_handles("please @alice\n@bob later"),
            Vec::<String>::new()
        );
        assert_eq!(
            extract_dispatch_handles("  @lead\nbody @alice"),
            vec!["lead"]
        );
    }
}
