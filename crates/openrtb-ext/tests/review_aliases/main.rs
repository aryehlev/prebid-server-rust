//! Replays `testdata/golden/review_aliases.tsv` (`testdata/go/review.go`): `SetAliasBidderName`
//! with reserved, case-folded, colliding, repeated, empty and non-ASCII names, each followed by
//! the lookups it changes, in the generator's order (`function label arg... -> result...`).
//!
//! Registering an alias changes the registry for good, as in Go, so every call runs in one
//! test, in order, in a binary of its own.

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf, str::from_utf8};

    use pbs_errortypes::GoError;
    use pbs_gocompat::strconv::unquote_bytes;
    use pbs_gojson::{GoEncode, RawJson, jsonutil};
    use pbs_openrtb::openrtb2;
    use pbs_openrtb_ext::openrtb_ext::{self, BidderName};

    type TestResult<T = ()> = Result<T, String>;

    /// The results of one call, as the generator writes them.
    type Results = Vec<Vec<u8>>;

    /// The rows of the golden: tab-separated `strconv.QuoteToASCII` fields, unquoted.
    fn rows() -> TestResult<Vec<Vec<Vec<u8>>>> {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/golden/review_aliases.tsv");
        let text =
            fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        text.lines()
            .enumerate()
            .map(|(n, line)| {
                line.split('\t')
                    .map(|field| {
                        unquote_bytes(field)
                            .map_err(|e| format!("review_aliases.tsv:{}: {e}: {field}", n + 1))
                    })
                    .collect()
            })
            .collect()
    }

    /// `jsonutil.Marshal`'s encoding of `v`, or `!` and its error.
    fn marshal<T: GoEncode + ?Sized>(v: &T) -> Vec<u8> {
        jsonutil::marshal(v).unwrap_or_else(|e| {
            let mut out = b"!".to_vec();
            out.extend(e.message.into_bytes());
            out
        })
    }

    fn boolean(b: bool) -> Vec<u8> {
        if b {
            b"true".to_vec()
        } else {
            b"false".to_vec()
        }
    }

    /// An error's Go dynamic type and text, both empty for nil.
    fn error_fields(err: Option<&GoError>) -> Results {
        err.map_or_else(
            || vec![Vec::new(), Vec::new()],
            |e| {
                vec![
                    e.kind().go_type().as_bytes().to_vec(),
                    e.message().as_bytes().to_vec(),
                ]
            },
        )
    }

    /// The strings joined with `,`.
    fn joined<I: IntoIterator<Item = String>>(items: I) -> Vec<u8> {
        items.into_iter().collect::<Vec<_>>().join(",").into_bytes()
    }

    /// The names, sorted (as the generator lists Go's maps and sets) and joined with `,`.
    fn sorted<I: IntoIterator<Item = String>>(items: I) -> Vec<u8> {
        let mut names: Vec<String> = items.into_iter().collect();
        names.sort();
        joined(names)
    }

    /// Calls the Rust port of the row's Go function with the generator's arguments.
    fn call(name: &str, args: &[Vec<u8>]) -> TestResult<Results> {
        let text = |i: usize| -> TestResult<String> {
            let arg = args
                .get(i)
                .ok_or_else(|| format!("{name}: no argument {i}"))?;
            from_utf8(arg)
                .map(str::to_owned)
                .map_err(|e| format!("{name}: argument {i}: {e}"))
        };
        Ok(match name {
            "SetAliasBidderName" => error_fields(
                openrtb_ext::set_alias_bidder_name(&text(0)?, BidderName::from(text(1)?))
                    .err()
                    .as_ref(),
            ),
            "NormalizeBidderName" => openrtb_ext::normalize_bidder_name(&text(0)?).map_or_else(
                || vec![Vec::new(), boolean(false)],
                |n| vec![n.as_str().as_bytes().to_vec(), boolean(true)],
            ),
            "NormalizeBidderNameOrUnchanged" => vec![
                openrtb_ext::normalize_bidder_name_or_unchanged(&text(0)?)
                    .as_str()
                    .as_bytes()
                    .to_vec(),
            ],
            "IsBidderNameReserved" => {
                vec![boolean(openrtb_ext::is_bidder_name_reserved(&text(0)?))]
            }
            "CoreBidderNames" => vec![joined(
                openrtb_ext::core_bidder_names()
                    .iter()
                    .map(|n| n.as_str().to_owned()),
            )],
            "BuildBidderStringSlice" => vec![joined(openrtb_ext::build_bidder_string_slice())],
            "BuildBidderMap" => vec![sorted(
                openrtb_ext::build_bidder_map()
                    .into_iter()
                    .map(|(k, v)| format!("{k}={v}")),
            )],
            "BuildBidderNameHashSet" => vec![sorted(
                openrtb_ext::build_bidder_name_hash_set()
                    .into_iter()
                    .map(|n| n.as_str().to_owned()),
            )],
            "GetAliasBidderToParent" => vec![marshal(&openrtb_ext::get_alias_bidder_to_parent())],
            "ReadDealTiersFromImp" => {
                let imp = openrtb2::Imp {
                    ext: RawJson::from_slice(text(0)?.as_bytes()),
                    ..openrtb2::Imp::default()
                };
                match openrtb_ext::read_deal_tiers_from_imp(&imp) {
                    Ok(tiers) => {
                        let mut out = vec![marshal(&tiers)];
                        out.extend(error_fields(None));
                        out
                    }
                    Err(e) => {
                        let mut out = vec![b"null".to_vec()];
                        out.extend(error_fields(Some(&e)));
                        out
                    }
                }
            }
            _ => return Err(format!("no Rust port for Go function {name}")),
        })
    }

    /// Text for a failure message.
    fn shown(fields: &[Vec<u8>]) -> String {
        format!(
            "{:?}",
            fields
                .iter()
                .map(|f| String::from_utf8_lossy(f).into_owned())
                .collect::<Vec<_>>()
        )
    }

    /// Every call returns Go's results, in the generator's order.
    #[test]
    fn aliases_match_go() -> TestResult {
        let mut failures = Vec::new();
        let mut calls = 0;
        for row in rows()? {
            if row.first().is_some_and(|f| f == b"#go") {
                continue;
            }
            let split = row
                .iter()
                .position(|f| f == b"->")
                .ok_or("a review_aliases.tsv row without \"->\"")?;
            let (head, results) = row.split_at(split);
            let results = results.get(1..).unwrap_or_default();
            let [name, _label, args @ ..] = head else {
                return Err(format!(
                    "bad review_aliases.tsv row of {} fields",
                    row.len()
                ));
            };
            let name = from_utf8(name).map_err(|e| e.to_string())?;
            calls += 1;
            let rust = call(name, args)?;
            if rust != results {
                failures.push(format!(
                    "call {calls}: {name}({}): {}, Go's is {}",
                    shown(args),
                    shown(&rust),
                    shown(results)
                ));
            }
        }
        if calls < 150 {
            return Err(format!("review_aliases.tsv: only {calls} calls"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "review_aliases.tsv: {} failures:\n{}",
                failures.len(),
                failures.join("\n")
            ))
        }
    }
}
