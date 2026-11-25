//! The hardest Go call sites of pbs-openrtb's consumers, written against its API: the request
//! wrapper of `openrtb_ext`, the exchange, first-party data, the adapters that decode with
//! `json.Unmarshal`, the endpoints and the hooks. Each test cites the Go code it models and
//! checks what that code relies on: nil, empty and partial results, error codes, copies that
//! do not share, values that cross threads and awaits, and the merge as an extension point.

#[cfg(test)]
mod tests {
    use core::{
        future::{Future, ready},
        pin::Pin,
        task::{Context, Poll, Waker},
    };
    use std::thread;

    use pbs_errortypes::{
        BAD_SERVER_RESPONSE_ERROR_CODE, FAILED_TO_UNMARSHAL_ERROR_CODE, GoError,
        UNKNOWN_ERROR_CODE, read_code,
    };
    use pbs_gojson::RawJson;
    use pbs_jsonutil::{MergeClone, marshal, merge_clone, unmarshal, unmarshal_valid};
    use pbs_openrtb::{adcom1, gojson_bridge::EncodingJson, native1, openrtb2};
    use serde_json::{Map, Value, value::RawValue};

    /// A test result whose error is the failure message.
    type TestResult<T = ()> = Result<T, String>;

    /// Fails with `what` unless `cond` holds.
    fn ensure(cond: bool, what: &str) -> TestResult {
        if cond { Ok(()) } else { Err(what.to_owned()) }
    }

    /// `value` written with `marshal`, as text.
    fn written<T: serde::Serialize>(value: &T) -> TestResult<String> {
        let bytes = marshal(value).map_err(|e| e.to_string())?;
        String::from_utf8(bytes).map_err(|e| e.to_string())
    }

    /// Fails unless `value` writes `want`.
    fn ensure_written<T: serde::Serialize>(value: &T, want: &str, what: &str) -> TestResult {
        let text = written(value)?;
        ensure(text == want, &format!("{what}: wrote {text}, want {want}"))
    }

    // Go: openrtb_ext/request_wrapper.go:254-271 (rebuildUserExt).
    /// The rebuilt user ext: a nil user is made to carry ext bytes, an existing user has its ext
    /// replaced, nil included, and nil bytes leave a nil user nil.
    fn rebuild_user_ext(request: &mut openrtb2::BidRequest, user_json: RawJson) {
        if !user_json.is_nil() && request.user.is_none() {
            request.user = Some(Box::new(openrtb2::User {
                ext: user_json,
                ..openrtb2::User::default()
            }));
        } else if let Some(user) = request.user.as_mut() {
            user.ext = user_json;
        }
    }

