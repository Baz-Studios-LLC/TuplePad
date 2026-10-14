//! Detecting, decoding and re-encoding file contents.

use encoding_rs::{Encoding, UTF_8, UTF_16BE, UTF_16LE};
use std::io::Write;

use crate::error::{CoreError, CoreResult};

/// How a document's bytes map to text, as shown in the status bar.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct TextEncoding {
    /// The WHATWG name, e.g. "UTF-8", "UTF-16LE", "windows-1252", "Shift_JIS".
    pub name: String,
    pub has_bom: bool,
}

impl TextEncoding {
    pub fn utf8() -> Self {
        TextEncoding { name: UTF_8.name().to_string(), has_bom: false }
    }

    pub(crate) fn resolve(&self) -> CoreResult<&'static Encoding> {
        Encoding::for_label(self.name.as_bytes())
            .ok_or_else(|| CoreError::UnknownEncoding { name: self.name.clone() })
    }
}

pub(crate) struct Decoded {
    pub text: String,
    pub encoding: TextEncoding,
    /// True when some bytes weren't valid in the detected encoding and were replaced with U+FFFD.
    pub had_errors: bool,
}

/// Decodes raw file bytes, detecting the encoding from a BOM, UTF-8 validity,
/// UTF-16 byte patterns and finally chardetng's statistical guess.
pub(crate) fn decode(bytes: Vec<u8>) -> Decoded {
    if let Some((encoding, bom_len)) = Encoding::for_bom(&bytes) {
        let (text, had_errors) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return Decoded {
            text: text.into_owned(),
            encoding: TextEncoding { name: encoding.name().to_string(), has_bom: true },
            had_errors,
        };
    }

    // NUL bytes are valid UTF-8, so BOM-less UTF-16 has to be ruled out first.
    if let Some(encoding) = sniff_utf16(&bytes) {
        let (text, had_errors) = encoding.decode_without_bom_handling(&bytes);
        return Decoded {
            text: text.into_owned(),
            encoding: TextEncoding { name: encoding.name().to_string(), has_bom: false },
            had_errors,
        };
    }

    let bytes = match String::from_utf8(bytes) {
        Ok(text) => return Decoded { text, encoding: TextEncoding::utf8(), had_errors: false },
        Err(error) => error.into_bytes(),
    };

    let encoding = {
        let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
        detector.feed(&bytes, true);
        detector.guess(None, chardetng::Utf8Detection::Allow)
    };
    let (text, had_errors) = encoding.decode_without_bom_handling(&bytes);
    Decoded {
        text: text.into_owned(),
        encoding: TextEncoding { name: encoding.name().to_string(), has_bom: false },
        had_errors,
    }
}

/// BOM-less UTF-16 shows up as ASCII text with a NUL in every other byte.
fn sniff_utf16(bytes: &[u8]) -> Option<&'static Encoding> {
    let sample = &bytes[..bytes.len().min(4096) & !1];
    if sample.len() < 4 {
        return None;
    }
    let pairs = sample.len() / 2;
    let even_nuls = sample.iter().step_by(2).filter(|&&b| b == 0).count();
    let odd_nuls = sample.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
    if odd_nuls * 10 >= pairs * 4 && even_nuls * 10 < pairs {
        Some(UTF_16LE)
    } else if even_nuls * 10 >= pairs * 4 && odd_nuls * 10 < pairs {
        Some(UTF_16BE)
    } else {
        None
    }
}

/// Streams text chunks into `out` in the given encoding, refusing to silently
/// replace characters the encoding can't represent.
pub(crate) fn write_encoded<'a>(
    out: &mut impl Write,
    chunks: impl Iterator<Item = &'a str>,
    encoding: &TextEncoding,
) -> CoreResult<()> {
    let resolved = encoding.resolve()?;

    if resolved == UTF_16LE || resolved == UTF_16BE {
        let little = resolved == UTF_16LE;
        if encoding.has_bom {
            out.write_all(if little { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] })?;
        }
        let mut buf = Vec::new();
        for chunk in chunks {
            buf.clear();
            for unit in chunk.encode_utf16() {
                buf.extend_from_slice(&if little { unit.to_le_bytes() } else { unit.to_be_bytes() });
            }
            out.write_all(&buf)?;
        }
        return Ok(());
    }

    if resolved == UTF_8 {
        if encoding.has_bom {
            out.write_all(&[0xEF, 0xBB, 0xBF])?;
        }
        for chunk in chunks {
            out.write_all(chunk.as_bytes())?;
        }
        return Ok(());
    }

    let mut encoder = resolved.new_encoder();
    let mut buf = vec![0u8; 64 * 1024];
    let mut chunks = chunks.peekable();
    while let Some(chunk) = chunks.next() {
        let last = chunks.peek().is_none();
        let mut src = chunk;
        loop {
            let (result, read, written) =
                encoder.encode_from_utf8_without_replacement(src, &mut buf, last);
            out.write_all(&buf[..written])?;
            src = &src[read..];
            match result {
                encoding_rs::EncoderResult::InputEmpty => break,
                encoding_rs::EncoderResult::OutputFull => continue,
                encoding_rs::EncoderResult::Unmappable(character) => {
                    return Err(CoreError::Unmappable {
                        encoding: resolved.name().to_string(),
                        character: character.to_string(),
                    });
                }
            }
        }
    }
    Ok(())
}

