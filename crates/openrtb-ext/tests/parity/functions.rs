//! Replays `functions.tsv`: what the package's functions and methods return for the inputs of
//! `testdata/go/functions.go`, one call per row (`function label arg... -> result...`).

use core::{
    fmt::Display,
    str::{FromStr, from_utf8},
};
use std::collections::BTreeMap;

use pbs_errortypes::GoError;
use pbs_gojson::{GoEncode, JsoniterDecode, RawJson, jsonutil};
use pbs_openrtb::openrtb2;
use pbs_openrtb_ext::openrtb_ext::{
    self, BidderName, ExtAdapterAlternateBidderCodes, ExtAlternateBidderCodes, ExtMultiBid,
    ExtRegsDSA, ExtRequestPrebid, PriceFloorData, PriceFloorEnforcement, PriceFloorModelGroup,
    PriceFloorRules, PriceGranularity, TargetingKey,
};

use super::support::{TestResult, ensure_eq, golden_rows, report, show};

/// Go's nil pointer, map or slice argument.
const NIL: &[u8] = b"nil";

/// The results of one call, as the generator writes them.
type Results = Vec<Vec<u8>>;

/// The text of a string argument. Go strings are bytes; an argument with invalid UTF-8 becomes
/// a `&str` with U+FFFD (the documented `String` gap).
fn text(arg: &[u8]) -> String {
    String::from_utf8_lossy(arg).into_owned()
}

/// A JSON argument, decoded with `jsonutil.Unmarshal` as the generator decodes it.
fn decode<T: Default + JsoniterDecode>(doc: &[u8]) -> TestResult<T> {
    let mut v = T::default();
    jsonutil::unmarshal(doc, &mut v)
        .map_err(|e| format!("argument {}: {}", show(doc), e.message.to_string_lossy()))?;
    Ok(v)
}

/// A JSON argument that may be [`NIL`].
fn decode_opt<T: Default + JsoniterDecode>(doc: &[u8]) -> TestResult<Option<T>> {
    if doc == NIL {
        Ok(None)
    } else {
        decode(doc).map(Some)
    }
}

/// `jsonutil.Marshal`'s encoding of `v`, or `!` and its error.
fn marshal<T: GoEncode + ?Sized>(v: &T) -> Vec<u8> {
    match jsonutil::marshal(v) {
        Ok(bytes) => bytes,
        Err(e) => {
            let mut out = b"!".to_vec();
            out.extend(e.message.into_bytes());
            out
        }
    }
}

/// An error's Go dynamic type and text.
fn error_fields(err: &GoError) -> [Vec<u8>; 2] {
    [
        err.kind().go_type().as_bytes().to_vec(),
        err.message().as_bytes().to_vec(),
    ]
}

/// A result followed by an error's Go dynamic type and text.
fn failed(first: Vec<u8>, err: &GoError) -> Results {
    let [ty, msg] = error_fields(err);
    vec![first, ty, msg]
}

/// A result followed by a nil error (empty type and text).
fn succeeded(first: Vec<u8>) -> Results {
    vec![first, Vec::new(), Vec::new()]
}

fn boolean(b: bool) -> Vec<u8> {
    if b {
        b"true".to_vec()
    } else {
        b"false".to_vec()
    }
}

fn bytes<S: AsRef<str>>(s: S) -> Vec<u8> {
    s.as_ref().as_bytes().to_vec()
}

/// The strings joined with `,`.
fn joined<I: IntoIterator<Item = String>>(items: I) -> Vec<u8> {
    bytes(items.into_iter().collect::<Vec<_>>().join(","))
}

/// One row's call: the Go function's name and the generator's arguments.
struct Call<'a> {
    name: &'a str,
    args: &'a [Vec<u8>],
}

