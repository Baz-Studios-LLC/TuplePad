//! Encode and decode text (Base64, URL, HTML entities, hex, quoted-printable), and hashes.

use std::io::Read;

use base64::Engine;
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

use crate::error::{CoreError, CoreResult};

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    Base64,
    Url,
    HtmlEntities,
    Hex,
    QuotedPrintable,
}

/// Encodes text (as UTF-8 bytes where the codec works on bytes).
#[uniffi::export]
pub fn encode_text(codec: Codec, text: String) -> String {
    match codec {
        Codec::Base64 => base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),
        Codec::Url => url_encode(&text),
        Codec::HtmlEntities => html_escape::encode_safe(&text).into_owned(),
        Codec::Hex => text.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
        Codec::QuotedPrintable => quoted_printable_encode(text.as_bytes()),
    }
}

/// Decodes text. Fails when the input isn't valid for the codec or doesn't decode to text.
#[uniffi::export]
pub fn decode_text(codec: Codec, text: String) -> CoreResult<String> {
    let bytes = match codec {
        Codec::Base64 => {
            let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            let engines = [
                &base64::engine::general_purpose::STANDARD,
                &base64::engine::general_purpose::STANDARD_NO_PAD,
                &base64::engine::general_purpose::URL_SAFE,
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            ];
            engines
                .iter()
                .find_map(|engine| engine.decode(&compact).ok())
                .ok_or_else(|| CoreError::invalid("This isn’t valid Base64."))?
        }
        Codec::Url => url_decode(&text)?,
        Codec::HtmlEntities => return Ok(html_escape::decode_html_entities(&text).into_owned()),
        Codec::Hex => hex_decode(&text)?,
        Codec::QuotedPrintable => quoted_printable_decode(&text)?,
    };
    String::from_utf8(bytes).map_err(|_| CoreError::invalid("The decoded bytes aren’t UTF-8 text."))
}

fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn url_decode(text: &str) -> CoreResult<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = text.get(i + 1..i + 3).filter(|h| h.len() == 2);
                let value = hex.and_then(|h| u8::from_str_radix(h, 16).ok());
                out.push(value.ok_or_else(|| CoreError::invalid(format!("“{}” isn’t a valid %-escape.", &text[i..(i + 3).min(text.len())])))?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    Ok(out)
}

fn hex_decode(text: &str) -> CoreResult<Vec<u8>> {
    let digits: String = text
        .split(|c: char| c.is_whitespace() || c == ',' || c == ':')
        .map(|part| part.trim_start_matches("0x").trim_start_matches("0X"))
        .collect();
    if digits.len() % 2 != 0 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CoreError::invalid("This isn’t hex: use pairs of 0–9 and a–f."));
    }
    Ok((0..digits.len()).step_by(2).map(|i| u8::from_str_radix(&digits[i..i + 2], 16).unwrap_or(0)).collect())
}

/// RFC 2045 quoted-printable, with soft line breaks keeping lines under 76 characters.
fn quoted_printable_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut line_length = 0;
    for (index, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' {
            out.push('\n');
            line_length = 0;
            continue;
        }
        // Spaces and tabs at the end of a line must be encoded.
        let at_line_end = bytes.get(index + 1).is_none_or(|&next| next == b'\n' || next == b'\r');
        let literal = (byte == b' ' || byte == b'\t') && !at_line_end || (33..=126).contains(&byte) && byte != b'=';
        let piece = if literal || byte == b'\r' && bytes.get(index + 1) == Some(&b'\n') { (byte as char).to_string() } else { format!("={byte:02X}") };
        if line_length + piece.len() > 75 {
            out.push_str("=\n");
            line_length = 0;
        }
        line_length += piece.len();
        out.push_str(&piece);
    }
    out
}

fn quoted_printable_decode(text: &str) -> CoreResult<Vec<u8>> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            // A soft line break.
            if bytes.get(i + 1) == Some(&b'\n') {
                i += 2;
                continue;
            }
            if bytes.get(i + 1) == Some(&b'\r') && bytes.get(i + 2) == Some(&b'\n') {
                i += 3;
                continue;
            }
            let value = text.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok());
            out.push(value.ok_or_else(|| CoreError::invalid("This isn’t valid quoted-printable text."))?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Ok(out)
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashAlgorithm {
    Md5,
    Sha1,
    Sha256,
    Sha512,
}

fn digest<D: Digest>(mut hasher: D, reader: &mut dyn Read) -> std::io::Result<String> {
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn hash_reader(algorithm: HashAlgorithm, reader: &mut dyn Read) -> std::io::Result<String> {
    match algorithm {
        HashAlgorithm::Md5 => digest(Md5::new(), reader),
        HashAlgorithm::Sha1 => digest(Sha1::new(), reader),
        HashAlgorithm::Sha256 => digest(Sha256::new(), reader),
        HashAlgorithm::Sha512 => digest(Sha512::new(), reader),
    }
}

/// The hash of text's UTF-8 bytes, as lowercase hex.
#[uniffi::export]
pub fn hash_text(algorithm: HashAlgorithm, text: String) -> String {
    hash_reader(algorithm, &mut text.as_bytes()).unwrap_or_default()
}

/// The hash of a file's bytes, read in chunks.
#[uniffi::export]
pub fn hash_file(algorithm: HashAlgorithm, path: String) -> CoreResult<String> {
    let mut file = std::fs::File::open(&path).map_err(CoreError::from)?;
    Ok(hash_reader(algorithm, &mut file)?)
}
