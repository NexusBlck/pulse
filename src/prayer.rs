//! The Prayer times add-on: the five prayers (and sunrise) worked out on
//! the PC from where you are and the sun's position, no internet needed.
//! The calculation method and Asr's rule follow your country unless you
//! pick them. Where you are comes from Windows' location the first time
//! (or `prayer.lat` / `prayer.lon` in the config), named after the nearest
//! big city.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prayer {
    Fajr,
    Sunrise,
    Dhuhr,
    Asr,
    Maghrib,
    Isha,
}

impl Prayer {
    pub const ALL: [Prayer; 6] = [Prayer::Fajr, Prayer::Sunrise, Prayer::Dhuhr, Prayer::Asr, Prayer::Maghrib, Prayer::Isha];

    pub fn label(self) -> &'static str {
        match self {
            Prayer::Fajr => "Fajr",
            Prayer::Sunrise => "Sunrise",
            Prayer::Dhuhr => "Dhuhr",
            Prayer::Asr => "Asr",
            Prayer::Maghrib => "Maghrib",
            Prayer::Isha => "Isha",
        }
    }

    /// Sunrise is a time to know, not a prayer to be reminded of.
    pub fn is_prayer(self) -> bool {
        self != Prayer::Sunrise
    }
}

/// How a method sets Fajr and Isha: the sun's angle below the horizon, or
/// Isha a fixed time after Maghrib.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Isha {
    Angle(f64),
    Minutes(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Method {
    pub key: &'static str,
    pub name: &'static str,
    pub fajr: f64,
    pub isha: Isha,
    /// Maghrib: the sun this far below the horizon (0 = at sunset).
    pub maghrib: f64,
}

pub const METHODS: [Method; 11] = [
    Method { key: "mwl", name: "Muslim World League", fajr: 18.0, isha: Isha::Angle(17.0), maghrib: 0.0 },
    Method { key: "isna", name: "ISNA (North America)", fajr: 15.0, isha: Isha::Angle(15.0), maghrib: 0.0 },
    Method { key: "egypt", name: "Egyptian Authority", fajr: 19.5, isha: Isha::Angle(17.5), maghrib: 0.0 },
    Method { key: "makkah", name: "Umm al-Qura, Makkah", fajr: 18.5, isha: Isha::Minutes(90.0), maghrib: 0.0 },
    Method { key: "karachi", name: "University of Karachi", fajr: 18.0, isha: Isha::Angle(18.0), maghrib: 0.0 },
    Method { key: "tehran", name: "Institute of Geophysics, Tehran", fajr: 17.7, isha: Isha::Angle(14.0), maghrib: 4.5 },
    Method { key: "gulf", name: "Gulf region", fajr: 19.5, isha: Isha::Minutes(90.0), maghrib: 0.0 },
    Method { key: "kemenag", name: "Kemenag (Indonesia)", fajr: 20.0, isha: Isha::Angle(18.0), maghrib: 0.0 },
    Method { key: "diyanet", name: "Diyanet (Turkey)", fajr: 18.0, isha: Isha::Angle(17.0), maghrib: 0.0 },
    Method { key: "jakim", name: "JAKIM (Malaysia)", fajr: 20.0, isha: Isha::Angle(18.0), maghrib: 0.0 },
    Method { key: "singapore", name: "MUIS (Singapore)", fajr: 20.0, isha: Isha::Angle(18.0), maghrib: 0.0 },
];

pub fn method(key: &str) -> Method {
    METHODS.iter().copied().find(|m| m.key == key).unwrap_or(METHODS[0])
}

/// The method a country's mosques usually follow.
pub fn method_for(country: &str) -> &'static str {
    match country {
        "PK" | "IN" | "BD" | "AF" => "karachi",
        "ID" => "kemenag",
        "MY" | "BN" => "jakim",
        "SG" => "singapore",
        "SA" | "YE" => "makkah",
        "AE" | "KW" | "QA" | "BH" | "OM" => "gulf",
        "EG" | "SD" | "LY" | "DZ" | "MA" | "TN" | "JO" | "SY" | "LB" | "IQ" | "PS" => "egypt",
        "IR" => "tehran",
        "TR" | "AZ" => "diyanet",
        "US" | "CA" => "isna",
        _ => "mwl",
    }
}

