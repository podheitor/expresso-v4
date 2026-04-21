//! CardDAV URL parsing.
//!
//! URL shapes served under /carddav:
//!   /carddav/<user>/                     → Home
//!   /carddav/<user>/<addressbook>/          → Addressbook collection
//!   /carddav/<user>/<addressbook>/<uid>.vcf → Contact resource
//!
//! Both `user` and `addressbook` are UUIDs. `uid` is the vCard UID (arbitrary
//! text — URL-decoded here).

use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum Target {
    Home { user_id: Uuid },
    Addressbook { user_id: Uuid, addressbook_id: Uuid },
    Contact { user_id: Uuid, addressbook_id: Uuid, uid: String },
    Unknown,
}

/// Classify a CardDAV request path.
pub fn classify(path: &str) -> Target {
    // Strip leading /carddav
    let rest = match path.strip_prefix("/carddav/") {
        Some(r) => r,
        None => return Target::Unknown,
    };

    // Trim trailing slash for matching
    let (segments_str, trailing_slash) = if let Some(s) = rest.strip_suffix('/') {
        (s, true)
    } else {
        (rest, false)
    };

    if segments_str.is_empty() {
        return Target::Unknown;
    }

    let segments: Vec<&str> = segments_str.split('/').collect();

    match segments.len() {
        1 if trailing_slash => parse_uuid(segments[0])
            .map(|u| Target::Home { user_id: u })
            .unwrap_or(Target::Unknown),
        2 if trailing_slash => {
            let u = parse_uuid(segments[0]);
            let c = parse_uuid(segments[1]);
            match (u, c) {
                (Some(u), Some(c)) => Target::Addressbook { user_id: u, addressbook_id: c },
                _ => Target::Unknown,
            }
        }
        3 if !trailing_slash => {
            let u = parse_uuid(segments[0]);
            let c = parse_uuid(segments[1]);
            let last = segments[2];
            let uid = last
                .strip_suffix(".vcf")
                .map(|s| percent_decode(s));
            match (u, c, uid) {
                (Some(u), Some(c), Some(uid)) =>
                    Target::Contact { user_id: u, addressbook_id: c, uid },
                _ => Target::Unknown,
            }
        }
        _ => Target::Unknown,
    }
}

fn parse_uuid(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s.trim()).ok()
}

/// Minimal percent-decoder (UTF-8 safe; falls back to lossy on invalid bytes).
pub fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h1), Some(h2)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((h1 << 4) | h2);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_home() {
        let u = Uuid::new_v4();
        let t = classify(&format!("/carddav/{u}/"));
        assert!(matches!(t, Target::Home { user_id } if user_id == u));
    }

    #[test]
    fn parse_addressbook() {
        let u = Uuid::new_v4();
        let c = Uuid::new_v4();
        let t = classify(&format!("/carddav/{u}/{c}/"));
        assert!(matches!(t, Target::Addressbook { user_id, addressbook_id } if user_id == u && addressbook_id == c));
    }

    #[test]
    fn parse_event() {
        let u = Uuid::new_v4();
        let c = Uuid::new_v4();
        let t = classify(&format!("/carddav/{u}/{c}/abc-123%40ex.vcf"));
        match t {
            Target::Contact { user_id, addressbook_id, uid } => {
                assert_eq!(user_id, u);
                assert_eq!(addressbook_id, c);
                assert_eq!(uid, "abc-123@ex");
            }
            _ => panic!("wrong target {t:?}"),
        }
    }

    #[test]
    fn unknown_paths() {
        assert!(matches!(classify("/carddav/"), Target::Unknown));
        assert!(matches!(classify("/carddav/not-a-uuid/"), Target::Unknown));
        assert!(matches!(classify("/foo/bar/"), Target::Unknown));
    }
}
