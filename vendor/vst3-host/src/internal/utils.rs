//! Internal utility functions

use std::os::raw::c_char;
use vst3::Steinberg::{Vst::String128, TUID};

/// Convert a C-style string to Rust String.
///
/// Takes `c_char` (not `i8`) because `c_char` is unsigned on some platforms (e.g. ARM
/// Linux) and signed on others (macOS, x86) — the VST3 bindings use `c_char`.
// The `c as u8` cast is required on platforms where `c_char` is `i8`; on platforms where
// `c_char` is already `u8` clippy sees it as redundant. Keep it for portability.
#[allow(clippy::unnecessary_cast)]
pub fn c_str_to_string(c_str: &[c_char]) -> String {
    let end = c_str.iter().position(|&c| c == 0).unwrap_or(c_str.len());
    let bytes: Vec<u8> = c_str[..end].iter().map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).to_string()
}

/// Convert VST3 String128 (UTF-16) to Rust String
pub fn vst_string_to_string(vst_str: &String128) -> String {
    let mut utf16_vec = Vec::new();

    for &ch in vst_str.iter() {
        if ch == 0 {
            break;
        }
        utf16_vec.push(ch);
    }

    String::from_utf16_lossy(&utf16_vec)
}

/// Format a raw VST3 `TUID` as the canonical 32-character FUID text used in preset files.
///
/// VST3 uses COM-compatible GUID byte order for the first three GUID fields on Windows. The
/// textual representation is platform-independent, so those fields must be put back into
/// network/display order before hex encoding.
pub fn format_class_uid(cid: &[c_char; 16]) -> String {
    format_class_uid_for_platform(cid, cfg!(target_os = "windows"))
}

#[allow(clippy::unnecessary_cast)]
fn format_class_uid_for_platform(cid: &[c_char; 16], com_compatible: bool) -> String {
    const COM_TO_CANONICAL: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let order = if com_compatible {
        &COM_TO_CANONICAL
    } else {
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
    };
    order
        .iter()
        .map(|&index| format!("{:02X}", cid[index] as u8))
        .collect()
}

/// Compare canonical class-id text while accepting the raw COM-compatible spelling emitted by
/// older Windows versions of this crate.
pub fn class_uid_matches(expected_canonical: &str, candidate: &str) -> bool {
    class_uid_matches_for_platform(expected_canonical, candidate, cfg!(target_os = "windows"))
}

/// Parse canonical 32-character FUID text into the platform-native `TUID` byte layout.
///
/// The public API deliberately accepts only the canonical, separator-free spelling. On
/// Windows the first three GUID fields are converted to COM-compatible little-endian order
/// before being passed to a VST3 interface.
pub(crate) fn parse_class_uid(uid: &str) -> Option<TUID> {
    parse_class_uid_for_platform(uid, cfg!(target_os = "windows"))
}

fn parse_class_uid_for_platform(uid: &str, com_compatible: bool) -> Option<TUID> {
    if !is_class_uid(uid) {
        return None;
    }

    let mut canonical = [0_u8; 16];
    for (index, slot) in canonical.iter_mut().enumerate() {
        let offset = index * 2;
        *slot = u8::from_str_radix(&uid[offset..offset + 2], 16).ok()?;
    }

    const COM_TO_CANONICAL: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let order = if com_compatible {
        &COM_TO_CANONICAL
    } else {
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
    };
    let mut native = [0 as c_char; 16];
    for (native_index, &canonical_index) in order.iter().enumerate() {
        native[native_index] = canonical[canonical_index] as c_char;
    }
    Some(native)
}

fn class_uid_matches_for_platform(
    expected_canonical: &str,
    candidate: &str,
    com_compatible: bool,
) -> bool {
    if !is_class_uid(expected_canonical) || !is_class_uid(candidate) {
        return false;
    }
    if expected_canonical.eq_ignore_ascii_case(candidate) {
        return true;
    }
    com_compatible
        && swap_com_fields(expected_canonical)
            .is_some_and(|legacy| legacy.eq_ignore_ascii_case(candidate))
}

fn is_class_uid(uid: &str) -> bool {
    uid.len() == 32 && uid.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn swap_com_fields(uid: &str) -> Option<String> {
    if !is_class_uid(uid) {
        return None;
    }
    const COM_TO_CANONICAL: [usize; 16] = [3, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    let bytes = uid.as_bytes();
    let mut out = String::with_capacity(32);
    for index in COM_TO_CANONICAL {
        out.push(bytes[index * 2] as char);
        out.push(bytes[index * 2 + 1] as char);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_uid_format_is_canonical_on_com_platforms() {
        let raw = [
            0x78u8 as c_char,
            0x56u8 as c_char,
            0x34u8 as c_char,
            0x12u8 as c_char,
            0xBCu8 as c_char,
            0x9Au8 as c_char,
            0xF0u8 as c_char,
            0xDEu8 as c_char,
            0x11u8 as c_char,
            0x22u8 as c_char,
            0x33u8 as c_char,
            0x44u8 as c_char,
            0x55u8 as c_char,
            0x66u8 as c_char,
            0x77u8 as c_char,
            0x88u8 as c_char,
        ];
        assert_eq!(
            format_class_uid_for_platform(&raw, true),
            "123456789ABCDEF01122334455667788"
        );
        assert_eq!(
            format_class_uid_for_platform(&raw, false),
            "78563412BC9AF0DE1122334455667788"
        );
    }

    #[test]
    fn windows_match_accepts_previous_raw_com_spelling() {
        let canonical = "123456789ABCDEF01122334455667788";
        let legacy = "78563412BC9AF0DE1122334455667788";
        assert!(class_uid_matches_for_platform(canonical, legacy, true));
        assert!(class_uid_matches_for_platform(
            canonical,
            &canonical.to_ascii_lowercase(),
            true
        ));
        assert!(!class_uid_matches_for_platform(canonical, legacy, false));
        assert!(!class_uid_matches_for_platform(
            canonical,
            "not-a-class-id",
            true
        ));
    }

    #[test]
    fn class_uid_parser_is_strict_and_uses_native_com_order() {
        let canonical = "123456789ABCDEF01122334455667788";
        let plain = parse_class_uid_for_platform(canonical, false).unwrap();
        let com = parse_class_uid_for_platform(canonical, true).unwrap();
        #[allow(clippy::unnecessary_cast)]
        let bytes = |uid: TUID| uid.map(|byte| byte as u8);

        assert_eq!(
            bytes(plain),
            [
                0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
                0x77, 0x88,
            ]
        );
        assert_eq!(
            bytes(com),
            [
                0x78, 0x56, 0x34, 0x12, 0xBC, 0x9A, 0xF0, 0xDE, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66,
                0x77, 0x88,
            ]
        );

        assert!(parse_class_uid_for_platform("", false).is_none());
        assert!(parse_class_uid_for_platform("1234", false).is_none());
        assert!(
            parse_class_uid_for_platform("12345678-9ABC-DEF0-1122-334455667788", false).is_none()
        );
        assert!(parse_class_uid_for_platform("123456789ABCDEF0112233445566778G", false).is_none());
        assert!(
            parse_class_uid_for_platform("123456789ABCDEF0112233445566778800", false).is_none()
        );
    }
}
