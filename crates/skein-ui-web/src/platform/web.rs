use super::Facts;
use skein_lib::{Time, Wall};

pub struct Browser<R> {
    pub fetch: crate::fetch::web::Fetch<R>,
    hidden: crate::hidden::web::Hidden<R>,
}
impl<R: 'static> Browser<R> {
    #[must_use]
    pub fn new(fetch: crate::fetch::web::Fetch<R>) -> Self {
        Self { hidden: crate::hidden::web::Hidden::new(fetch.clone()), fetch }
    }
}
fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}
impl<R: 'static> super::Platform for Browser<R> {
    fn stream(&mut self, duty: crate::transport::StreamDuty) {
        self.hidden.carry(duty);
    }
    fn read_all(&self) -> Vec<crate::store::Record> {
        let Some(storage) = storage() else { return Vec::new() };
        (0..storage.length().unwrap_or(0))
            .filter_map(|index| {
                let key = storage.key(index).ok().flatten()?;
                let value = storage.get_item(&key).ok().flatten()?;
                Some(crate::store::Record { key: key.into_bytes().into(), value: value.into_bytes().into() })
            })
            .collect()
    }
    fn write(&mut self, key: &[u8], value: &[u8]) -> bool {
        let (Ok(key), Ok(value)) = (std::str::from_utf8(key), std::str::from_utf8(value)) else { return false };
        storage().is_some_and(|storage| storage.set_item(key, value).is_ok())
    }
    fn remove(&mut self, key: &[u8]) {
        if let (Some(storage), Ok(key)) = (storage(), std::str::from_utf8(key)) {
            let _removed = storage.remove_item(key);
        }
    }
    fn send(&mut self, exchange: &crate::fetch::Exchange) {
        self.fetch.exchange(exchange);
    }
    fn abandon(&mut self, exchange: u32) {
        self.fetch.abandon(exchange);
    }
    fn read_on(&mut self) {
        self.fetch.read_on();
    }
    fn navigate(&mut self, to: &[u8]) {
        if !crate::navigation::allowed(to, cfg!(feature = "loopback-navigation")) {
            return;
        }
        let to = std::str::from_utf8(to).expect("visible ASCII navigation");
        web_sys::window().expect("browser window").location().set_href(to).expect("navigation address");
    }
}

// Browser clocks have millisecond precision and finite nonnegative values.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "browser dates and clocks are nonnegative milliseconds; saturating cast bounds the result"
)]
fn nanos(millis: f64) -> u64 {
    (millis.max(0.0) * 1_000_000.0) as u64
}
#[must_use]
pub fn clocks() -> (Time, Wall) {
    let window = web_sys::window().expect("browser window");
    let now = window.performance().expect("monotonic browser clock").now();
    (Time::from_nanos(nanos(now)), Wall::from_nanos(nanos(js_sys::Date::now())))
}
#[must_use]
pub fn facts() -> Facts {
    let window = web_sys::window().expect("browser window");
    let navigator = window.navigator();
    let mut random = [0_u8; 8];
    window
        .crypto()
        .expect("browser cryptographic generator")
        .get_random_values_with_u8_array(&mut random)
        .expect("seed from cryptographic generator");
    let languages =
        navigator.languages().iter().filter_map(|language| language.as_string()).collect::<Vec<_>>().join(",");
    let (now, wall) = clocks();
    Facts {
        now,
        wall,
        seed: u64::from_le_bytes(random),
        offset: offset(),
        languages: languages.into_bytes().into(),
        primary: super::primary(navigator.platform().ok().as_deref()),
    }
}
#[expect(clippy::cast_possible_truncation, reason = "the browser timezone offset is integral minutes within i32")]
fn offset() -> i32 {
    -(js_sys::Date::new_0().get_timezone_offset() as i32)
}
