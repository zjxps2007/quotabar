pub(crate) fn allows(bytes: &[u8], maximum: usize) -> bool {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for &b in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else {
            match b {
                b'"' => quoted = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > maximum {
                        return false;
                    }
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    true
}
