//! Response decompression for subgraph HTTP responses.
//!
//! When a subgraph is configured with `traffic_shaping.*.accept_encoding`, the router
//! advertises those encodings via `Accept-Encoding` (see [`accept_encoding_header_value`])
//! and decodes the response body according to its `Content-Encoding` header
//! (see [`decompress_response`]).

use std::fmt;
use std::io::Read;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue};

use hive_router_config::traffic_shaping::SubgraphAcceptEncoding;

/// Builds the `Accept-Encoding` header value from the configured encodings, preserving order.
/// Returns `None` when the list is empty (compression negotiation disabled).
pub fn accept_encoding_header_value(encodings: &[SubgraphAcceptEncoding]) -> Option<HeaderValue> {
    if encodings.is_empty() {
        return None;
    }

    let joined = encodings
        .iter()
        .map(|e| e.as_token())
        .collect::<Vec<_>>()
        .join(", ");

    // Values come from a closed enum of ASCII tokens, so this never fails.
    HeaderValue::from_str(&joined).ok()
}

/// A `Content-Encoding` the router knows how to decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentEncoding {
    Identity,
    Zstd,
    Gzip,
    Brotli,
    Deflate,
}

impl ContentEncoding {
    /// Parses the response `Content-Encoding` header. A missing, empty, or `identity`
    /// header maps to [`ContentEncoding::Identity`]. Stacked encodings (e.g. `gzip, br`)
    /// and unknown encodings are rejected.
    fn from_headers(headers: &HeaderMap) -> Result<Self, DecompressError> {
        let Some(value) = headers.get(http::header::CONTENT_ENCODING) else {
            return Ok(ContentEncoding::Identity);
        };

        let value = value
            .to_str()
            .map_err(|_| DecompressError::InvalidHeader)?
            .trim();

        if value.is_empty() || value.eq_ignore_ascii_case("identity") {
            return Ok(ContentEncoding::Identity);
        }

        // Reject multiple stacked encodings: decoding `gzip, deflate` by peeling layers
        // is very rare and not worth supporting.
        if value.as_bytes().contains(&b',') {
            return Err(DecompressError::Unsupported(value.to_string()));
        }

        if value.eq_ignore_ascii_case("zstd") {
            Ok(ContentEncoding::Zstd)
        } else if value.eq_ignore_ascii_case("gzip") || value.eq_ignore_ascii_case("x-gzip") {
            Ok(ContentEncoding::Gzip)
        } else if value.eq_ignore_ascii_case("br") {
            Ok(ContentEncoding::Brotli)
        } else if value.eq_ignore_ascii_case("deflate") {
            Ok(ContentEncoding::Deflate)
        } else {
            Err(DecompressError::Unsupported(value.to_string()))
        }
    }

    fn decode(self, body: Bytes) -> Result<Bytes, DecompressError> {
        match self {
            ContentEncoding::Identity => Ok(body),
            ContentEncoding::Zstd => {
                // `decode_all` transparently handles single- and multi-frame zstd streams.
                let out = zstd::stream::decode_all(body.as_ref())
                    .map_err(|source| DecompressError::Failed { encoding: "zstd", source })?;
                Ok(Bytes::from(out))
            }
            ContentEncoding::Gzip => {
                decode_reader(flate2::read::GzDecoder::new(body.as_ref()), body.len(), "gzip")
            }
            ContentEncoding::Brotli => decode_reader(
                brotli::Decompressor::new(body.as_ref(), 4096),
                body.len(),
                "br",
            ),
            ContentEncoding::Deflate => decode_reader(
                flate2::read::ZlibDecoder::new(body.as_ref()),
                body.len(),
                "deflate",
            ),
        }
    }
}

fn decode_reader<R: Read>(
    mut decoder: R,
    compressed_len: usize,
    encoding: &'static str,
) -> Result<Bytes, DecompressError> {
    // Payloads are typically several times larger than their compressed form.
    let mut out = Vec::with_capacity(compressed_len.saturating_mul(4));
    decoder
        .read_to_end(&mut out)
        .map_err(|source| DecompressError::Failed { encoding, source })?;
    Ok(Bytes::from(out))
}

/// Decodes `body` according to the response `Content-Encoding` header.
///
/// Returns the (possibly unchanged) body and whether decompression actually happened.
/// When the encoding is `identity`/absent the body is returned untouched with `false`.
pub fn decompress_response(
    headers: &HeaderMap,
    body: Bytes,
) -> Result<(Bytes, bool), DecompressError> {
    let encoding = ContentEncoding::from_headers(headers)?;
    if encoding == ContentEncoding::Identity {
        return Ok((body, false));
    }
    let decoded = encoding.decode(body)?;
    Ok((decoded, true))
}