    #[test]
    fn rebuilt_user_ext_makes_the_user() -> TestResult {
        let mut request = openrtb2::BidRequest::default();
        rebuild_user_ext(&mut request, RawJson::nil());
        ensure(request.user.is_none(), "nil bytes leave the user nil")?;
        rebuild_user_ext(&mut request, RawJson::from_slice(br#"{"consent":"x"}"#));
        ensure_written(
            &request,
            r#"{"id":"","imp":null,"user":{"ext":{"consent":"x"}}}"#,
            "a user made for its ext",
        )?;
        rebuild_user_ext(&mut request, RawJson::nil());
        ensure_written(
            &request,
            r#"{"id":"","imp":null,"user":{}}"#,
            "an ext set to nil",
        )
    }

    // Go: openrtb_ext/request_wrapper.go:404-424 (Clone) and
    // firstpartydata/first_party_data.go:243-248 (ptrutil.Clone of the request's user).
    #[test]
    fn copies_share_nothing() -> TestResult {
        let original: openrtb2::BidRequest = unmarshal(
            br#"{"id":"r","imp":[{"id":"1","ext":{"a":1}}],"user":{"id":"u","geo":{"lat":1.5}}}"#,
        )
        .map_err(|e| e.to_string())?;
        let mut copy = original.clone();
        ensure(copy == original, "a copy equals its original")?;
        if let Some(user) = copy.user.as_mut() {
            user.id = "v".to_owned();
            user.geo.get_or_insert_with(Box::default).lat = Some(2.5);
        }
        if let Some(imp) = copy.imp.as_mut().and_then(|imps| imps.first_mut()) {
            imp.ext = RawJson::from_slice(b"{}");
        }
        ensure_written(
            &original,
            r#"{"id":"r","imp":[{"id":"1","ext":{"a":1}}],"user":{"id":"u","geo":{"lat":1.5}}}"#,
            "the original after its copy changed",
        )?;
        ensure(copy != original, "whole requests compare")
    }

    // Go: endpoints/openrtb2/video_auction.go:516-521 (createImpressionTemplate): each imp gets
    // its own copy of the video object.
    #[test]
    fn imps_made_from_a_template_own_their_video() -> TestResult {
        let video = openrtb2::Video {
            mimes: Some(vec!["video/mp4".to_owned()]),
            startdelay: adcom1::START_PRE_ROLL.ptr(),
            protocols: Some(vec![adcom1::CREATIVE_VAST30]),
            ..openrtb2::Video::default()
        };
        let template = openrtb2::Imp {
            id: "1".to_owned(),
            ..openrtb2::Imp::default()
        };
        let mut imps: Vec<openrtb2::Imp> = (0..2)
            .map(|_| openrtb2::Imp {
                video: Some(Box::new(video.clone())),
                ..template.clone()
            })
            .collect();
        if let Some(first) = imps.first_mut().and_then(|imp| imp.video.as_mut()) {
            first.maxduration = 30;
        }
        let durations: Vec<i64> = imps
            .iter()
            .filter_map(|imp| imp.video.as_ref().map(|v| v.maxduration))
            .collect();
        ensure(durations == [30, 0], &format!("durations {durations:?}"))?;
        ensure(
            adcom1::StartDelay::val(video.startdelay) == adcom1::START_PRE_ROLL,
            "the start delay",
        )
    }

    // Go: exchange/utils.go:719-727 (mergeImpFPD).
    fn merge_imp_fpd(imp: &mut openrtb2::Imp, fpd: &RawJson, index: usize) -> Result<(), GoError> {
        merge_clone(imp, fpd.as_bytes()).map_err(|err| {
            if err.to_string().contains("invalid json on existing object") {
                GoError::generic(format!("invalid imp ext for imp[{index}]"))
            } else {
                GoError::generic(format!("invalid first party data for imp[{index}]"))
            }
        })
    }

    #[test]
    fn imp_first_party_data_merges_into_the_imp() -> TestResult {
        let mut imp: openrtb2::Imp =
            unmarshal(br#"{"id":"1","ext":{"bidder":{"a":1}}}"#).map_err(|e| e.to_string())?;
        let fpd = RawJson::from_slice(br#"{"banner":{"w":300},"ext":{"bidder":{"b":2}}}"#);
        merge_imp_fpd(&mut imp, &fpd, 0).map_err(|e| e.to_string())?;
        ensure_written(
            &imp,
            r#"{"id":"1","banner":{"w":300},"ext":{"bidder":{"a":1,"b":2}}}"#,
            "the merged imp",
        )?;
        let mut broken = openrtb2::Imp {
            ext: RawJson::from_slice(b"{"),
            ..openrtb2::Imp::default()
        };
        let err = merge_imp_fpd(&mut broken, &RawJson::from_slice(br#"{"ext":{"x":1}}"#), 1)
            .err()
            .map(|e| e.to_string());
        ensure(
            err.as_deref() == Some("invalid imp ext for imp[1]"),
            &format!("an imp ext that is not JSON: {err:?}"),
        )?;
        let wrong_type = merge_imp_fpd(&mut imp, &RawJson::from_slice(br#"{"banner":1}"#), 2)
            .err()
            .map(|e| e.to_string());
        ensure(
            wrong_type.as_deref() == Some("invalid first party data for imp[2]"),
            &format!("first party data of the wrong type: {wrong_type:?}"),
        )
    }

    // Go: firstpartydata/first_party_data.go:232-271 (resolveUser) and :348-353
    // (formatMergeCloneError).
    fn resolve_user(
        request_user: Option<&openrtb2::User>,
        fpd_user: &RawJson,
        global_data: &[openrtb2::Data],
    ) -> Result<Option<openrtb2::User>, &'static str> {
        if request_user.is_none() && fpd_user.is_nil() {
            return Ok(None);
        }
        let mut user = request_user.cloned().unwrap_or_default();
        if !global_data.is_empty() {
            user.data = Some(global_data.to_vec());
        }
        if !fpd_user.is_nil() {
            merge_clone(&mut user, fpd_user.as_bytes()).map_err(|err| {
                if err.to_string().contains("invalid json on existing object") {
                    "bad request"
                } else {
                    "bad first party data"
                }
            })?;
        }
        Ok(Some(user))
    }

    #[test]
    fn first_party_data_resolves_a_copy_of_the_user() -> TestResult {
        let request_user: openrtb2::User =
            unmarshal(br#"{"id":"u","geo":{"country":"USA"},"eids":[{"source":"s"}]}"#)
                .map_err(|e| e.to_string())?;
        let before = request_user.clone();
        let fpd = RawJson::from_slice(br#"{"geo":{"city":"Austin"},"yob":1990}"#);
        let data = [openrtb2::Data {
            id: "d".to_owned(),
            ..openrtb2::Data::default()
        }];
        let resolved = resolve_user(Some(&request_user), &fpd, &data)?;
        ensure(
            request_user == before,
            "the request's user is left as it was",
        )?;
        ensure_written(
            &resolved,
            concat!(
                r#"{"id":"u","yob":1990,"geo":{"country":"USA","city":"Austin"},"#,
                r#""data":[{"id":"d"}],"eids":[{"source":"s"}]}"#,
            ),
            "the resolved user",
        )?;
        ensure(
            resolve_user(None, &RawJson::nil(), &[])?.is_none(),
            "no user and no first party data",
        )?;
        let made = resolve_user(None, &RawJson::from_slice(br#"{"id":"n"}"#), &[])?;
        ensure(
            made.map(|user| user.id).as_deref() == Some("n"),
            "a user made from first party data",
        )?;
        ensure(
            resolve_user(
                Some(&request_user),
                &RawJson::from_slice(br#"{"geo":1}"#),
                &[],
            ) == Err("bad first party data"),
            "first party data of the wrong type",
        )
    }

    // Go: exchange/bidder.go:474-482 (addNativeTypes): the native request is used as far as it
    // decoded (json.partial-results); merge_clone into the default keeps what was read.
    #[test]
    fn a_native_request_that_fails_keeps_its_assets() -> TestResult {
        let request = br#"{"assets":[{"id":1,"img":{"type":3}}],"ver":1}"#;
        let decoded = unmarshal_valid::<native1::request::Request>(request).err();
        ensure(
            decoded.as_ref().map(read_code) == Some(FAILED_TO_UNMARSHAL_ERROR_CODE),
            "unmarshal_valid returns the error alone",
        )?;
        let mut payload = native1::request::Request::default();
        let failed = merge_clone(&mut payload, request).is_err();
        let image_type = payload
            .assets
            .as_deref()
            .and_then(<[_]>::first)
            .and_then(|asset| asset.img.as_ref())
            .map(|img| img.r#type);
        ensure(
            failed && image_type == Some(native1::IMAGE_ASSET_TYPE_MAIN),
            &format!("the assets read before the failure: {image_type:?}"),
        )
    }

    // Go: exchange/utils.go:1143-1163 (getMediaTypeForBid).
    fn media_type_for_bid(bid: &openrtb2::Bid) -> Result<&'static str, String> {
        if bid.mtype > openrtb2::MarkupType(0) {
            match bid.mtype {
                openrtb2::MARKUP_BANNER => Ok("banner"),
                openrtb2::MARKUP_VIDEO => Ok("video"),
                openrtb2::MARKUP_AUDIO => Ok("audio"),
                openrtb2::MARKUP_NATIVE => Ok("native"),
                openrtb2::MarkupType(_) => Err(format!(
                    "Failed to parse bid mType for impression \"{}\"",
                    bid.impid
                )),
            }
        } else {
            Ok("from the bid's ext")
        }
    }

    // Go: exchange/exchange.go:1501-1512 (stored auction responses): each stored seat bid's
    // bids take the request's imp id.
    #[test]
    fn stored_seat_bids_take_the_imp_id() -> TestResult {
        let stored = concat!(
            r#"[{"bid":[{"id":"a","impid":"x","price":1,"mtype":2},"#,
            r#"{"id":"b","impid":"x","price":2,"mtype":9}],"seat":"s"},{"seat":"t"}]"#,
        )
        .as_bytes();
        let mut seat_bids: Vec<openrtb2::SeatBid> =
            unmarshal_valid(stored).map_err(|e| e.to_string())?;
        let mut types = Vec::new();
        for seat in &mut seat_bids {
            for bid in seat.bid.iter_mut().flatten() {
                "imp-1".clone_into(&mut bid.impid);
                types.push(media_type_for_bid(bid));
            }
        }
        ensure(
            types
                == [
                    Ok("video"),
                    Err("Failed to parse bid mType for impression \"imp-1\"".to_owned()),
                ],
            &format!("the media types {types:?}"),
        )?;
        let nil: Result<Vec<openrtb2::SeatBid>, _> = unmarshal_valid(b"null");
        ensure(
            nil.as_ref().is_ok_and(Vec::is_empty) || nil.is_err(),
            "a stored response of null",
        )
    }

    // Go: adapters/zentotem/zentotem.go:70-73 (also loopme.go:58, tradplus.go:111,
    // adverxo.go:99) and adapters/resetdigital/resetdigital.go:130-135.
    #[test]
    fn adapters_decode_bid_responses_as_json_unmarshal() -> TestResult {
        let body = concat!(
            r#"{"id":"r","cur":"USD","#,
            r#""seatbid":[{"bid":[{"id":"b","impid":"i","price":1.5,"ext":null}]}]}"#,
        )
        .as_bytes();
        let response =
            EncodingJson::<openrtb2::BidResponse>::unmarshal(body).map_err(|e| e.to_string())?;
        let ext = response
            .seatbid
            .iter()
            .flatten()
            .flat_map(|seat| seat.bid.iter().flatten())
            .map(|bid| bid.ext.as_bytes().to_vec())
            .next();
        ensure(
            ext.as_deref() == Some(&b"null"[..]),
            "json.Unmarshal keeps a bid's ext: null",
        )?;
        let zentotem = EncodingJson::<openrtb2::BidResponse>::unmarshal(b"{")
            .err()
            .map(|err| read_code(&err));
        ensure(
            zentotem == Some(UNKNOWN_ERROR_CODE),
            "json.Unmarshal's error has no errortypes code",
        )?;
        let reset = EncodingJson::<openrtb2::BidResponse>::unmarshal(b"{")
            .map_err(|err| GoError::bad_server_response(format!("Bad server response: {err}")))
            .err();
        ensure(
            reset.as_ref().map(read_code) == Some(BAD_SERVER_RESPONSE_ERROR_CODE)
                && reset
                    .as_ref()
                    .is_some_and(|err| err.message().starts_with("Bad server response: ")),
            "resetdigital's wrapper",
        )
    }

    // Go: exchange/exchange.go:1373-1390 (makeBidExtJSON): a bid's ext read as a map, written
    // back with Prebid's keys.
    #[test]
    fn a_bid_ext_round_trips_through_a_map() -> TestResult {
        let mut bid = openrtb2::Bid {
            price: 1.5,
            ext: RawJson::from_slice(br#"{"z":{"b":1,"a":2},"prebid":{"meta":{}}}"#),
            ..openrtb2::Bid::default()
        };
        let mut ext_map: Map<String, Value> = if bid.ext.is_empty() {
            Map::new()
        } else {
            unmarshal(bid.ext.as_bytes()).map_err(|e| e.to_string())?
        };
        let _no_cpm = ext_map.insert("origbidcpm".to_owned(), Value::from(1.5));
        let _no_cur = ext_map.insert("origbidcur".to_owned(), Value::from("USD"));
        bid.ext = RawJson::from_vec(marshal(&ext_map).map_err(|e| e.to_string())?);
        ensure_written(
            &bid,
            concat!(
                r#"{"id":"","impid":"","price":1.5,"ext":{"origbidcpm":1.5,"origbidcur":"USD","#,
                r#""prebid":{"meta":{}},"z":{"a":2,"b":1}}}"#,
            ),
            "the bid with its ext rebuilt",
        )?;
        let value = serde_json::to_value(&bid).map_err(|e| e.to_string())?;
        let back: openrtb2::Bid = serde_json::from_value(value).map_err(|e| e.to_string())?;
        ensure(back == bid, "a bid through serde_json::Value")
    }

    // Go: openrtb_ext/convert_up.go:83-123 (moveSupplyChainFrom25To26, moveGDPRFrom25To26) and
    // :151 (moveEIDFrom25To26): 2.5 locations move to the 2.6 fields, made where missing.
    #[test]
    fn convert_up_moves_into_made_objects() -> TestResult {
        let mut request: openrtb2::BidRequest = unmarshal(
            concat!(
                r#"{"id":"r","imp":[],"regs":{"ext":{"gdpr":1}},"#,
                r#""user":{"ext":{"eids":[{"source":"s"}]}}}"#,
            )
            .as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        let schain: openrtb2::SupplyChain =
            unmarshal(br#"{"complete":1,"nodes":[{"asi":"a","sid":"1","hp":1}],"ver":"1.0"}"#)
                .map_err(|e| e.to_string())?;
        let source = request.source.get_or_insert_with(Box::default);
        if source.schain.is_none() {
            source.schain = Some(Box::new(schain));
        }
        if let Some(regs) = request.regs.as_mut()
            && regs.gdpr.is_none()
        {
            regs.gdpr = Some(1);
            regs.ext = RawJson::nil();
        }
        if let Some(user) = request.user.as_mut() {
            let ext: Map<String, Value> =
                unmarshal(user.ext.as_bytes()).map_err(|e| e.to_string())?;
            let eids = ext.get("eids").cloned().unwrap_or(Value::Null);
            user.eids = serde_json::from_value(eids).map_err(|e| e.to_string())?;
            user.ext = RawJson::nil();
        }
        ensure_written(
            &request,
            concat!(
                r#"{"id":"r","imp":[],"user":{"eids":[{"source":"s"}]},"#,
                r#""source":{"schain":{"complete":1,"nodes":[{"asi":"a","sid":"1","hp":1}],"#,
                r#""ver":"1.0"}},"regs":{"gdpr":1}}"#,
            ),
            "the request in its 2.6 locations",
        )
    }

    /// A boxed future, as the hooks' and bidders' async boundaries hold one.
    type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// A stage that changes the request across an await, as a hook does to its payload.
    fn stage(request: &mut openrtb2::BidRequest) -> BoxFuture<'_, usize> {
        Box::pin(async move {
            ready(()).await;
            request.tmax = 100;
            request
                .imp
                .get_or_insert_with(Vec::new)
                .push(openrtb2::Imp::default());
            ready(()).await;
            request.imp.as_ref().map_or(0, Vec::len)
        })
    }

    /// Compiles when `T` can cross threads and outlive any borrow.
    const fn send_sync_static<T: Send + Sync + 'static>() {}

    // Go: hooks/hookexecution/execution.go (hooks run in goroutines and change the payload
    // they are given) and exchange/exchange.go:730-790 (bidders run in goroutines sharing the
    // request).
    #[test]
    fn requests_cross_threads_and_awaits() -> TestResult {
        send_sync_static::<openrtb2::BidRequest>();
        send_sync_static::<openrtb2::BidResponse>();
        send_sync_static::<native1::request::Request>();
        send_sync_static::<EncodingJson<openrtb2::BidResponse>>();
        let mut request = openrtb2::BidRequest::default();
        let mut future = stage(&mut request);
        let mut cx = Context::from_waker(Waker::noop());
        let imps = match future.as_mut().poll(&mut cx) {
            Poll::Ready(imps) => imps,
            Poll::Pending => return Err("the stage did not finish".to_owned()),
        };
        drop(future);
        ensure(imps == 1 && request.tmax == 100, "the stage's changes")?;
        let shared = &request;
        let read = || shared.imp.as_ref().map_or(0, Vec::len);
        let counted = thread::scope(|scope| {
            let first = scope.spawn(read);
            let second = scope.spawn(read);
            first.join().unwrap_or(0) + second.join().unwrap_or(0)
        });
        ensure(counted == 2, "two bidders read the same request")
    }

    // Go: util/jsonutil/jsonutil.go MergeClone(v any, data) takes any value: the merge is an
    // extension point a module can hold as a trait object.
    #[test]
    fn merges_go_through_trait_objects() -> TestResult {
        let mut imp = openrtb2::Imp::default();
        let mut user = openrtb2::User::default();
        let patch: Box<RawValue> =
            serde_json::from_str(r#"{"id":"x"}"#).map_err(|e| e.to_string())?;
        let targets: [&mut dyn MergeClone; 2] = [&mut imp, &mut user];
        for target in targets {
            target.merge_clone(&patch).map_err(|e| e.to_string())?;
        }
        let as_dyn: &mut dyn MergeClone = &mut imp;
        merge_clone(as_dyn, br#"{"tagid":"t"}"#).map_err(|e| e.to_string())?;
        ensure(
            imp.id == "x" && imp.tagid == "t" && user.id == "x",
            "merged through dyn MergeClone",
        )
    }
}