/// Hanafi Asr (shadow twice the object) where it's the custom.
pub fn hanafi_for(country: &str) -> bool {
    matches!(country, "PK" | "IN" | "BD" | "AF" | "TR" | "AZ")
}

fn dsin(d: f64) -> f64 {
    (d * PI / 180.0).sin()
}
fn dcos(d: f64) -> f64 {
    (d * PI / 180.0).cos()
}
fn dtan(d: f64) -> f64 {
    (d * PI / 180.0).tan()
}
fn darcsin(x: f64) -> f64 {
    x.asin() * 180.0 / PI
}
fn darccos(x: f64) -> f64 {
    x.acos() * 180.0 / PI
}
fn darctan2(y: f64, x: f64) -> f64 {
    y.atan2(x) * 180.0 / PI
}
fn fix(a: f64, b: f64) -> f64 {
    a.rem_euclid(b)
}

fn julian(year: i32, month: i32, day: i32) -> f64 {
    let (y, m) = if month <= 2 { (year - 1, month + 12) } else { (year, month) };
    let a = (y as f64 / 100.0).floor();
    let b = 2.0 - a + (a / 4.0).floor();
    (365.25 * (y as f64 + 4716.0)).floor() + (30.6001 * (m as f64 + 1.0)).floor() + day as f64 + b - 1524.5
}

/// The sun's declination (degrees) and the equation of time (hours) on Julian day `jd`.
fn sun(jd: f64) -> (f64, f64) {
    let d = jd - 2451545.0;
    let g = fix(357.529 + 0.98560028 * d, 360.0);
    let q = fix(280.459 + 0.98564736 * d, 360.0);
    let l = fix(q + 1.915 * dsin(g) + 0.020 * dsin(2.0 * g), 360.0);
    let e = 23.439 - 0.00000036 * d;
    let ra = fix(darctan2(dcos(e) * dsin(l), dcos(l)) / 15.0, 24.0);
    let eqt = q / 15.0 - ra;
    let decl = darcsin(dsin(e) * dsin(l));
    (decl, eqt - 24.0 * (eqt / 24.0).round())
}

/// Times for one day, in hours after local midnight (`tz` hours from UTC).
pub struct Times {
    pub hours: [f64; 6],
}

impl Times {
    pub fn get(&self, p: Prayer) -> f64 {
        self.hours[p as usize]
    }
}

pub fn compute(year: i32, month: i32, day: i32, lat: f64, lon: f64, tz: f64, m: Method, hanafi: bool) -> Times {
    let jd = julian(year, month, day) - lon / (15.0 * 24.0);
    let noon_at = |t: f64| {
        let (_, eqt) = sun(jd + t);
        fix(12.0 - eqt, 24.0)
    };
    // When the sun is `angle` degrees below the horizon, before noon (`before`) or after.
    let angle_time = |angle: f64, t: f64, before: bool| {
        let (decl, _) = sun(jd + t);
        let noon = noon_at(t);
        let v = darccos((-dsin(angle) - dsin(decl) * dsin(lat)) / (dcos(decl) * dcos(lat))) / 15.0;
        if before {
            noon - v
        } else {
            noon + v
        }
    };
    let asr_time = |factor: f64, t: f64| {
        let (decl, _) = sun(jd + t);
        let angle = -(1.0 / (factor + dtan((lat - decl).abs()))).atan() * 180.0 / PI;
        angle_time(angle, t, false)
    };
    // Two passes: the second starts from the first's answers.
    let mut t = [5.0, 6.0, 12.0, 13.0, 18.0, 18.0, 18.0];
    for _ in 0..2 {
        let p = t.map(|h| h / 24.0);
        t = [
            angle_time(m.fajr, p[0], true),
            angle_time(0.833, p[1], true),
            noon_at(p[2]),
            asr_time(if hanafi { 2.0 } else { 1.0 }, p[3]),
            angle_time(0.833, p[4], false),
            angle_time(m.maghrib.max(0.833), p[5], false),
            match m.isha {
                Isha::Angle(a) => angle_time(a, p[6], false),
                Isha::Minutes(_) => 18.0,
            },
        ];
    }
    let shift = tz - lon / 15.0;
    let [mut fajr, sunrise, dhuhr, asr, sunset, mut maghrib, mut isha] = t.map(|h| h + shift);
    if m.maghrib <= 0.833 {
        maghrib = sunset;
    }
    if let Isha::Minutes(mins) = m.isha {
        isha = maghrib + mins / 60.0;
    }
    // Far north or south the sun may not get that low: use a share of the night.
    let night = 24.0 - (sunset - sunrise);
    if !fajr.is_finite() || sunrise - fajr > night * m.fajr / 60.0 {
        fajr = sunrise - night * m.fajr / 60.0;
    }
    if let Isha::Angle(a) = m.isha {
        if !isha.is_finite() || isha - sunset > night * a / 60.0 {
            isha = sunset + night * a / 60.0;
        }
    }
    Times { hours: [fajr, sunrise, dhuhr + 1.0 / 60.0, asr, maghrib, isha] }
}