impl Call<'_> {
    /// Argument `i`.
    fn arg(&self, i: usize) -> TestResult<&[u8]> {
        self.args
            .get(i)
            .map(Vec::as_slice)
            .ok_or_else(|| format!("{}: no argument {i}", self.name))
    }

    /// Argument `i`, a string.
    fn text(&self, i: usize) -> TestResult<String> {
        self.arg(i).map(text)
    }

    /// Argument `i`, an integer.
    fn int<T: FromStr<Err: Display>>(&self, i: usize) -> TestResult<T> {
        let arg = self.arg(i)?;
        text(arg)
            .parse()
            .map_err(|e| format!("{}: bad integer argument {}: {e}", self.name, show(arg)))
    }
}

/// The ports of `bid.go`, `request.go` and `multibid.go`, or `None` for another function.
fn bid_and_request(c: &Call<'_>) -> TestResult<Option<Results>> {
    Ok(Some(match c.name {
        "ParseBidType" => openrtb_ext::parse_bid_type(&c.text(0)?).map_or_else(
            |e| failed(Vec::new(), &e),
            |bt| succeeded(bytes(bt.as_str())),
        ),
        "BidTypes" => vec![joined(
            openrtb_ext::bid_types()
                .iter()
                .map(|bt| bt.as_str().to_owned()),
        )],
        "TargetingKey.BidderKey" => {
            let key = TargetingKey::from(c.text(0)?);
            let bidder = BidderName::from(c.text(2)?);
            vec![bytes(key.bidder_key(&c.text(1)?, &bidder, c.int(3)?))]
        }
        "TargetingKey.TruncateKey" => {
            let key = TargetingKey::from(c.text(0)?);
            vec![bytes(key.truncate_key(&c.text(1)?, c.int(2)?))]
        }
        "NewPriceGranularityDefault" => {
            vec![marshal(&openrtb_ext::new_price_granularity_default())]
        }
        "NewPriceGranularityFromLegacyID" => {
            openrtb_ext::new_price_granularity_from_legacy_id(&c.text(0)?).map_or_else(
                || vec![marshal(&PriceGranularity::default()), boolean(false)],
                |pg| vec![marshal(&pg), boolean(true)],
            )
        }
        "ExtMultiBid.String" => vec![bytes(decode::<ExtMultiBid>(c.arg(0)?)?.string())],
        "ValidateAndBuildExtMultiBid" => {
            let mut prebid = decode_opt::<ExtRequestPrebid>(c.arg(0)?)?;
            let (validated, errs) = openrtb_ext::validate_and_build_ext_multi_bid(prebid.as_mut());
            let after = prebid
                .as_ref()
                .map_or_else(|| NIL.to_vec(), |p| marshal(&p.multi_bid));
            let mut out = vec![marshal(&validated), after];
            for e in &errs {
                out.extend(error_fields(e));
            }
            out
        }
        "ExtRequestPrebid.Clone" => {
            let prebid = decode_opt::<ExtRequestPrebid>(c.arg(0)?)?;
            vec![marshal(&prebid.as_ref().map(ExtRequestPrebid::clone_go))]
        }
        _ => return Ok(None),
    }))
}

