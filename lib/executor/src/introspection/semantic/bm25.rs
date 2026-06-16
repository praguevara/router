//! A compact in-memory [BM25](https://en.wikipedia.org/wiki/Okapi_BM25)
//! retrieval index over the schema corpus.
//!
//! The RFC leaves the indexing strategy implementation-defined and the reference
//! implementation defaults to BM25; this is a dependency-free implementation
//! using a standard inverted index for retrieval.

use ahash::HashMap;

/// BM25 term-frequency saturation parameter.
const K1: f64 = 1.2;
/// BM25 length-normalization parameter.
const B: f64 = 0.75;

#[derive(Debug)]
struct Posting {
    doc: u32,
    tf: u32,
}

/// An immutable BM25 index. Documents are referred to by their insertion index.
#[derive(Debug, Default)]
pub struct Bm25Index {
    n_docs: usize,
    avgdl: f64,
    doc_len: Vec<u32>,
    postings: HashMap<String, Vec<Posting>>,
}

/// Accumulates documents (as token lists) before building a [`Bm25Index`].
#[derive(Debug, Default)]
pub struct Bm25Builder {
    docs: Vec<Vec<String>>,
}

impl Bm25Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a document and returns its index (kept in sync by the caller with a
    /// parallel coordinate list).
    pub fn add(&mut self, tokens: Vec<String>) -> usize {
        let idx = self.docs.len();
        self.docs.push(tokens);
        idx
    }

    pub fn build(self) -> Bm25Index {
        let n_docs = self.docs.len();
        let mut doc_len = Vec::with_capacity(n_docs);
        let mut postings: HashMap<String, Vec<Posting>> = HashMap::default();
        let mut total_len: u64 = 0;

        for (doc_idx, tokens) in self.docs.iter().enumerate() {
            doc_len.push(tokens.len() as u32);
            total_len += tokens.len() as u64;

            let mut tf: HashMap<&str, u32> = HashMap::default();
            for t in tokens {
                *tf.entry(t.as_str()).or_insert(0) += 1;
            }
            for (term, freq) in tf {
                postings.entry(term.to_string()).or_default().push(Posting {
                    doc: doc_idx as u32,
                    tf: freq,
                });
            }
        }

        let avgdl = if n_docs > 0 {
            total_len as f64 / n_docs as f64
        } else {
            0.0
        };

        Bm25Index {
            n_docs,
            avgdl,
            doc_len,
            postings,
        }
    }
}

impl Bm25Index {
    /// Scores every document that contains at least one of `terms`, returning
    /// `(doc_index, raw_score)` pairs in arbitrary order. `terms` is expected to
    /// be deduplicated by the caller.
    pub fn score(&self, terms: &[String]) -> Vec<(usize, f64)> {
        let mut acc: HashMap<usize, f64> = HashMap::default();
        let n = self.n_docs as f64;

        for term in terms {
            let Some(postings) = self.postings.get(term) else {
                continue;
            };
            let df = postings.len() as f64;
            // BM25 idf with the +1 guard so it stays non-negative even for terms
            // that appear in more than half the corpus.
            let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();

            for p in postings {
                let tf = p.tf as f64;
                let dl = self.doc_len[p.doc as usize] as f64;
                let denom = tf + K1 * (1.0 - B + B * dl / self.avgdl);
                let contribution = idf * (tf * (K1 + 1.0)) / denom;
                *acc.entry(p.doc as usize).or_insert(0.0) += contribution;
            }
        }

        acc.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &[&str]) -> Vec<String> {
        s.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn ranks_more_relevant_documents_higher() {
        let mut b = Bm25Builder::new();
        let weather = b.add(toks(&["weather", "forecast", "today"])); // 0
        let taxis = b.add(toks(&["available", "taxis", "area"])); // 1
        let _air = b.add(toks(&["air", "quality", "index"])); // 2
        let index = b.build();

        let mut scores = index.score(&toks(&["weather"]));
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        assert_eq!(scores[0].0, weather);
        assert!(scores.iter().all(|(doc, _)| *doc != taxis));
    }

    #[test]
    fn empty_query_terms_match_nothing() {
        let mut b = Bm25Builder::new();
        b.add(toks(&["weather"]));
        let index = b.build();
        assert!(index.score(&[]).is_empty());
    }

    #[test]
    fn unknown_term_matches_nothing() {
        let mut b = Bm25Builder::new();
        b.add(toks(&["weather"]));
        let index = b.build();
        assert!(index.score(&toks(&["nonexistent"])).is_empty());
    }
}
