/// Split text into overlapping chunks by character count.
///
/// Embedding each chunk separately (rather than one vector for a whole document)
/// is what makes retrieval over long documents accurate.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if text.trim().is_empty() {
        return vec![];
    }

    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= chunk_size {
        return vec![text.trim().to_string()];
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

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_chunk() {
        let text = "a ".repeat(500);
        let chunks = chunk_text(&text, 100, 20);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.chars().count() <= 100);
        }
    }

    #[test]
    fn empty_input() {
        assert!(chunk_text("", 100, 10).is_empty());
        assert!(chunk_text("   ", 100, 10).is_empty());
    }

    #[test]
    fn short_text_single_chunk() {
        let chunks = chunk_text("hello world", 512, 64);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello world");
    }
}