/// The direction of the Kaaba from here, degrees clockwise from north.
pub fn qibla(lat: f64, lon: f64) -> f64 {
    let (klat, klon) = (21.4225, 39.8262);
    let y = dsin(klon - lon);
    let x = dcos(lat) * dtan(klat) - dsin(lat) * dcos(klon - lon);
    fix(darctan2(y, x), 360.0)
}

/// "N", "NE"... for a bearing.
pub fn compass(deg: f64) -> &'static str {
    const D: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    D[((deg + 22.5).rem_euclid(360.0) / 45.0) as usize % 8]
}

/// "15:42" for hours after midnight.
pub fn fmt(h: f64) -> String {
    let m = (h * 60.0).round() as i64;
    format!("{:02}:{:02}", m.rem_euclid(1440) / 60, m.rem_euclid(60))
}

/// Big cities, to name where you are: name, country, latitude, longitude.
pub const CITIES: &[(&str, &str, f64, f64)] = &[
    ("Makkah", "SA", 21.4225, 39.8262),
    ("Madinah", "SA", 24.4672, 39.6111),
    ("Riyadh", "SA", 24.7136, 46.6753),
    ("Jeddah", "SA", 21.4858, 39.1925),
    ("Dubai", "AE", 25.2048, 55.2708),
    ("Abu Dhabi", "AE", 24.4539, 54.3773),
    ("Doha", "QA", 25.2854, 51.5310),
    ("Kuwait City", "KW", 29.3759, 47.9774),
    ("Manama", "BH", 26.2285, 50.5860),
    ("Muscat", "OM", 23.5880, 58.3829),
    ("Karachi", "PK", 24.8607, 67.0011),
    ("Lahore", "PK", 31.5204, 74.3587),
    ("Islamabad", "PK", 33.6844, 73.0479),
    ("Rawalpindi", "PK", 33.5651, 73.0169),
    ("Faisalabad", "PK", 31.4504, 73.1350),
    ("Multan", "PK", 30.1575, 71.5249),
    ("Peshawar", "PK", 34.0151, 71.5249),
    ("Quetta", "PK", 30.1798, 66.9750),
    ("Hyderabad", "PK", 25.3960, 68.3578),
    ("Sialkot", "PK", 32.4945, 74.5229),
    ("Gujranwala", "PK", 32.1877, 74.1945),
    ("Delhi", "IN", 28.6139, 77.2090),
    ("Mumbai", "IN", 19.0760, 72.8777),
    ("Hyderabad", "IN", 17.3850, 78.4867),
    ("Bengaluru", "IN", 12.9716, 77.5946),
    ("Kolkata", "IN", 22.5726, 88.3639),
    ("Lucknow", "IN", 26.8467, 80.9462),
    ("Srinagar", "IN", 34.0837, 74.7973),
    ("Dhaka", "BD", 23.8103, 90.4125),
    ("Chittagong", "BD", 22.3569, 91.7832),
    ("Kabul", "AF", 34.5553, 69.2075),
    ("Tehran", "IR", 35.6892, 51.3890),
    ("Mashhad", "IR", 36.2605, 59.6168),
    ("Baghdad", "IQ", 33.3152, 44.3661),
    ("Istanbul", "TR", 41.0082, 28.9784),
    ("Ankara", "TR", 39.9334, 32.8597),
    ("Baku", "AZ", 40.4093, 49.8671),
    ("Cairo", "EG", 30.0444, 31.2357),
    ("Alexandria", "EG", 31.2001, 29.9187),
    ("Amman", "JO", 31.9454, 35.9284),
    ("Jerusalem", "PS", 31.7683, 35.2137),
    ("Damascus", "SY", 33.5138, 36.2765),
    ("Beirut", "LB", 33.8938, 35.5018),
    ("Casablanca", "MA", 33.5731, -7.5898),
    ("Algiers", "DZ", 36.7538, 3.0588),
    ("Tunis", "TN", 36.8065, 10.1815),
    ("Khartoum", "SD", 15.5007, 32.5599),
    ("Lagos", "NG", 6.5244, 3.3792),
    ("Kano", "NG", 12.0022, 8.5920),
    ("Nairobi", "KE", -1.2921, 36.8219),
    ("Johannesburg", "ZA", -26.2041, 28.0473),
    ("Cape Town", "ZA", -33.9249, 18.4241),
    ("Jakarta", "ID", -6.2088, 106.8456),
    ("Surabaya", "ID", -7.2575, 112.7521),
    ("Bandung", "ID", -6.9175, 107.6191),
    ("Medan", "ID", 3.5952, 98.6722),
    ("Makassar", "ID", -5.1477, 119.4327),
    ("Yogyakarta", "ID", -7.7956, 110.3695),
    ("Kuala Lumpur", "MY", 3.1390, 101.6869),
    ("Singapore", "SG", 1.3521, 103.8198),
    ("Bandar Seri Begawan", "BN", 4.9031, 114.9398),
    ("Bangkok", "TH", 13.7563, 100.5018),
    ("Manila", "PH", 14.5995, 120.9842),
    ("Beijing", "CN", 39.9042, 116.4074),
    ("Tokyo", "JP", 35.6762, 139.6503),
    ("Seoul", "KR", 37.5665, 126.9780),
    ("Sydney", "AU", -33.8688, 151.2093),
    ("Melbourne", "AU", -37.8136, 144.9631),
    ("London", "GB", 51.5074, -0.1278),
    ("Birmingham", "GB", 52.4862, -1.8904),
    ("Manchester", "GB", 53.4808, -2.2426),
    ("Paris", "FR", 48.8566, 2.3522),
    ("Berlin", "DE", 52.5200, 13.4050),
    ("Amsterdam", "NL", 52.3676, 4.9041),
    ("Brussels", "BE", 50.8503, 4.3517),
    ("Madrid", "ES", 40.4168, -3.7038),
    ("Rome", "IT", 41.9028, 12.4964),
    ("Stockholm", "SE", 59.3293, 18.0686),
    ("Oslo", "NO", 59.9139, 10.7522),
    ("Moscow", "RU", 55.7558, 37.6173),
    ("Kazan", "RU", 55.7963, 49.1088),
    ("Tashkent", "UZ", 41.2995, 69.2401),
    ("Almaty", "KZ", 43.2220, 76.8512),
    ("New York", "US", 40.7128, -74.0060),
    ("Chicago", "US", 41.8781, -87.6298),
    ("Houston", "US", 29.7604, -95.3698),
    ("Los Angeles", "US", 34.0522, -118.2437),
    ("Detroit", "US", 42.3314, -83.0458),
    ("Toronto", "CA", 43.6532, -79.3832),
    ("Montreal", "CA", 45.5017, -73.5673),
    ("Vancouver", "CA", 49.2827, -123.1207),
    ("Mexico City", "MX", 19.4326, -99.1332),
    ("São Paulo", "BR", -23.5505, -46.6333),
    ("Buenos Aires", "AR", -34.6037, -58.3816),
];

