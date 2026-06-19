/// Support for the "current locale."
pub use fish_printf::locale::{C_LOCALE, Locale};
use std::{ffi::CStr, sync::Mutex};

/// Lock guarding libc `setlocale()` or `localeconv()` calls to avoid races.
pub(crate) static LOCALE_LOCK: Mutex<()> = Mutex::new(());

/// # Safety
/// Call this either before starting any locale-using thread, or while holding a lock on the
/// above mutex.
pub unsafe fn set_libc_locales(log_ok: bool) -> bool {
    let mut ok = true;
    let mut set = |category_name, category, candidates: &[std::ffi::CString]| {
        // Try each candidate spelling in turn; the first one the CRT accepts
        // wins. On glibc the single candidate is the empty string, which means
        // "use the POSIX environment".
        let mut locale_string = None;
        let mut chosen: Option<&std::ffi::CString> = None;
        for cand in candidates {
            if let Some(loc) = setlocale(category, Some(cand)) {
                locale_string = Some(loc);
                chosen = Some(cand);
                break;
            }
        }
        if log_ok {
            crate::flog::flog!(env_locale, {
                let source = match chosen {
                    Some(c) if c.as_bytes().is_empty() => "from environment".to_owned(),
                    Some(c) => format!("to '{}'", c.to_string_lossy()),
                    None => "from environment".to_owned(),
                };
                match locale_string {
                    Some(locale_string) => {
                        format!(
                            "Set {category_name} {source}: {}",
                            locale_string.to_string_lossy()
                        )
                    }
                    None => {
                        format!("Failed to set {category_name} {source}")
                    }
                }
            });
        }
        ok &= locale_string.is_some();
    };
    // For strerror(3p) and strsignal(3p)
    set("LC_MESSAGES", libc::LC_MESSAGES, &locale_candidates("LC_MESSAGES"));
    // For builtin printf
    set("LC_NUMERIC", libc::LC_NUMERIC, &locale_candidates("LC_NUMERIC"));
    // For "history --show-time"
    set("LC_TIME", libc::LC_TIME, &locale_candidates("LC_TIME"));
    ok
}

/// Resolve the ordered list of locale strings to try with libc `setlocale` for
/// `category`.
///
/// On glibc, `setlocale(cat, "")` consults the POSIX environment variables
/// (`LC_ALL`, then the specific category, then `LANG`). The Windows CRT does
/// *not*: `setlocale(cat, "")` ignores those variables and instead selects the
/// user's default system locale, which on a non-US install yields a comma
/// decimal separator and breaks `printf`/`strerror` output. Worse, the mingw
/// msvcrt does not accept BCP-47 (`de-DE`) or POSIX (`de_DE`) names at all — it
/// only understands Windows names like `German_Germany` (or the language alone,
/// `German`). So on Windows we emulate the POSIX precedence ourselves and
/// produce a list of candidate spellings, most specific first. The
/// empty/`C`/`POSIX` cases (which the test harness sets via `LANG=C`) map to the
/// CRT's `"C"` locale.
#[cfg(windows)]
fn locale_candidates(category: &str) -> Vec<std::ffi::CString> {
    use std::ffi::CString;
    let lookup = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let value = lookup("LC_ALL")
        .or_else(|| lookup(category))
        .or_else(|| lookup("LANG"));
    let c_locale = || CString::new("C").unwrap();
    let value = match value {
        // No POSIX locale variables set, or an explicit C/POSIX locale: use the
        // CRT "C" locale, which guarantees '.' as the decimal separator.
        None => return vec![c_locale()],
        Some(v) if v == "C" || v == "POSIX" || v.starts_with("C.") || v.starts_with("POSIX.") => {
            return vec![c_locale()];
        }
        Some(v) => v,
    };

    // Strip any ".codeset" / "@modifier" suffix, e.g. "de_DE.UTF-8" -> "de_DE".
    let base = value.split(['.', '@']).next().unwrap_or(&value);
    let mut parts = base.split(['_', '-']);
    let lang = parts.next().unwrap_or("").to_ascii_lowercase();
    let country = parts.next().unwrap_or("").to_ascii_uppercase();

    let mut candidates: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if !s.is_empty() && !candidates.contains(&s) {
            candidates.push(s);
        }
    };

    // Most specific: the Windows "Language_Country" spelling.
    if let Some(win_lang) = windows_language(&lang) {
        if let Some(win_country) = windows_country(&country) {
            push(format!("{win_lang}_{win_country}"));
        }
        // Language alone (the CRT picks that language's default country).
        push(win_lang.to_owned());
    }
    // BCP-47 "ll-CC" / "ll" — accepted by the Universal CRT toolchains.
    if !country.is_empty() {
        push(format!("{lang}-{country}"));
    }
    push(lang.clone());
    // Finally the raw POSIX base and the original value, just in case.
    push(base.replace('_', "-"));
    push(base.to_owned());
    push(value.clone());

    candidates
        .into_iter()
        .filter_map(|s| CString::new(s).ok())
        .collect()
}

