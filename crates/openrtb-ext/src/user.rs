//! Go: `openrtb_ext/user.go`: Google's Additional Consent string (the types are generated).

/// Go: `ParseConsentedProvidersString(cps)`: the elements of a Google Additional Consent string.
///
/// For instance, the string `"1~1.35.41.101"` gives `[1, 35, 41, 101]`.
///
/// The string must have exactly one `~`; the elements after it that `strconv.Atoi` reads are
/// kept, the others dropped. `None` is Go's nil: no `~`, more than one, or no element read.
/// `str::parse` reads what `Atoi` reads: an optional sign and decimal digits, failing beyond
/// `int`'s 64 bits.
#[must_use]
pub fn parse_consented_providers_string(cps: &str) -> Option<Vec<isize>> {
    // Go: openrtb_ext/user.go:40
    let (_, providers) = cps.split_once('~')?;
    if providers.contains('~') {
        return None;
    }
    let mut consented_providers: Option<Vec<isize>> = None;
    for provider_str in providers.split('.') {
        if let Some(provider) = provider_str
            .parse::<i64>()
            .ok()
            .and_then(|n| isize::try_from(n).ok())
        {
            consented_providers
                .get_or_insert_with(Vec::new)
                .push(provider);
        }
    }
    consented_providers
}