/// Distance in km between two points (haversine).
pub fn km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dlat, dlon) = (b.0 - a.0, b.1 - a.1);
    let h = dsin(dlat / 2.0).powi(2) + dcos(a.0) * dcos(b.0) * dsin(dlon / 2.0).powi(2);
    2.0 * 6371.0 * h.sqrt().asin()
}

/// The nearest big city: its name, country, and how far away it is.
pub fn nearest(lat: f64, lon: f64) -> (&'static str, &'static str, f64) {
    CITIES.iter().map(|c| (c.0, c.1, km((lat, lon), (c.2, c.3)))).min_by(|a, b| a.2.total_cmp(&b.2)).unwrap()
}

/// This PC's offset from UTC right now, in hours (daylight saving included).
pub fn utc_offset_h() -> f64 {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    use windows::Win32::System::Time::SystemTimeToFileTime;
    unsafe {
        let (l, u) = (GetLocalTime(), GetSystemTime());
        let to = |s: &SYSTEMTIME| {
            let mut f = FILETIME::default();
            let _ = SystemTimeToFileTime(s, &mut f);
            (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64
        };
        ((to(&l) - to(&u)) / 1e7 / 60.0 / 15.0).round() * 15.0 / 60.0
    }
}

/// Where the PC is, from Windows' location (on a worker thread: it can take seconds).
pub fn locate() -> Result<(f64, f64), String> {
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator, PositionAccuracy};
    let access = Geolocator::RequestAccessAsync().and_then(|o| o.get()).map_err(|e| e.message().to_string())?;
    if access != GeolocationAccessStatus::Allowed {
        return Err("Windows location is off for apps".into());
    }
    let g = Geolocator::new().map_err(|e| e.message().to_string())?;
    let _ = g.SetDesiredAccuracy(PositionAccuracy::Default);
    let pos = g.GetGeopositionAsync().and_then(|o| o.get()).map_err(|e| e.message().to_string())?;
    let p = pos.Coordinate().and_then(|c| c.Point()).and_then(|p| p.Position()).map_err(|e| e.message().to_string())?;
    Ok((p.Latitude, p.Longitude))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_come_in_order() {
        for (lat, lon, tz) in [(31.52, 74.36, 5.0), (21.42, 39.83, 3.0), (-6.21, 106.85, 7.0), (51.51, -0.13, 0.0)] {
            let t = compute(2026, 11, 10, lat, lon, tz, method("mwl"), false);
            let h = t.hours;
            assert!(h.windows(2).all(|w| w[0] < w[1]), "{lat},{lon}: {:?}", h.map(fmt));
            assert!(h.iter().all(|x| x.is_finite() && *x > 0.0 && *x < 24.0));
        }
    }

    #[test]
    fn noon_is_near_solar_noon() {
        // On the meridian at UTC, Dhuhr is noon give or take the equation of time (±17 min).
        let t = compute(2026, 3, 20, 0.0, 0.0, 0.0, method("mwl"), false);
        assert!((t.get(Prayer::Dhuhr) - 12.0).abs() < 0.35, "{}", fmt(t.get(Prayer::Dhuhr)));
        // An equinox on the equator: about 12 hours of daylight.
        let day = t.get(Prayer::Maghrib) - t.get(Prayer::Sunrise);
        assert!((day - 12.1).abs() < 0.2, "day {day}");
    }

    #[test]
    fn hanafi_asr_is_later() {
        let s = compute(2026, 11, 10, 31.52, 74.36, 5.0, method("karachi"), false);
        let h = compute(2026, 11, 10, 31.52, 74.36, 5.0, method("karachi"), true);
        assert!(h.get(Prayer::Asr) > s.get(Prayer::Asr) + 0.4);
    }

    #[test]
    fn lahore_in_november_is_about_right() {
        // Approximate Karachi-method times for Lahore around 10 Nov (a sanity
        // check within 10 minutes, not an official timetable): Fajr ≈ 05:05,
        // Sunrise ≈ 06:29, Dhuhr ≈ 11:55, Asr (Hanafi) ≈ 15:30, Maghrib ≈ 17:13, Isha ≈ 18:38.
        let t = compute(2026, 11, 10, 31.5204, 74.3587, 5.0, method("karachi"), true);
        let want = [5.0 + 5.0 / 60.0, 6.0 + 29.0 / 60.0, 11.0 + 55.0 / 60.0, 15.5, 17.0 + 13.0 / 60.0, 18.0 + 38.0 / 60.0];
        for (got, want) in t.hours.iter().zip(want) {
            assert!((got - want).abs() < 0.17, "got {} want {}", fmt(*got), fmt(want));
        }
    }

    #[test]
    fn umm_al_qura_isha_is_90_after_maghrib() {
        let t = compute(2026, 11, 10, 21.42, 39.83, 3.0, method("makkah"), false);
        assert!((t.get(Prayer::Isha) - t.get(Prayer::Maghrib) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn qibla_and_places() {
        // From London the Kaaba is about 119° (ESE).
        assert!((qibla(51.5074, -0.1278) - 119.0).abs() < 1.5);
        assert_eq!(compass(119.0), "SE");
        assert_eq!(nearest(31.5, 74.3).0, "Lahore");
        assert_eq!(method_for("PK"), "karachi");
    }
}
