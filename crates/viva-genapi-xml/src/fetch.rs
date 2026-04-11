//! URL parsing and XML document retrieval utilities.

use std::future::Future;

use crate::XmlError;
use crate::util::parse_u64;

/// Address of the first URL register in the GigE Vision bootstrap register map.
/// GigE Vision spec: GevFirstURL at 0x0200 (512 bytes max).
const FIRST_URL_ADDRESS: u64 = 0x0200;
/// Maximum length of the first URL string.
const FIRST_URL_MAX_LEN: usize = 512;

/// Fetch the GenICam XML document using the provided memory reader closure.
///
/// The closure must return the requested number of bytes starting at the
/// provided address. It can internally perform chunked transfers.
pub async fn fetch_and_load_xml<F, Fut>(mut read_mem: F) -> Result<String, XmlError>
where
    F: FnMut(u64, usize) -> Fut,
    Fut: Future<Output = Result<Vec<u8>, XmlError>>,
{
    let url_bytes = read_mem(FIRST_URL_ADDRESS, FIRST_URL_MAX_LEN).await?;
    let url = first_cstring(&url_bytes)
        .ok_or_else(|| XmlError::Invalid("FirstURL register is empty".into()))?;
    let location = UrlLocation::parse(&url)?;
    match location {
        UrlLocation::Local { address, length } => {
            let xml_bytes = read_mem(address, length).await?;
            String::from_utf8(xml_bytes)
                .map_err(|err| XmlError::Xml(format!("invalid UTF-8: {err}")))
        }
        UrlLocation::LocalNamed(name) => Err(XmlError::Unsupported(format!(
            "named local URL '{name}' is not supported"
        ))),
        UrlLocation::Http(url) => Err(XmlError::Unsupported(format!(
            "HTTP retrieval is not implemented ({url})"
        ))),
        UrlLocation::File(path) => Err(XmlError::Unsupported(format!(
            "file URL '{path}' is not supported"
        ))),
    }
}

/// Extract the first null-terminated C string from a byte buffer.
fn first_cstring(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let slice = &bytes[..end];
    let value = String::from_utf8_lossy(slice).trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

/// Parsed URL location variants.
#[derive(Debug)]
enum UrlLocation {
    /// Memory-mapped local XML at a fixed address.
    Local { address: u64, length: usize },
    /// Named local XML resource (unsupported).
    #[allow(dead_code)]
    LocalNamed(String),
    /// HTTP(S) remote URL (unsupported).
    Http(String),
    /// File system URL (unsupported).
    File(String),
}

impl UrlLocation {
    fn parse(url: &str) -> Result<Self, XmlError> {
        let lower = url.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("local:") {
            // Use original URL (case-preserved) for the rest, at same offset.
            let rest_original = &url[url.len() - rest.len()..];
            parse_local_url(rest_original)
        } else if lower.starts_with("http://") || lower.starts_with("https://") {
            Ok(UrlLocation::Http(url.to_string()))
        } else if lower.starts_with("file://") {
            Ok(UrlLocation::File(url.to_string()))
        } else {
            Err(XmlError::Unsupported(format!("unknown URL scheme: {url}")))
        }
    }
}

/// Parse a `local:` URL into its components.
///
/// Supports two formats:
///
/// 1. **Key-value**: `local:address=0x10;length=0x3`
/// 2. **GenICam standard**: `Local:///filename;hex_address;hex_length`
///
/// In format 2, the filename is optional (can be just `///;addr;len`).
fn parse_local_url(rest: &str) -> Result<UrlLocation, XmlError> {
    // Strip the `///` prefix used by the standard GenICam URL format.
    let trimmed = rest.strip_prefix("///").unwrap_or(rest).trim();
    if trimmed.is_empty() {
        return Err(XmlError::Invalid("empty local URL".into()));
    }

    let parts: Vec<&str> = trimmed.split(';').collect();

    // GenICam standard format: filename;hex_address;hex_length (3 semicolon-separated parts).
    if parts.len() >= 3 {
        let addr_str = parts[parts.len() - 2].trim();
        let len_str = parts[parts.len() - 1].trim();
        if let (Ok(address), Ok(length)) = (
            u64::from_str_radix(addr_str, 16),
            u64::from_str_radix(len_str, 16),
        ) {
            return Ok(UrlLocation::Local {
                address,
                length: length as usize,
            });
        }
    }

    // Fall back to key-value parsing.
    let mut address = None;
    let mut length = None;
    for part in parts {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        if let Some((key, value)) = token.split_once('=') {
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "address" | "addr" | "offset" => {
                    address = Some(parse_u64(value)?);
                }
                "length" | "size" => {
                    let len = parse_u64(value)?;
                    length = Some(
                        len.try_into()
                            .map_err(|_| XmlError::Invalid("length does not fit usize".into()))?,
                    );
                }
                _ => {}
            }
        } else if token.starts_with("0x") {
            address = Some(parse_u64(token)?);
        }
        // Ignore unrecognized tokens (like the filename).
    }
    match (address, length) {
        (Some(address), Some(length)) => Ok(UrlLocation::Local { address, length }),
        _ => Err(XmlError::Invalid(format!("unsupported local URL: {rest}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fetch_local_xml() {
        let data = b"local:address=0x10;length=0x3\0".to_vec();
        let xml_payload = b"<a/>".to_vec();
        let loaded = fetch_and_load_xml(|addr, len| {
            let data = data.clone();
            let xml_payload = xml_payload.clone();
            async move {
                if addr == FIRST_URL_ADDRESS {
                    Ok(data)
                } else if addr == 0x10 && len == 0x3 {
                    Ok(xml_payload)
                } else {
                    Err(XmlError::Transport("unexpected read".into()))
                }
            }
        })
        .await
        .expect("load xml");
        assert_eq!(loaded, "<a/>");
    }
}
