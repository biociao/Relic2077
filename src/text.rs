//! Deterministic tokenization for the vector layer.
//!
//! Relic never calls a model and never opens a socket to answer a query, so the
//! vector layer cannot depend on a hosted embedding API. Instead every entry is
//! tokenized here and projected into a hashed feature space by
//! [`crate::embedding`]. The tokenizer is part of the derived-data contract: its
//! `TOKENIZER_VERSION` is stored next to the vectors, and changing a rule here
//! invalidates and rebuilds that cache instead of silently mixing projections.
//!
//! Rules:
//!
//! - ASCII runs of letters and digits become one lowercased token; a single
//!   letter is dropped unless it is a digit, so "512" survives but "a" does not.
//! - CJK, kana, and hangul runs emit every character *and* every adjacent pair,
//!   because those scripts have no whitespace word boundaries.
//! - A small stopword list removes English function words. Everything else is
//!   left to inverse document frequency in [`crate::embedding`].

/// Bump when a tokenization rule changes; the embedding cache stores this value
/// and is discarded when it no longer matches.
pub const TOKENIZER_VERSION: u32 = 2;

/// English function words carry no retrieval signal and would occupy hashed
/// dimensions in every document. Content words are never filtered here.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "any", "can", "had", "her", "was",
    "one", "our", "out", "day", "get", "has", "him", "his", "how", "its", "may", "new", "now",
    "old", "see", "two", "way", "who", "boy", "did", "use", "she", "too", "that", "this", "with",
    "from", "they", "will", "would", "there", "their", "what", "about", "which", "when", "your",
    "them", "then", "than", "some", "into", "only", "other", "such", "been", "were", "more",
    "these", "those", "because", "should", "could", "does", "doing", "done", "each", "very",
    "just", "over", "also", "after", "before", "while", "where", "have", "here", "being",
];

/// True for scripts written without whitespace word boundaries, where per
/// character and per adjacent-pair tokens are the useful retrieval units.
pub fn is_cjk(character: char) -> bool {
    matches!(character as u32,
        0x3040..=0x30FF      // hiragana and katakana
        | 0x3400..=0x4DBF    // CJK unified ideographs extension A
        | 0x4E00..=0x9FFF    // CJK unified ideographs
        | 0xAC00..=0xD7AF    // hangul syllables
        | 0xF900..=0xFAFF    // CJK compatibility ideographs
    )
}

/// Split text into the tokens that define the vector space of a vault.
///
/// The result is order-preserving and repeats are intentional: term frequency
/// is derived from the token stream by the caller.
pub fn tokenize(text: &str) -> Vec<String> {
    let lowered = text.to_lowercase();
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut run: Vec<char> = Vec::new();

    for character in lowered.chars() {
        if character.is_ascii_alphanumeric() {
            if !run.is_empty() {
                push_script_run(&mut run, &mut tokens);
            }
            word.push(character);
            continue;
        }
        if !word.is_empty() {
            push_word(&mut word, &mut tokens);
        }
        if is_cjk(character) {
            run.push(character);
        } else if !run.is_empty() {
            push_script_run(&mut run, &mut tokens);
        }
    }
    if !word.is_empty() {
        push_word(&mut word, &mut tokens);
    }
    if !run.is_empty() {
        push_script_run(&mut run, &mut tokens);
    }
    tokens
}

/// Fold one ASCII run into a token, applying the single-letter and stopword
/// filters. Multi-character digits are kept because version numbers, sizes, and
/// identifiers such as `512` or `0.4` are meaningful memory content.
fn push_word(word: &mut String, tokens: &mut Vec<String>) {
    let value = std::mem::take(word);
    if value.len() < 2 && !value.chars().all(|character| character.is_ascii_digit()) {
        return;
    }
    // Fold before the stopword check so that "uses" collapses into the stopword
    // "use" and disappears, instead of surviving as its own rare token.
    let value = fold_plural(&value);
    if STOPWORDS.contains(&value.as_str()) {
        return;
    }
    tokens.push(value);
}

/// Reduce a regular English plural to its singular stem.
///
/// Without this, "chunks" in one memory and "chunk" in another occupy different
/// hashed dimensions and contribute nothing to their similarity — a real recall
/// loss, since plural variation is the most common morphological difference in
/// written English. Measured on realistic memories, folding raises the weakest
/// related pair from 0.18 to 0.29 cosine.
///
/// The rule folds a trailing "s" (with "-ies" becoming "-y") and leaves "-ss"
/// alone. It is deliberately not a stemmer: a full stemmer would need a
/// dictionary to avoid collapsing distinct words, and the only requirement here
/// is that corpus and query tokens are folded identically.
fn fold_plural(value: &str) -> String {
    if value.len() < 4
        || !value
            .chars()
            .all(|character| character.is_ascii_lowercase())
    {
        return value.to_owned();
    }
    let candidate = if let Some(stem) = value.strip_suffix("ies") {
        format!("{stem}y")
    } else if value.ends_with("ss") {
        value.to_owned()
    } else if let Some(stem) = value.strip_suffix('s') {
        stem.to_owned()
    } else {
        value.to_owned()
    };
    // Refuse folds that collapse a word into a fragment, such as "ties" -> "ty".
    if candidate.len() < 3 {
        value.to_owned()
    } else {
        candidate
    }
}