/// The ports of the other extension files, or `None` for another function.
fn extensions(c: &Call<'_>) -> TestResult<Option<Results>> {
    Ok(Some(match c.name {
        "ReadDealTiersFromImp" => {
            let doc = c.arg(0)?;
            let ext = if doc == NIL {
                RawJson::nil()
            } else {
                RawJson::from_slice(doc)
            };
            let imp = openrtb2::Imp {
                ext,
                ..openrtb2::Imp::default()
            };
            openrtb_ext::read_deal_tiers_from_imp(&imp).map_or_else(
                |e| failed(b"null".to_vec(), &e),
                |tiers| succeeded(marshal(&tiers)),
            )
        }
        "ParseDeviceExtATTS" => {
            let doc = c.arg(0)?;
            let ext = if doc == NIL { &[][..] } else { doc };
            match openrtb_ext::parse_device_ext_atts(ext) {
                Ok(Some(status)) => succeeded(bytes(status.0.to_string())),
                Ok(None) => succeeded(NIL.to_vec()),
                Err(e) => failed(NIL.to_vec(), &e),
            }
        }
        "IsKnownIOSAppTrackingStatus" => {
            vec![boolean(openrtb_ext::is_known_ios_app_tracking_status(
                c.int(0)?,
            ))]
        }
        "ParseConsentedProvidersString" => vec![marshal(
            &openrtb_ext::parse_consented_providers_string(&c.text(0)?),
        )],
        "ExtAlternateBidderCodes.IsValidBidderCode" => {
            let codes = decode::<ExtAlternateBidderCodes>(c.arg(0)?)?;
            codes
                .is_valid_bidder_code(&c.text(1)?, &c.text(2)?)
                .map_or_else(
                    |e| failed(boolean(false), &e),
                    |()| succeeded(boolean(true)),
                )
        }
        "ExtAlternateBidderCodes.IsBidderInAlternateBidderCodes" => {
            let codes = decode_opt::<ExtAlternateBidderCodes>(c.arg(0)?)?;
            let bidder = c.text(1)?;
            codes
                .as_ref()
                .and_then(|codes| codes.is_bidder_in_alternate_bidder_codes(&bidder))
                .map_or_else(
                    || {
                        vec![
                            marshal(&ExtAdapterAlternateBidderCodes::default()),
                            boolean(false),
                        ]
                    },
                    |cfg| vec![marshal(cfg), boolean(true)],
                )
        }
        "ExtRegsDSA.Clone" => vec![marshal(&decode_opt::<ExtRegsDSA>(c.arg(0)?)?)],
        "GetImpIDs" => {
            let imps = decode::<Option<Vec<openrtb2::Imp>>>(c.arg(0)?)?;
            vec![marshal(&openrtb_ext::get_imp_ids(
                imps.as_deref().unwrap_or_default(),
            ))]
        }
        _ => return Ok(None),
    }))
}

/// The ports of `floors.go`, or `None` for another function.
fn floors(c: &Call<'_>) -> TestResult<Option<Results>> {
    Ok(Some(match c.name {
        "PriceFloorRules.Get" => {
            let rules = decode_opt::<PriceFloorRules>(c.arg(0)?)?;
            let r = rules.as_ref();
            vec![
                boolean(PriceFloorRules::get_enforce_pbs(r)),
                boolean(PriceFloorRules::get_floors_skipped_flag(r)),
                bytes(PriceFloorRules::get_enforce_rate(r).to_string()),
                boolean(PriceFloorRules::get_enforce_deals_flag(r)),
                boolean(PriceFloorRules::get_enabled(r)),
            ]
        }
        "PriceFloorRules.DeepCopy" => {
            let rules = decode_opt::<PriceFloorRules>(c.arg(0)?)?;
            vec![marshal(&rules.as_ref().map(PriceFloorRules::deep_copy))]
        }
        "PriceFloorData.DeepCopy" => {
            let data = decode_opt::<PriceFloorData>(c.arg(0)?)?;
            vec![marshal(&data.as_ref().map(PriceFloorData::deep_copy))]
        }
        "PriceFloorModelGroup.Copy" => {
            vec![marshal(&decode::<PriceFloorModelGroup>(c.arg(0)?)?.copy())]
        }
        "PriceFloorEnforcement.DeepCopy" => {
            let enforcement = decode_opt::<PriceFloorEnforcement>(c.arg(0)?)?;
            vec![marshal(
                &enforcement.as_ref().map(PriceFloorEnforcement::deep_copy),
            )]
        }
        _ => return Ok(None),
    }))
}

