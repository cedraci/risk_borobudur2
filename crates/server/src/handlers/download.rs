//! `Content-Disposition` for the xlsx exports.
//!
//! One helper, because both evidence exports build the same header out of a
//! portfolio name and both got it wrong the same way (finding I1).
//!
//! Both interpolated the portfolio name into a bare `filename="…"` and handed
//! the result to `HeaderValue::from_str`. What that actually did, measured
//! rather than assumed (the review predicted a 500 for the accented case, and
//! that half is wrong — `http`'s `HeaderValue::is_valid` is
//! `b >= 32 && b != 127 || b == b'\t'`, which admits obs-text, i.e. every byte
//! from 0x80 up):
//!
//! - **A non-ASCII name** (`Borobudur Actions Européennes`) produced a 200
//!   whose header carried raw UTF-8 bytes in a field HTTP defines as
//!   ISO-8859-1, so the browser saved
//!   `Borobudur Actions EuropÃ©ennes.xlsx`. This product's funds are
//!   French-named, so that is the NORMAL case, not an edge case.
//! - **A quote in the name** (`Fonds "Alpha"`) closed the quoted-string early
//!   and clients truncated the filename there (M7).
//! - **A control character** in the name — `valid_name`
//!   (`handlers::portfolios`) only trims and rejects empty, so
//!   `{"name":"A\nB"}` is accepted — DID fail `is_valid`, and
//!   `.map_err(anyhow::Error::from)?` turned that into
//!   `AppError::Internal` → 500 `{"detail":"internal error"}`, naming no
//!   cause and unreachable for that fund thereafter. (Not a header-injection
//!   vector: the value was rejected, not split.)
//!
//! RFC 6266/5987 answers all three at once.

use axum::http::HeaderValue;

/// The characters RFC 5987 §3.2.1 allows unescaped in an `ext-value`
/// (`attr-char`). Everything else is percent-encoded.
fn is_attr_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'!' | b'#' | b'$' | b'&' | b'+' | b'-' | b'.'
        | b'^' | b'_' | b'`' | b'|' | b'~')
}

/// `attachment; filename="…"; filename*=UTF-8''…` for `filename`.
///
/// Two forms, per RFC 6266 §4.3: an ASCII `filename=` that any client
/// understands, and the `filename*` `ext-value` that carries the real name.
/// Clients that understand both prefer `filename*`, so an accented fund name
/// arrives intact and an old client still gets a usable name instead of a 500.
///
/// The ASCII fallback replaces every character outside printable ASCII with
/// `_`, and drops `"` and `\` (finding M7): a fund named `Fonds "Alpha"`
/// otherwise produced a malformed quoted-string that clients truncate at the
/// second quote. `\r`, `\n` and `\t` fall out of the same filter, so a name
/// carrying one can no longer 500 the export — and no header-injection vector
/// appears either, since the value is printable ASCII by construction.
pub fn attachment(filename: &str) -> HeaderValue {
    let ascii: String = filename.chars()
        .map(|c| match c {
            '"' | '\\' => '_',
            c if c.is_ascii_graphic() || c == ' ' => c,
            _ => '_',
        })
        .collect();
    let mut encoded = String::with_capacity(filename.len());
    for b in filename.as_bytes() {
        if is_attr_char(*b) {
            encoded.push(*b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    let value = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    // Printable ASCII by construction: `ascii` is filtered to it, `encoded` is
    // `attr-char` and percent escapes, and the literals are ASCII. `expect`
    // rather than a silent fallback so a future edit that breaks the
    // construction fails loudly in tests rather than shipping a wrong name.
    HeaderValue::from_str(&value).expect("built from printable ASCII only")
}

#[cfg(test)]
mod tests {
    use super::attachment;

    #[test]
    fn a_french_fund_name_survives_instead_of_500ing() {
        let v = attachment("Breach register - Borobudur Actions Européennes - 2026-08-21.xlsx");
        let s = v.to_str().unwrap();
        // The ASCII half stays a well-formed quoted-string...
        assert!(s.contains(r#"filename="Breach register - Borobudur Actions Europ_ennes - 2026-08-21.xlsx""#),
            "{s}");
        // ...and the real name travels in the RFC 5987 form. `é` is UTF-8
        // 0xC3 0xA9.
        assert!(s.contains("filename*=UTF-8''"), "{s}");
        assert!(s.contains("Europ%C3%A9ennes"), "{s}");
    }

    #[test]
    fn quotes_and_backslashes_cannot_break_out_of_the_quoted_string() {
        let v = attachment(r#"Fonds "Alpha" \ Beta.xlsx"#);
        let s = v.to_str().unwrap();
        let quoted = s.strip_prefix("attachment; filename=\"").unwrap();
        let (inside, _) = quoted.split_once('"').unwrap();
        assert_eq!(inside, "Fonds _Alpha_ _ Beta.xlsx",
            "the quoted-string must end where the filename ends: {s}");
        assert!(s.contains("%22"), "the real name still carries its quotes: {s}");
    }

    #[test]
    fn control_characters_never_reach_the_header() {
        let v = attachment("a\r\nX-Evil: 1\tb.xlsx");
        let s = v.to_str().unwrap();
        assert!(!s.contains('\r') && !s.contains('\n') && !s.contains('\t'), "{s}");
        assert!(s.contains(r#"filename="a__X-Evil: 1_b.xlsx""#), "{s}");
    }
}
