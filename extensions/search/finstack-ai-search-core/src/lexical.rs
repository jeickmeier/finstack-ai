use crate::SearchError;

/// Maximum normalized terms in one keyword/BM25 query or expansion.
pub const MAX_LEXICAL_TERMS: usize = 64;

/// Tokenize untrusted lexical text into a finite set of alphanumeric terms.
/// Syntax characters are separators and cannot become FTS operators.
///
/// # Errors
/// Rejects empty queries and more than 64 terms before database execution.
pub fn lexical_terms(text: &str) -> Result<Vec<&str>, SearchError> {
    let terms: Vec<_> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .take(MAX_LEXICAL_TERMS + 1)
        .collect();
    if terms.is_empty() || terms.len() > MAX_LEXICAL_TERMS {
        return Err(SearchError::invalid("lexical_terms"));
    }
    Ok(terms)
}

/// Quote a bounded OR expression for `SQLite` FTS5. All source adapters use the
/// same token interpretation; user text never supplies SQL or FTS syntax.
///
/// # Errors
/// Returns the same finite-term validation errors as [`lexical_terms`].
pub fn fts_expression(text: &str) -> Result<String, SearchError> {
    Ok(lexical_terms(text)?
        .into_iter()
        .map(|term| format!("\"{term}\""))
        .collect::<Vec<_>>()
        .join(" OR "))
}
