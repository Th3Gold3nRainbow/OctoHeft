pub fn expand_pattern(pattern: &str) -> Result<Vec<String>, String> {
    // Find pattern within square brackets [start-end]
    let start_idx = match pattern.find('[') {
        Some(i) => i,
        None => return Ok(vec![pattern.to_string()]),
    };
    let end_idx = match pattern[start_idx..].find(']') {
        Some(i) => start_idx + i,
        None => return Ok(vec![pattern.to_string()]),
    };

    let prefix = &pattern[..start_idx];
    let range_str = &pattern[start_idx + 1..end_idx];
    let suffix = &pattern[end_idx + 1..];

    // Check if range contains dash '-'
    let parts: Vec<&str> = range_str.split('-').collect();
    if parts.len() != 2 {
        return Err("Invalid range format. Use [start-end], e.g. [001-050] or [a-z]".to_string());
    }

    let left = parts[0].trim();
    let right = parts[1].trim();

    // Check numeric range
    if let (Ok(start_num), Ok(end_num)) = (left.parse::<i64>(), right.parse::<i64>()) {
        if start_num > end_num {
            return Err("Range start number cannot be greater than end number".to_string());
        }
        if end_num - start_num > 1000 {
            return Err("Maximum 1000 items allowed per batch expansion".to_string());
        }

        let is_padded = left.starts_with('0') && left.len() > 1;
        let pad_width = if is_padded { left.len() } else { 0 };

        let mut results = Vec::new();
        for n in start_num..=end_num {
            let num_str = if pad_width > 0 {
                format!("{:0width$}", n, width = pad_width)
            } else {
                n.to_string()
            };

            // Recursively expand any subsequent brackets in suffix
            let current = format!("{}{}{}", prefix, num_str, suffix);
            results.push(current);
        }
        return Ok(results);
    }

    // Check alphabetical range [a-z] or [A-Z]
    if left.len() == 1 && right.len() == 1 {
        let start_char = left.chars().next().unwrap();
        let end_char = right.chars().next().unwrap();

        if start_char.is_ascii_alphabetic() && end_char.is_ascii_alphabetic() {
            if start_char > end_char {
                return Err("Range start character cannot come after end character".to_string());
            }

            let mut results = Vec::new();
            for c in (start_char as u8)..=(end_char as u8) {
                let current = format!("{}{}{}", prefix, c as char, suffix);
                results.push(current);
            }
            return Ok(results);
        }
    }

    Err("Invalid range. Supported types: [1-100], [001-050], or [a-z]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_padded_numeric_expansion() {
        let pattern = "https://example.com/img_[001-003].jpg";
        let res = expand_pattern(pattern).unwrap();
        assert_eq!(
            res,
            vec![
                "https://example.com/img_001.jpg",
                "https://example.com/img_002.jpg",
                "https://example.com/img_003.jpg"
            ]
        );
    }

    #[test]
    fn test_alpha_expansion() {
        let pattern = "https://example.com/file_[a-c].zip";
        let res = expand_pattern(pattern).unwrap();
        assert_eq!(
            res,
            vec![
                "https://example.com/file_a.zip",
                "https://example.com/file_b.zip",
                "https://example.com/file_c.zip"
            ]
        );
    }
}