/// The ports of `bidders.go`, or `None` for another function. Go's maps and sets are listed
/// sorted, as the generator lists them.
fn bidders(c: &Call<'_>) -> TestResult<Option<Results>> {
    Ok(Some(match c.name {
        "IsBidderNameReserved" => vec![boolean(openrtb_ext::is_bidder_name_reserved(&c.text(0)?))],
        "IsPotentialBidder" => vec![boolean(openrtb_ext::is_potential_bidder(&c.text(0)?))],
        "NormalizeBidderName" => openrtb_ext::normalize_bidder_name(&c.text(0)?).map_or_else(
            || vec![Vec::new(), boolean(false)],
            |n| vec![bytes(n.as_str()), boolean(true)],
        ),
        "NormalizeBidderNameOrUnchanged" => vec![bytes(
            openrtb_ext::normalize_bidder_name_or_unchanged(&c.text(0)?).as_str(),
        )],
        "CoreBidderNames" => vec![joined(
            openrtb_ext::core_bidder_names()
                .iter()
                .map(|n| n.as_str().to_owned()),
        )],
        "BuildBidderStringSlice" => vec![joined(openrtb_ext::build_bidder_string_slice())],
        "BuildBidderMap" => {
            let mut entries: Vec<String> = openrtb_ext::build_bidder_map()
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            entries.sort();
            vec![joined(entries)]
        }
        "BuildBidderNameHashSet" => {
            let mut names: Vec<String> = openrtb_ext::build_bidder_name_hash_set()
                .iter()
                .map(|name| name.as_str().to_owned())
                .collect();
            names.sort();
            vec![joined(names)]
        }
        "GetAliasBidderToParent" => vec![marshal(&openrtb_ext::get_alias_bidder_to_parent())],
        _ => return Ok(None),
    }))
}

/// Calls the Rust port of the row's Go function with the generator's arguments.
fn call(c: &Call<'_>) -> TestResult<Results> {
    for port in [bid_and_request, extensions, floors, bidders] {
        if let Some(results) = port(c)? {
            return Ok(results);
        }
    }
    Err(format!("no Rust port for Go function {}", c.name))
}

/// What the port does where Go panics, checked for one call. Go panics on a negative
/// `maxLength` in `TargetingKey.BidderKey` (`s[:maxLength]`) and on a nil element of
/// `multibid` or `schains` in `ValidateAndBuildExtMultiBid` and `ExtRequestPrebid.Clone` (a nil
/// pointer dereference); the port returns the whole key, validates a nil `multibid` element as
/// an empty one (without changing the request), and clones nil elements as nil.
fn documented_panic(c: &Call<'_>, rust: &Results) -> TestResult {
    let want: Results = match c.name {
        "TargetingKey.BidderKey" => {
            let mut key = c.arg(1)?.to_vec();
            key.extend_from_slice(c.arg(0)?);
            key.push(b'_');
            key.extend_from_slice(c.arg(2)?);
            vec![key]
        }
        "ValidateAndBuildExtMultiBid" => {
            let empty = ExtMultiBid::default().string();
            let error = format!("maxBids not defined for {empty}");
            let doc = c.arg(0)?;
            let validated: &[u8] = if doc.starts_with(br#"{"multibid":[null]"#) {
                b"null"
            } else {
                br#"[{"bidder":"a","maxbids":2}]"#
            };
            let prebid = decode::<ExtRequestPrebid>(doc)?;
            vec![
                validated.to_vec(),
                marshal(&prebid.multi_bid),
                b"*errors.errorString".to_vec(),
                bytes(error),
            ]
        }
        "ExtRequestPrebid.Clone" => vec![marshal(&decode::<ExtRequestPrebid>(c.arg(0)?)?)],
        _ => {
            return Err(format!(
                "{}: Go panics where the port has no documented result",
                c.name
            ));
        }
    };
    ensure_eq(
        rust,
        &want,
        &format!("{} {}: where Go panics", c.name, shown(c.args)),
    )
}

/// The fields, shown.
fn shown(fields: &[Vec<u8>]) -> String {
    format!("{:?}", fields.iter().map(|f| show(f)).collect::<Vec<_>>())
}

/// Calls where Go panics: `TargetingKey.BidderKey` with a negative `maxLength` (2 negative
/// lengths for each of the 36 key, prefix and bidder combinations), and the two
/// `ValidateAndBuildExtMultiBid` and two `ExtRequestPrebid.Clone` calls with nil elements.
const GO_PANICS: usize = 76;