#[cfg(not(windows))]
fn locale_candidates(_category: &str) -> Vec<std::ffi::CString> {
    // glibc's setlocale(cat, "") already honours the POSIX environment.
    vec![std::ffi::CString::default()]
}

/// Map an ISO 639-1 language code to the Windows/msvcrt English language name.
#[cfg(windows)]
fn windows_language(code: &str) -> Option<&'static str> {
    Some(match code {
        "af" => "Afrikaans",
        "sq" => "Albanian",
        "ar" => "Arabic",
        "hy" => "Armenian",
        "eu" => "Basque",
        "be" => "Belarusian",
        "bg" => "Bulgarian",
        "ca" => "Catalan",
        "zh" => "Chinese",
        "hr" => "Croatian",
        "cs" => "Czech",
        "da" => "Danish",
        "nl" => "Dutch",
        "en" => "English",
        "et" => "Estonian",
        "fo" => "Faroese",
        "fa" => "Persian",
        "fi" => "Finnish",
        "fr" => "French",
        "gl" => "Galician",
        "ka" => "Georgian",
        "de" => "German",
        "el" => "Greek",
        "he" => "Hebrew",
        "hi" => "Hindi",
        "hu" => "Hungarian",
        "is" => "Icelandic",
        "id" => "Indonesian",
        "it" => "Italian",
        "ja" => "Japanese",
        "kk" => "Kazakh",
        "ko" => "Korean",
        "lv" => "Latvian",
        "lt" => "Lithuanian",
        "mk" => "Macedonian",
        "ms" => "Malay",
        "mt" => "Maltese",
        "nb" | "nn" | "no" => "Norwegian",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ro" => "Romanian",
        "ru" => "Russian",
        "sr" => "Serbian",
        "sk" => "Slovak",
        "sl" => "Slovenian",
        "es" => "Spanish",
        "sv" => "Swedish",
        "th" => "Thai",
        "tr" => "Turkish",
        "uk" => "Ukrainian",
        "vi" => "Vietnamese",
        _ => return None,
    })
}

/// Map an ISO 3166-1 alpha-2 country code to the Windows/msvcrt English country
/// name.
#[cfg(windows)]
fn windows_country(code: &str) -> Option<&'static str> {
    Some(match code {
        "AR" => "Argentina",
        "AT" => "Austria",
        "AU" => "Australia",
        "BE" => "Belgium",
        "BG" => "Bulgaria",
        "BR" => "Brazil",
        "BY" => "Belarus",
        "CA" => "Canada",
        "CH" => "Switzerland",
        "CL" => "Chile",
        "CN" => "China",
        "CO" => "Colombia",
        "CZ" => "Czech Republic",
        "DE" => "Germany",
        "DK" => "Denmark",
        "EE" => "Estonia",
        "ES" => "Spain",
        "FI" => "Finland",
        "FR" => "France",
        "GB" => "United Kingdom",
        "GR" => "Greece",
        "HR" => "Croatia",
        "HU" => "Hungary",
        "ID" => "Indonesia",
        "IE" => "Ireland",
        "IL" => "Israel",
        "IN" => "India",
        "IS" => "Iceland",
        "IT" => "Italy",
        "JP" => "Japan",
        "KR" => "Korea",
        "KZ" => "Kazakhstan",
        "LT" => "Lithuania",
        "LV" => "Latvia",
        "MX" => "Mexico",
        "MY" => "Malaysia",
        "NL" => "Netherlands",
        "NO" => "Norway",
        "NZ" => "New Zealand",
        "PL" => "Poland",
        "PT" => "Portugal",
        "RO" => "Romania",
        "RU" => "Russia",
        "SE" => "Sweden",
        "SI" => "Slovenia",
        "SK" => "Slovakia",
        "TH" => "Thailand",
        "TR" => "Turkey",
        "TW" => "Taiwan",
        "UA" => "Ukraine",
        "US" => "United States",
        "VN" => "Vietnam",
        "ZA" => "South Africa",
        _ => return None,
    })
}

