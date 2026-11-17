//! Replays `encode.tsv`, the encodings of values that no JSON document decodes to.
//!
//! The values (NaN and infinite floats, invalid `json.RawMessage` bytes) are built here as
//! `testdata/go/encode.go` builds them in Go, and encoded the three ways. jsoniter's errors name
//! every field encoded after the failing one, and a later float error replaces an earlier one;
//! `encoding/json` stops at the first error.

use std::collections::BTreeMap;

use pbs_gojson::RawJson;
use pbs_openrtb::{adcom1, openrtb2, openrtb3};

use super::{
    golden::{Encoded, encode},
    support::{TestResult, ensure_eq, golden_rows, report, show},
};

/// Values of `encodeCases` by name, each encoded.
type Values = Vec<(&'static str, [Encoded; 3])>;

fn bid(id: &str, price: f64) -> openrtb2::Bid {
    openrtb2::Bid {
        id: id.to_owned(),
        price,
        ..openrtb2::Bid::default()
    }
}

/// A bid whose `ext` is not valid JSON.
fn invalid_ext() -> openrtb2::Bid {
    openrtb2::Bid {
        ext: RawJson::from("{bad}"),
        ..openrtb2::Bid::default()
    }
}

fn imp(id: &str, bidfloor: f64) -> openrtb2::Imp {
    openrtb2::Imp {
        id: id.to_owned(),
        bidfloor,
        ..openrtb2::Imp::default()
    }
}

fn request(id: &str, imps: Vec<openrtb2::Imp>) -> openrtb2::BidRequest {
    openrtb2::BidRequest {
        id: id.to_owned(),
        imp: Some(imps),
        ..openrtb2::BidRequest::default()
    }
}

/// The bid, seat bid and bid response values.
fn bids() -> Values {
    vec![
        (
            "bid price NaN",
            encode(&openrtb2::Bid {
                w: 3,
                ..bid("b", f64::NAN)
            }),
        ),
        ("bid price -Inf", encode(&bid("", f64::NEG_INFINITY))),
        (
            "second bid price +Inf",
            encode(&openrtb2::BidResponse {
                id: "r".to_owned(),
                seatbid: Some(vec![openrtb2::SeatBid {
                    bid: Some(vec![bid("1", 1.0), bid("2", f64::INFINITY)]),
                    seat: "s".to_owned(),
                    ..openrtb2::SeatBid::default()
                }]),
                cur: "USD".to_owned(),
                ..openrtb2::BidResponse::default()
            }),
        ),
        (
            "bid ext invalid",
            encode(&openrtb2::Bid {
                ext: RawJson::from("{bad}"),
                ..bid("b", 1.0)
            }),
        ),
        (
            "bid ext invalid then price NaN",
            encode(&openrtb2::SeatBid {
                bid: Some(vec![invalid_ext(), bid("", f64::NAN)]),
                ..openrtb2::SeatBid::default()
            }),
        ),
        (
            "bid price NaN then ext invalid",
            encode(&openrtb2::SeatBid {
                bid: Some(vec![bid("", f64::NAN), invalid_ext()]),
                ext: RawJson::from(r#"{"a":1}"#),
                ..openrtb2::SeatBid::default()
            }),
        ),
    ]
}

/// The bid request values.
fn requests() -> Values {
    vec![
        (
            "imp bidfloor NaN then qty and refresh",
            encode(&request(
                "x",
                vec![openrtb2::Imp {
                    qty: Some(Box::new(openrtb2::Qty {
                        multiplier: 2.0,
                        ..openrtb2::Qty::default()
                    })),
                    refresh: Some(Box::new(openrtb2::Refresh {
                        count: Some(1),
                        ..openrtb2::Refresh::default()
                    })),
                    ..imp("i", f64::NAN)
                }],
            )),
        ),
        (
            "imp bidfloor NaN then a second imp",
            encode(&request(
                "x",
                vec![
                    imp("i", f64::NAN),
                    openrtb2::Imp {
                        banner: Some(Box::default()),
                        ..imp("j", 0.0)
                    },
                ],
            )),
        ),
        (
            "imp bidfloor NaN then qty multiplier +Inf",
            encode(&request(
                "x",
                vec![openrtb2::Imp {
                    qty: Some(Box::new(openrtb2::Qty {
                        multiplier: f64::INFINITY,
                        ..openrtb2::Qty::default()
                    })),
                    ..imp("i", f64::NAN)
                }],
            )),
        ),
        (
            "deal durfloors bidfloor NaN",
            encode(&request(
                "",
                vec![openrtb2::Imp {
                    pmp: Some(Box::new(openrtb2::PMP {
                        deals: Some(vec![openrtb2::Deal {
                            id: "d".to_owned(),
                            durfloors: Some(vec![openrtb2::DurFloors {
                                mindur: 1,
                                bidfloor: f64::NAN,
                                ..openrtb2::DurFloors::default()
                            }]),
                            ext: RawJson::from(r#"{"a":1}"#),
                            ..openrtb2::Deal::default()
                        }]),
                        ..openrtb2::PMP::default()
                    })),
                    ..imp("", 0.0)
                }],
            )),
        ),
        (
            "geo lat NaN then user",
            encode(&openrtb2::BidRequest {
                device: Some(Box::new(openrtb2::Device {
                    geo: Some(Box::new(openrtb2::Geo {
                        lat: Some(f64::NAN),
                        ..openrtb2::Geo::default()
                    })),
                    ua: "u".to_owned(),
                    ..openrtb2::Device::default()
                })),
                user: Some(Box::new(openrtb2::User {
                    id: "u".to_owned(),
                    ..openrtb2::User::default()
                })),
                ..openrtb2::BidRequest::default()
            }),
        ),
    ]
}

/// The imp and video values, and those of adcom1 and openrtb3.
fn others() -> Values {
    vec![
        (
            "video mincpmpersec +Inf",
            encode(&openrtb2::Video {
                mincpmpersec: f64::INFINITY,
                poddur: 3,
                ..openrtb2::Video::default()
            }),
        ),
        (
            "metric value NaN after banner format",
            encode(&openrtb2::Imp {
                banner: Some(Box::new(openrtb2::Banner {
                    format: Some(vec![openrtb2::Format {
                        w: 1,
                        ..openrtb2::Format::default()
                    }]),
                    ..openrtb2::Banner::default()
                })),
                metric: Some(vec![openrtb2::Metric {
                    value: f64::NAN,
                    ..openrtb2::Metric::default()
                }]),
                ..openrtb2::Imp::default()
            }),
        ),
        (
            "adcom1 geo lon -Inf",
            encode(&adcom1::Geo {
                lat: 1.0,
                lon: f64::NEG_INFINITY,
                country: "c".to_owned(),
                ..adcom1::Geo::default()
            }),
        ),
        (
            "openrtb3 bid price NaN",
            encode(&openrtb3::Body {
                openrtb: openrtb3::OpenRTB {
                    ver: "3.0".to_owned(),
                    response: Some(Box::new(openrtb3::Response {
                        id: "r".to_owned(),
                        seatbid: Some(vec![openrtb3::SeatBid {
                            seat: "s".to_owned(),
                            bid: Some(vec![openrtb3::Bid {
                                id: "b".to_owned(),
                                price: f64::NAN,
                                ..openrtb3::Bid::default()
                            }]),
                            ..openrtb3::SeatBid::default()
                        }]),
                        ..openrtb3::Response::default()
                    })),
                    ..openrtb3::OpenRTB::default()
                },
            }),
        ),
        (
            "openrtb3 item flr +Inf",
            encode(&openrtb3::Item {
                id: "i".to_owned(),
                flr: f64::INFINITY,
                flrcur: "USD".to_owned(),
                ..openrtb3::Item::default()
            }),
        ),
    ]
}

/// A golden encoding: `'` and the bytes, or `!` and the error text.
fn want(cell: &[u8]) -> TestResult<Encoded> {
    match cell.split_first() {
        Some((b'\'', bytes)) => Ok(Ok(bytes.to_vec())),
        Some((b'!', text)) => Ok(Err(text.to_vec())),
        _ => Err(format!("bad encode.tsv cell {}", show(cell))),
    }
}

fn describe(e: &Encoded) -> String {
    match e {
        Ok(bytes) => show(bytes),
        Err(text) => format!("error {}", show(text)),
    }
}

/// Every value of `encode.tsv` encodes as in Go, and every one is built here, once.
#[test]
fn encodings() -> TestResult {
    let built: Values = bids()
        .into_iter()
        .chain(requests())
        .chain(others())
        .collect();
    let count = built.len();
    let mut values: BTreeMap<&str, [Encoded; 3]> = built.into_iter().collect();
    ensure_eq(&values.len(), &count, "distinct value names")?;
    let mut failures = Vec::new();
    for row in golden_rows("encode.tsv")? {
        if row.first().is_some_and(|f| f == b"#go") {
            continue;
        }
        let [name, c1, c2, c3] = row.as_slice() else {
            return Err(format!("bad encode.tsv row of {} fields", row.len()));
        };
        let name = String::from_utf8_lossy(name).into_owned();
        let rust = values
            .remove(name.as_str())
            .ok_or_else(|| format!("encode.tsv value {name:?} is not built here"))?;
        for (i, (cell, got)) in [c1, c2, c3].into_iter().zip(&rust).enumerate() {
            let want = want(cell)?;
            if &want != got {
                failures.push(format!(
                    "{name}: encoding {} is {}, Go's is {}",
                    i + 1,
                    describe(got),
                    describe(&want)
                ));
            }
        }
    }
    let unused: Vec<&str> = values.into_keys().collect();
    ensure_eq(
        &unused,
        &Vec::new(),
        "values built here but not in encode.tsv",
    )?;
    report("encode.tsv", &failures, 20)
}