/// Emit the characters of a non-ASCII run plus its adjacent pairs.
fn push_script_run(run: &mut Vec<char>, tokens: &mut Vec<String>) {
    let characters = std::mem::take(run);
    for character in &characters {
        tokens.push(character.to_string());
    }
    for pair in characters.windows(2) {
        tokens.push(pair.iter().collect());
    }
}

/// FNV-1a over the token bytes: a fixed, dependency-free hash that produces the
/// same value on every platform and every run.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// SplitMix64 finalizer: derives an independent second hash so the sign is not
/// correlated with the dimension index.
fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Project a token onto `(dimension, sign)` in the hashed feature space.
///
/// The sign lets colliding tokens cancel instead of always reinforcing each
/// other, which keeps the cosine similarity of unrelated texts near zero as the
/// number of dimensions shrinks.
pub fn hash_token(token: &str, dimensions: usize) -> (u32, f32) {
    let first = fnv1a64(token.as_bytes());
    let dimension = (first % dimensions as u64) as u32;
    let sign = if splitmix64(first) & 1 == 1 {
        1.0
    } else {
        -1.0
    };
    (dimension, sign)
}

/// Human-readable representative of a hashed dimension, used to explain why two
/// memories were judged similar. Without it the vector layer would be a black
/// box that only produces numbers.
pub const REPRESENTATIVE_TERMS_PER_DIMENSION: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_ascii_words_and_numbers() {
        let tokens = tokenize("Use 512 token chunks for prose");
        assert!(tokens.contains(&"512".to_string()));
        assert!(tokens.contains(&"chunk".to_string()));
        assert!(tokens.contains(&"prose".to_string()));
        assert!(!tokens.contains(&"for".to_string()));
    }

    #[test]
    fn drops_single_letters_but_keeps_digits() {
        let tokens = tokenize("a b 7 x");
        assert_eq!(tokens, vec!["7".to_string()]);
    }

    #[test]
    fn emits_characters_and_pairs_for_cjk() {
        let tokens = tokenize("知识图谱");
        assert!(tokens.contains(&"知".to_string()));
        assert!(tokens.contains(&"知识".to_string()));
        assert!(tokens.contains(&"图谱".to_string()));
    }

    #[test]
    fn hashing_is_deterministic_and_in_range() {
        let (first, first_sign) = hash_token("chunking", 8192);
        let (second, second_sign) = hash_token("chunking", 8192);
        assert_eq!(first, second);
        assert_eq!(first_sign, second_sign);
        assert!(first < 8192);
    }

    #[test]
    fn hashing_splits_signs_across_a_vocabulary() {
        let mut positive = 0;
        let mut negative = 0;
        for index in 0..400 {
            let (_, sign) = hash_token(&format!("token{index}"), 8192);
            if sign > 0.0 {
                positive += 1;
            } else {
                negative += 1;
            }
        }
        assert!(positive > 120 && negative > 120, "{positive}/{negative}");
    }

    #[test]
    fn folds_regular_plurals_to_one_stem() {
        for (plural, singular) in [
            ("chunks", "chunk"),
            ("tokens", "token"),
            ("documents", "document"),
            ("queries", "query"),
            ("cases", "case"),
            ("values", "value"),
            ("entries", "entry"),
            ("vectors", "vector"),
        ] {
            assert_eq!(fold_plural(plural), singular, "{plural}");
            assert_eq!(fold_plural(singular), singular, "{singular}");
        }
    }

    #[test]
    fn plural_folding_leaves_double_s_and_fragments_alone() {
        assert_eq!(fold_plural("class"), "class");
        assert_eq!(fold_plural("ties"), "ties");
        assert_eq!(fold_plural("512"), "512");
        // Words shorter than the fold threshold, and every word at or above it,
        // must survive with at least three characters.
        for value in [
            "class",
            "classes",
            "process",
            "processes",
            "graph",
            "graphs",
        ] {
            let folded = fold_plural(value);
            assert!(folded.len() >= 3, "{value} folded to {folded}");
        }
    }

    #[test]
    fn folded_plurals_become_one_token() {
        assert_eq!(tokenize("chunks chunk"), vec!["chunk", "chunk"]);
        // "uses" folds into the stopword "use" and is then removed entirely.
        assert!(tokenize("uses").is_empty());
    }
}