/// The calls, by function, whose results differ from Go's only by the documented `String` gap: a
/// Go string holds any bytes, a Rust `String` holds U+FFFD for invalid UTF-8.
/// `ParseBidType("x\xff")`'s error text; the keys that `TargetingKey.BidderKey` and
/// `TruncateKey` cut inside a two-byte character (`é`, `ü`, `ï`), which Go leaves as a lone byte;
/// and the error of `ReadDealTiersFromImp` whose jsoniter text ends in the byte `0xff` (jsoniter
/// names a missing character so; a `GoError`'s text is a `String`).
const STRING_GAPS: [(&str, usize); 4] = [
    ("ParseBidType", 1),
    ("ReadDealTiersFromImp", 1),
    ("TargetingKey.BidderKey", 17),
    ("TargetingKey.TruncateKey", 2),
];

/// Replays the calls of a golden file in the format of `functions.tsv`: every call must return
/// Go's results, except the `go_panics` calls where Go panics (each checked against the port's
/// documented behaviour) and the `string_gaps` calls, by function, that differ only by the
/// documented `String` gap. At least `min_calls` calls must be replayed.
fn replay_calls(
    file: &str,
    min_calls: usize,
    go_panics: usize,
    string_gap_calls: &[(&str, usize)],
) -> TestResult {
    let mut failures = Vec::new();
    let mut calls = 0;
    let mut panics = 0;
    let mut string_gaps: BTreeMap<String, usize> = BTreeMap::new();
    for row in golden_rows(file)? {
        if row.first().is_some_and(|f| f == b"#go") {
            continue;
        }
        let split = row
            .iter()
            .position(|f| f == b"->")
            .ok_or_else(|| format!("a {file} row without \"->\""))?;
        let (head, results) = row.split_at(split);
        let results = results.get(1..).unwrap_or_default();
        let [name, _label, args @ ..] = head else {
            return Err(format!("bad {file} row of {} fields", row.len()));
        };
        let c = Call {
            name: from_utf8(name).map_err(|e| e.to_string())?,
            args,
        };
        calls += 1;
        let rust = call(&c)?;
        if results.first().is_some_and(|r| r.starts_with(b"panic: ")) {
            panics += 1;
            if let Err(e) = documented_panic(&c, &rust) {
                failures.push(e);
            }
            continue;
        }
        if rust == results {
            continue;
        }
        let lossy: Vec<Vec<u8>> = results
            .iter()
            .map(|r| bytes(String::from_utf8_lossy(r)))
            .collect();
        let has_invalid = args.iter().chain(results).any(|f| from_utf8(f).is_err());
        if has_invalid && rust == lossy {
            *string_gaps.entry(c.name.to_owned()).or_default() += 1;
            continue;
        }
        failures.push(format!(
            "{}({}): {}, Go's is {}",
            c.name,
            shown(args),
            shown(&rust),
            shown(results)
        ));
    }
    report(file, &failures, 20)?;
    if calls < min_calls {
        return Err(format!("{file}: only {calls} calls"));
    }
    ensure_eq(
        &panics,
        &go_panics,
        &format!("{file}: calls where Go panics"),
    )?;
    let want: BTreeMap<String, usize> = string_gap_calls
        .iter()
        .map(|&(name, n)| (name.to_owned(), n))
        .collect();
    ensure_eq(
        &string_gaps,
        &want,
        &format!("{file}: calls that differ by the String gap"),
    )
}

/// Every call of `functions.tsv` returns Go's results, except where Go panics (each such call
/// is checked against the port's documented behaviour) and the documented `String` gap.
#[test]
fn functions() -> TestResult {
    replay_calls("functions.tsv", 1200, GO_PANICS, &STRING_GAPS)
}

/// The adversarial review's calls (`testdata/go/review.go`), none where Go panics: bidder names
/// under Go's Unicode case mapping and folding, and sampled multibid, deal tier, alternate
/// bidder code, ATTS, floors, consented-provider and clone inputs. The only `String` gap is the
/// one [`STRING_GAPS`] documents for `ReadDealTiersFromImp`: the `"minDealTier":"5"` documents,
/// whose jsoniter error text ends in the byte `0xff`.
#[test]
fn review_functions() -> TestResult {
    replay_calls(
        "review_functions.tsv",
        8500,
        0,
        &[("ReadDealTiersFromImp", 5)],
    )
}