/// Failure while negotiating or decoding a compressed subgraph response.
#[derive(Debug)]
pub enum DecompressError {
    /// The `Content-Encoding` header was not valid UTF-8/ASCII.
    InvalidHeader,
    /// The `Content-Encoding` was unknown or stacked (e.g. `gzip, br`).
    Unsupported(String),
    /// The body could not be decoded with the advertised encoding.
    Failed {
        encoding: &'static str,
        source: std::io::Error,
    },
}

impl DecompressError {
    /// A short, non-sensitive label of the offending encoding, for telemetry/error context.
    pub fn encoding_label(&self) -> String {
        match self {
            DecompressError::InvalidHeader => "invalid".to_string(),
            DecompressError::Unsupported(e) => e.clone(),
            DecompressError::Failed { encoding, .. } => (*encoding).to_string(),
        }
    }
}

impl fmt::Display for DecompressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecompressError::InvalidHeader => {
                write!(f, "invalid content-encoding header")
            }
            DecompressError::Unsupported(encoding) => {
                write!(f, "unsupported content-encoding '{encoding}'")
            }
            DecompressError::Failed { encoding, source } => {
                write!(f, "failed to decode '{encoding}' body: {source}")
            }
        }
    }
}

impl std::error::Error for DecompressError {}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_router_config::traffic_shaping::SubgraphAcceptEncoding::*;
    use std::io::Write;

    fn headers_with(encoding: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            http::header::CONTENT_ENCODING,
            HeaderValue::from_str(encoding).unwrap(),
        );
        h
    }

    const SAMPLE: &[u8] = br#"{"data":{"products":[{"id":1},{"id":2}]},"errors":null}"#;

    #[test]
    fn accept_encoding_header_orders_and_joins() {
        assert_eq!(accept_encoding_header_value(&[]), None);
        assert_eq!(
            accept_encoding_header_value(&[Zstd, Gzip]).unwrap(),
            HeaderValue::from_static("zstd, gzip")
        );
        assert_eq!(
            accept_encoding_header_value(&[Br]).unwrap(),
            HeaderValue::from_static("br")
        );
    }

    #[test]
    fn identity_and_absent_are_passthrough() {
        let body = Bytes::from_static(SAMPLE);
        let (out, decoded) = decompress_response(&HeaderMap::new(), body.clone()).unwrap();
        assert!(!decoded);
        assert_eq!(out, body);

        let (out, decoded) = decompress_response(&headers_with("identity"), body.clone()).unwrap();
        assert!(!decoded);
        assert_eq!(out, body);
    }

    #[test]
    fn zstd_roundtrip() {
        let compressed = zstd::stream::encode_all(SAMPLE, 3).unwrap();
        let (out, decoded) =
            decompress_response(&headers_with("zstd"), Bytes::from(compressed)).unwrap();
        assert!(decoded);
        assert_eq!(out.as_ref(), SAMPLE);
    }

    #[test]
    fn gzip_roundtrip() {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(SAMPLE).unwrap();
        let compressed = enc.finish().unwrap();
        let (out, decoded) =
            decompress_response(&headers_with("gzip"), Bytes::from(compressed)).unwrap();
        assert!(decoded);
        assert_eq!(out.as_ref(), SAMPLE);
    }

    #[test]
    fn deflate_roundtrip() {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(SAMPLE).unwrap();
        let compressed = enc.finish().unwrap();
        let (out, decoded) =
            decompress_response(&headers_with("deflate"), Bytes::from(compressed)).unwrap();
        assert!(decoded);
        assert_eq!(out.as_ref(), SAMPLE);
    }

    #[test]
    fn stacked_and_unknown_are_rejected() {
        let body = Bytes::from_static(SAMPLE);
        assert!(matches!(
            decompress_response(&headers_with("gzip, br"), body.clone()),
            Err(DecompressError::Unsupported(_))
        ));
        assert!(matches!(
            decompress_response(&headers_with("snappy"), body),
            Err(DecompressError::Unsupported(_))
        ));
    }

    #[test]
    fn corrupt_body_fails() {
        let garbage = Bytes::from_static(b"not really zstd");
        assert!(matches!(
            decompress_response(&headers_with("zstd"), garbage),
            Err(DecompressError::Failed { .. })
        ));
    }
}