fn setlocale(category: libc::c_int, locale: Option<&CStr>) -> Option<&'static CStr> {
    let loc_ptr = {
        let locale = locale.map_or(std::ptr::null(), |loc| loc.as_ptr());
        #[allow(clippy::disallowed_methods)]
        unsafe {
            libc::setlocale(category, locale)
        }
    };
    (!loc_ptr.is_null()).then(||
        // Safety: setlocale did not return a null-pointer, so it is a valid pointer
        unsafe{CStr::from_ptr(loc_ptr)})
}

/// It's CHAR_MAX.
const CHAR_MAX: libc::c_char = libc::c_char::MAX;

/// Return the first character of a C string, or None if null, empty, has a length more than 1, or negative.
unsafe fn first_char(s: *const libc::c_char) -> Option<char> {
    unsafe {
        #[allow(unused_comparisons, clippy::absurd_extreme_comparisons)]
        if !s.is_null() && *s > 0 && *s <= 127 && *s.add(1) == 0 {
            #[allow(clippy::unnecessary_cast)]
            Some((*s as u8) as char)
        } else {
            None
        }
    }
}

/// Convert a libc lconv to a Locale.
unsafe fn lconv_to_locale(lconv: &libc::lconv) -> Locale {
    let decimal_point = unsafe { first_char(lconv.decimal_point).unwrap_or('.') };
    let thousands_sep = unsafe { first_char(lconv.thousands_sep) };
    let empty = &[0 as libc::c_char];

    // Up to 4 groups.
    // group_cursor is terminated by either a 0 or CHAR_MAX.
    let mut group_cursor = lconv.grouping.cast_const();
    if group_cursor.is_null() {
        group_cursor = empty.as_ptr();
    }

    let mut grouping = [0; 4];
    let mut last_group: u8 = 0;
    let mut group_repeat = false;
    for group in grouping.iter_mut() {
        let gc = unsafe { *group_cursor };
        if gc == 0 {
            // Preserve last_group, do not advance cursor.
            group_repeat = true;
        } else if gc == CHAR_MAX {
            // Remaining groups are 0, do not advance cursor.
            last_group = 0;
            group_repeat = false;
        } else {
            // Record last group, advance cursor.
            last_group = gc as u8;
            group_cursor = unsafe { group_cursor.add(1) };
        }
        *group = last_group;
    }
    Locale {
        decimal_point,
        thousands_sep,
        grouping,
        group_repeat,
    }
}

/// Read the numeric locale, or None on any failure.
#[cfg(have_localeconv_l)]
unsafe fn read_locale() -> Option<Locale> {
    unsafe extern "C" {
        unsafe fn localeconv_l(loc: libc::locale_t) -> *const libc::lconv;
    }

    // We create a new locale (pass 0 locale_t base)
    // and pass no "locale", so everything else is taken from the environment.
    // This is fine because we're only using this for numbers.
    let loc = unsafe { libc::newlocale(libc::LC_NUMERIC_MASK, c"".as_ptr(), 0 as libc::locale_t) };
    if loc.is_null() {
        return None;
    }

    let lconv = unsafe { localeconv_l(loc) };
    let result = if lconv.is_null() {
        None
    } else {
        Some(unsafe { lconv_to_locale(&*lconv) })
    };

    unsafe { libc::freelocale(loc) };
    result
}

#[cfg(not(have_localeconv_l))]
unsafe fn read_locale() -> Option<Locale> {
    // Bleh, we have to go through localeconv, which races with setlocale.
    // TODO: There has to be a better way to do this.
    let _guard = LOCALE_LOCK.lock().unwrap();
    let lconv = unsafe { libc::localeconv() };
    (!lconv.is_null()).then(|| unsafe { lconv_to_locale(&*lconv) })
}

// Current numeric locale.
static NUMERIC_LOCALE: Mutex<Option<Locale>> = Mutex::new(None);

pub fn get_numeric_locale() -> Locale {
    let mut locale = NUMERIC_LOCALE.lock().unwrap();
    if locale.is_none() {
        let new_locale = (unsafe { read_locale() }).unwrap_or(C_LOCALE);
        *locale = Some(new_locale);
    }
    locale.unwrap()
}

/// Invalidate the cached numeric locale.
pub fn invalidate_numeric_locale() {
    *NUMERIC_LOCALE.lock().unwrap() = None;
}