/// Every encoding TuplePad offers in its menus, in display order.
#[uniffi::export]
pub fn supported_encodings() -> Vec<String> {
    [
        "UTF-8",
        "UTF-16LE",
        "UTF-16BE",
        "windows-1252",
        "ISO-8859-1",
        "ISO-8859-2",
        "ISO-8859-15",
        "windows-1250",
        "windows-1251",
        "KOI8-R",
        "macintosh",
        "Shift_JIS",
        "EUC-JP",
        "ISO-2022-JP",
        "GBK",
        "gb18030",
        "Big5",
        "EUC-KR",
    ]
    .iter()
    .filter_map(|label| Encoding::for_label(label.as_bytes()))
    .fold(Vec::new(), |mut names, encoding| {
        // Some labels alias the same encoding (ISO-8859-1 is windows-1252 on the web).
        let name = encoding.name().to_string();
        if !names.contains(&name) {
            names.push(name);
        }
        names
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(text: &str, encoding: &TextEncoding) -> CoreResult<Vec<u8>> {
        let mut out = Vec::new();
        write_encoded(&mut out, std::iter::once(text), encoding)?;
        Ok(out)
    }

    #[test]
    fn detects_plain_utf8() {
        let decoded = decode("héllo 👋".as_bytes().to_vec());
        assert_eq!(decoded.text, "héllo 👋");
        assert_eq!(decoded.encoding, TextEncoding::utf8());
    }

    #[test]
    fn detects_bom_encodings() {
        let decoded = decode(vec![0xEF, 0xBB, 0xBF, b'h', b'i']);
        assert_eq!(decoded.text, "hi");
        assert!(decoded.encoding.has_bom);

        let decoded = decode(vec![0xFF, 0xFE, b'h', 0, b'i', 0]);
        assert_eq!(decoded.text, "hi");
        assert_eq!(decoded.encoding.name, "UTF-16LE");
    }

    #[test]
    fn detects_bomless_utf16() {
        let bytes: Vec<u8> = "hello world\n".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let decoded = decode(bytes);
        assert_eq!(decoded.encoding.name, "UTF-16LE");
        assert_eq!(decoded.text, "hello world\n");
    }

    #[test]
    fn detects_legacy_encoding() {
        // "café crème" in windows-1252.
        let decoded = decode(b"caf\xe9 cr\xe8me, d\xe9j\xe0 vu".to_vec());
        assert_eq!(decoded.text, "café crème, déjà vu");
        assert_eq!(decoded.encoding.name, "windows-1252");
    }

    #[test]
    fn round_trips_every_encoding_that_can_hold_ascii() {
        for name in supported_encodings() {
            let encoding = TextEncoding { name: name.clone(), has_bom: false };
            let bytes = encode("plain text\r\n", &encoding).unwrap();
            let resolved = encoding.resolve().unwrap();
            let (text, _, errors) = resolved.decode(&bytes);
            assert!(!errors, "{name}");
            assert_eq!(text, "plain text\r\n", "{name}");
        }
    }

    #[test]
    fn refuses_unmappable_characters() {
        let encoding = TextEncoding { name: "windows-1252".into(), has_bom: false };
        let error = encode("price: 5€ 👋", &encoding).unwrap_err();
        assert!(matches!(error, CoreError::Unmappable { character, .. } if character == "👋"));
    }

    #[test]
    fn writes_boms() {
        let utf8 = TextEncoding { name: "UTF-8".into(), has_bom: true };
        assert_eq!(encode("a", &utf8).unwrap(), vec![0xEF, 0xBB, 0xBF, b'a']);
        let utf16 = TextEncoding { name: "UTF-16BE".into(), has_bom: true };
        assert_eq!(encode("a", &utf16).unwrap(), vec![0xFE, 0xFF, 0, b'a']);
    }
}
