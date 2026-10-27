//! The visitor's jar: name identity, path scope and wall-clock expiry.
use skein_lib::Wall;

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Cookie {
    name: Box<[u8]>,
    value: Box<[u8]>,
    path: Box<[u8]>,
    expires: Option<u64>,
}

/// Parses without observing or changing a jar; ignored attributes grant no scope.
fn parse(header: &[u8], wall: Wall, limit: u32) -> Option<Cookie> {
    let mut parts = header.split(|byte| *byte == b';');
    let pair = parts.next()?.trim_ascii();
    let equals = pair.iter().position(|byte| *byte == b'=')?;
    let name = pair.get(..equals)?.trim_ascii();
    let value = pair.get(equals.checked_add(1)?..)?.trim_ascii();
    if name.is_empty() || !name.iter().all(|byte| token(*byte)) {
        return None;
    }
    let value = if value.starts_with(b"\"") { value.strip_prefix(b"\"")?.strip_suffix(b"\"")? } else { value };
    if !value.iter().all(|byte| matches!(byte, 0x21 | 0x23..=0x2b | 0x2d..=0x3a | 0x3c..=0x5b | 0x5d..=0x7e))
        || name.len().checked_add(value.len())? > usize::try_from(limit).expect("cookie bound fits")
    {
        return None;
    }
    let mut path: &[u8] = b"/";
    let mut expires = None;
    for part in parts {
        let part = part.trim_ascii();
        let Some(equals) = part.iter().position(|byte| *byte == b'=') else { continue };
        let attribute = part.get(..equals)?.trim_ascii();
        let value = part.get(equals.checked_add(1)?..)?.trim_ascii();
        if attribute.eq_ignore_ascii_case(b"path") {
            if value.starts_with(b"/") && value.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
                path = value;
            }
        } else if attribute.eq_ignore_ascii_case(b"max-age") {
            let seconds = std::str::from_utf8(value).ok()?.parse::<i64>().ok()?;
            expires = Some(if seconds <= 0 {
                0
            } else {
                wall.as_nanos().saturating_add(u64::try_from(seconds).ok()?.saturating_mul(1_000_000_000))
            });
        }
    }
    Some(Cookie { name: name.into(), value: value.into(), path: path.into(), expires })
}
fn token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}
fn under(request: &[u8], path: &[u8]) -> bool {
    let request = request.split(|byte| *byte == b'?').next().expect("one path");
    request == path || request.strip_prefix(path).is_some_and(|tail| path.ends_with(b"/") || tail.starts_with(b"/"))
}

pub(super) struct Jar {
    cookies: Vec<Cookie>,
    capacity: usize,
    limit: u32,
}
impl Jar {
    pub(super) fn new(capacity: u32, limit: u32) -> Self {
        let capacity = usize::try_from(capacity).expect("cookie count fits");
        Self { cookies: Vec::with_capacity(capacity), capacity, limit }
    }
    fn expire(&mut self, wall: Wall) {
        self.cookies.retain(|cookie| cookie.expires.is_none_or(|expires| expires > wall.as_nanos()));
    }
    pub(super) fn set(&mut self, header: &[u8], wall: Wall) {
        self.expire(wall);
        let Some(cookie) = parse(header, wall, self.limit) else { return };
        let at = self.cookies.iter().position(|held| held.name == cookie.name);
        if cookie.expires.is_some_and(|expires| expires <= wall.as_nanos()) {
            if let Some(at) = at {
                self.cookies.remove(at);
            }
        } else if let Some(at) = at {
            self.cookies[at] = cookie;
        } else if self.cookies.len() < self.capacity {
            self.cookies.push(cookie);
        }
    }
    pub(super) fn header(&mut self, path: &[u8], wall: Wall) -> Option<Box<[u8]>> {
        self.expire(wall);
        let mut value = Vec::new();
        for cookie in &self.cookies {
            if under(path, &cookie.path) {
                if !value.is_empty() {
                    value.extend_from_slice(b"; ");
                }
                value.extend_from_slice(&cookie.name);
                value.push(b'=');
                value.extend_from_slice(&cookie.value);
            }
        }
        (!value.is_empty()).then(|| value.into_boxed_slice())
    }
    pub(super) fn clear(&mut self) {
        self.cookies.clear();
    }
}

pub(super) fn worst_case(cookies: u32, cookie: u32, head: u32) -> u64 {
    // A stored Path came from one HTTP head; '=' and '; ' also occur in Cookie.
    u64::from(cookies) * (u64::try_from(size_of::<Cookie>()).expect("size fits") + u64::from(cookie) + u64::from(head))
}

#[cfg(test)]
mod tests {
    use super::{Jar, parse, under};
    use skein_lib::Wall;

    #[test]
    fn paths_match_at_a_segment_boundary_and_ignore_the_query() {
        for (request, path, expected) in [
            ("/api", "/api", true),
            ("/api?x=/", "/api", true),
            ("/api/x", "/api", true),
            ("/apian", "/api", false),
            ("/api", "/api/", false),
            ("/api/x", "/api/", true),
            ("/anything", "/", true),
        ] {
            assert_eq!(under(request.as_bytes(), path.as_bytes()), expected, "{request} under {path}");
        }
    }
    #[test]
    fn malformed_cookies_are_ignored() {
        for header in
            [b"no-pair".as_slice(), b"=x", b"bad name=x", b"n=bad value", b"n=\"unterminated", b"n=x; Max-Age=bad"]
        {
            assert!(parse(header, Wall::EPOCH, 32).is_none(), "{header:?}");
        }
    }
    #[test]
    fn attributes_are_case_insensitive_and_unrelated_attributes_are_ignored() {
        let cookie = parse(
            b"n=\"v\"; pAtH=/api; mAx-aGe=2; HttpOnly; Secure; Domain=elsewhere; Expires=bad",
            Wall::from_nanos(3),
            2,
        )
        .expect("cookie");
        assert_eq!(cookie.name.as_ref(), b"n");
        assert_eq!(cookie.value.as_ref(), b"v");
        assert_eq!(cookie.path.as_ref(), b"/api");
        assert_eq!(cookie.expires, Some(2_000_000_003));
    }
    #[test]
    fn the_jar_replaces_by_name_at_capacity_and_deletes_negative_max_age() {
        let mut jar = Jar::new(1, 2);
        jar.set(b"n=v; Path=/old", Wall::EPOCH);
        jar.set(b"n=w; Path=/new", Wall::EPOCH);
        assert_eq!(jar.header(b"/new", Wall::EPOCH).as_deref(), Some(b"n=w".as_slice()));
        assert!(jar.header(b"/old", Wall::EPOCH).is_none());
        jar.set(b"n=; Max-Age=-1", Wall::EPOCH);
        assert!(jar.header(b"/new", Wall::EPOCH).is_none());
    }
    #[test]
    fn cookie_length_is_checked_on_both_sides() {
        assert!(parse(b"n=v", Wall::EPOCH, 2).is_some());
        assert!(parse(b"n=vv", Wall::EPOCH, 2).is_none());
    }
    #[test]
    fn expiry_is_exclusive_and_session_cookies_have_none() {
        let mut jar = Jar::new(2, 4);
        jar.set(b"n=v; Max-Age=1", Wall::EPOCH);
        assert!(jar.header(b"/", Wall::from_nanos(999_999_999)).is_some());
        assert!(jar.header(b"/", Wall::from_nanos(1_000_000_000)).is_none());
        jar.set(b"s=v", Wall::EPOCH);
        assert!(jar.header(b"/", Wall::from_nanos(u64::MAX)).is_some());
    }
}
