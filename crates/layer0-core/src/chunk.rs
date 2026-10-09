pub fn validate_content(text: &str, chunk_size: usize, overlap: usize) -> anyhow::Result<()> {
    anyhow::ensure!(text.len() <= 1024 * 1024, "Document exceeds one MiB limit");
    anyhow::ensure!(
        (1..=8192).contains(&chunk_size) && overlap < chunk_size,
        "Invalid chunk size or overlap"
    );
    let characters = text.chars().count();
    let chunks = if characters <= chunk_size {
        1
    } else {
        1 + (characters - chunk_size).div_ceil(chunk_size - overlap)
    };
    anyhow::ensure!(chunks <= 256, "Document exceeds embedding chunk budget");
    Ok(())
}

/// Split text into overlapping chunks by character count.
///
/// Embedding each chunk separately (rather than one vector for a whole document)
/// is what makes retrieval over long documents accurate.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> anyhow::Result<Vec<String>> {
    validate_content(text, chunk_size, overlap)?;
    if text.trim().is_empty() {
        return Ok(vec![]);
    }

    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= chunk_size {
        return Ok(vec![text.trim().to_string()]);
    }

    let mut chunks = Vec::new();
    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut start = 0;

    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        let trimmed = chunk.trim().to_string();
        if !trimmed.is_empty() {
            chunks.push(trimmed);
        }
        if end == chars.len() {
            break;
        }
        start += step;
    }

    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_chunk() {
        let text = "a ".repeat(500);
        let chunks = chunk_text(&text, 100, 20).unwrap();
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.chars().count() <= 100);
        }
    }

    #[test]
    fn empty_input() {
        assert!(chunk_text("", 100, 10).unwrap().is_empty());
        assert!(chunk_text("   ", 100, 10).unwrap().is_empty());
    }

    #[test]
    fn short_text_single_chunk() {
        let chunks = chunk_text("hello world", 512, 64).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello world");
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn invalid_configs_and_fanout_are_rejected() {
        assert!(validate_content("ordinary", 512, 64).is_ok());
        assert!(validate_content("ordinary", 0, 0).is_err());
        assert!(validate_content("ordinary", 10, 10).is_err());
        assert!(validate_content(&"x".repeat(10000), 100, 99).is_err());
        assert!(validate_content(&"x".repeat(1024 * 1024 + 1), 8192, 0).is_err());
    }
}
