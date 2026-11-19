//! The hardest calls of the units that use the crate, written against its API, each test after
//! the Go code it cites: the account defaults and the account's row merged into a new account and
//! the DSA default unpacked over what the defaults left (ACCOUNT, CFG-TYPES, CFG-FULL); an imp's
//! bidder first-party data merged into a copy that shares its pointers, and its sanitized ext
//! written (the exchange); first-party data exts merged by merge patch (FPD-SCHAIN-VERSION);
//! `RequestWrapper`'s user and regs exts; stored requests, stored imps and the HTTP fetcher's
//! response (STORED-CORE, OPENRTB2-AUCTION); the usersync cookie (USERSYNC-PBS); the Audience
//! Network body (ADP-API); module configs (MOD-WURFL, MOD-SCOPE3, MOD-51D); and the values that
//! cross an `async` boundary.

#[cfg(test)]
pub mod support;

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        future::Future,
        pin::Pin,
        sync::Arc,
        task::{Context, Poll, Waker},
    };

    use pbs_errortypes::GoError;
    use pbs_jsonutil::{
        ElementError, ElementSpan, IntString, MergeClone, MergeCloneError, MergePatchError,
        StringInt, drop_element, is_nil_or_empty, is_zero, marshal, marshal_raw, merge_clone,
        merge_field, merge_patch, merge_struct, null_default, parse_into_string, unmarshal,
        unmarshal_valid,
    };
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::value::RawValue;

    use super::support::{TestResult, ensure_eq, golden, list, text};

    /// Implements `MergeClone` as typegen does: each key merges into its field, which errors name
    /// as Go names it.
    macro_rules! merge_clone_fields {
        (
            $ty:ident,
            $go:literal,
            { $($key:literal => $field:ident as $go_field:literal),+ $(,)? }
        ) => {
            impl MergeClone for $ty {
                fn merge_clone(&mut self, patch: &RawValue) -> Result<(), MergeCloneError> {
                    merge_struct(patch, |key, value| match key {
                        $($key => merge_field(&mut self.$field, value, $go, $go_field),)+
                        _ => Ok(()),
                    })
                }
            }
        };
    }

    /// The raw value `text`.
    fn raw(text: &str) -> TestResult<Box<RawValue>> {
        Ok(RawValue::from_string(text.to_owned())?)
    }

    /// The text of a raw field.
    fn text_of(field: Option<&RawValue>) -> Option<&str> {
        field.map(RawValue::get)
    }

    /// `openrtb_ext.ExtBidDSATransparency`.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    struct DsaTransparency {
        domain: String,
        params: Vec<i64>,
    }

    merge_clone_fields!(DsaTransparency, "openrtb_ext.ExtBidDSATransparency", {
        "domain" => domain as "Domain",
        "dsaparams" => params as "Params",
    });

    /// `openrtb_ext.ExtRegsDSA`, the fields the test uses.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    struct ExtRegsDsa {
        required: Option<i8>,
        transparency: Vec<DsaTransparency>,
    }

    merge_clone_fields!(ExtRegsDsa, "openrtb_ext.ExtRegsDSA", {
        "dsarequired" => required as "Required",
        "transparency" => transparency as "Transparency",
    });

    /// `config.AccountDSA`: `DefaultUnpacked` has no json tag, so the account defaults Prebid
    /// writes carry it under its Go name.
    #[derive(Debug, Clone, Default)]
    struct AccountDsa {
        default: String,
        default_unpacked: Option<Arc<ExtRegsDsa>>,
    }

    merge_clone_fields!(AccountDsa, "config.AccountDSA", {
        "default" => default as "Default",
        "DefaultUnpacked" => default_unpacked as "DefaultUnpacked",
    });

    /// `config.AccountPrivacy`, its DSA.
    #[derive(Debug, Clone, Default)]
    struct AccountPrivacy {
        dsa: Option<Arc<AccountDsa>>,
    }

    merge_clone_fields!(AccountPrivacy, "config.AccountPrivacy", { "dsa" => dsa as "DSA" });

    /// `config.AccountModules`, `map[string]map[string]json.RawMessage`, whose values a
    /// `MergeClone` type holds as `Option<Box<RawValue>>` (a nil `json.RawMessage` is `None`).
    type AccountModules = BTreeMap<String, BTreeMap<String, Option<Box<RawValue>>>>;

    /// `config.Account`, the fields the test uses.
    #[derive(Debug, Clone, Default)]
    struct Account {
        id: String,
        disabled: bool,
        privacy: AccountPrivacy,
        modules: Option<AccountModules>,
    }

    merge_clone_fields!(Account, "config.Account", {
        "id" => id as "ID",
        "disabled" => disabled as "Disabled",
        "privacy" => privacy as "Privacy",
        "modules" => modules as "Modules",
    });

    /// Go's `config.UnpackDSADefault`: `jsonutil.Unmarshal` of the default into the
    /// `DefaultUnpacked` the account defaults may have left, which `merge_clone` gives, but for the
    /// transparency entries, which it builds new (`json.decode-reuses-elements`).
    fn unpack_dsa_default(dsa: Option<&mut Arc<AccountDsa>>) -> Result<(), GoError> {
        let Some(shared) = dsa else {
            return Ok(());
        };
        if shared.default.is_empty() {
            return Ok(());
        }
        let target = Arc::make_mut(shared);
        merge_clone(&mut target.default_unpacked, target.default.as_bytes())
    }

    /// Go's `newAccountTransform`: the defaults, then the account's row, merged into a new
    /// account, the DSA default unpacked and the ID filled.
    fn account_transform(
        defaults: Option<&[u8]>,
        id: &str,
        row: &[u8],
    ) -> Result<Account, GoError> {
        let malformed = |what: &str| {
            GoError::malformed_acct(format!(
                "The prebid-server account config{what} for account id \"{id}\" is malformed. \
                 Please reach out to the prebid server host."
            ))
        };
        let mut account = Account::default();
        if let Some(defaults) = defaults {
            merge_clone(&mut account, defaults).map_err(|_error| malformed(""))?;
        }
        merge_clone(&mut account, row).map_err(|_error| malformed(""))?;
        unpack_dsa_default(account.privacy.dsa.as_mut()).map_err(|_error| malformed(" DSA"))?;
        if account.id.is_empty() {
            id.clone_into(&mut account.id);
        }
        Ok(account)
    }

    /// The account defaults merge first and the account's row over them: a module the row names
    /// gets the row's config alone, the others keep the defaults'; the DSA default the row sets is
    /// unpacked over the `DefaultUnpacked` the defaults left, keeping what it does not set; and a
    /// member of the wrong type fails the account as `MalformedAcct`.
    // Go: account/fetcher.go:136-162 (newAccountTransform), config/config.go:907-912
    // (UnpackDSADefault)
    #[test]
    fn account_defaults_then_row() -> TestResult {
        let defaults = concat!(
            r#"{"disabled":false,"privacy":{"dsa":{"default":"{\"dsarequired\":1}","#,
            r#""DefaultUnpacked":{"dsarequired":1,"transparency":"#,
            r#"[{"domain":"host.example","dsaparams":[1]}]}}},"#,
            r#""modules":{"prebid":{"a":{"x":1},"b":{"y":2}},"other":{"c":{"z":0}}}}"#,
        );
        let row = concat!(
            r#"{"modules":{"prebid":{"a":{"x":2}}},"#,
            r#""privacy":{"dsa":{"default":"{\"dsarequired\":3}"}}}"#,
        );
        let account = account_transform(Some(defaults.as_bytes()), "acc-1", row.as_bytes())?;
        ensure_eq(&account.id.as_str(), &"acc-1", "id")?;
        let modules = account.modules.as_ref().ok_or("modules")?;
        let module = |name: &str, key: &str| {
            modules
                .get(name)
                .and_then(|configs| configs.get(key))
                .and_then(|config| text_of(config.as_deref()))
        };
        ensure_eq(
            &module("prebid", "a"),
            &Some(r#"{"x":2}"#),
            "the row's module",
        )?;
        ensure_eq(&module("prebid", "b"), &None, "replaced with the row's")?;
        ensure_eq(
            &module("other", "c"),
            &Some(r#"{"z":0}"#),
            "a default module",
        )?;
        let dsa = account.privacy.dsa.as_ref().ok_or("dsa")?;
        let unpacked = dsa.default_unpacked.as_deref().ok_or("default unpacked")?;
        let host = DsaTransparency {
            domain: "host.example".to_owned(),
            params: vec![1],
        };
        ensure_eq(
            unpacked,
            &ExtRegsDsa {
                required: Some(3),
                transparency: vec![host],
            },
            "unpacked over the defaults'",
        )?;

        let malformed = account_transform(None, "acc-2", br#"{"disabled":"yes"}"#)
            .err()
            .ok_or("a string for a bool")?;
        let want = "The prebid-server account config for account id \"acc-2\" is malformed. \
                    Please reach out to the prebid server host.";
        ensure_eq(
            &(malformed.message(), malformed.code()),
            &(want, 10),
            "MalformedAcct",
        )?;
        let dsa_error = account_transform(None, "acc-3", br#"{"privacy":{"dsa":{"default":"{"}}}"#)
            .err()
            .ok_or("a DSA default that is not JSON")?;
        ensure_eq(
            &dsa_error.message().contains("config DSA for"),
            &true,
            "the DSA text",
        )
    }

    /// `openrtb2.Banner`, the field the test uses.
    #[derive(Debug, Clone, Default)]
    struct Banner {
        id: String,
    }

    merge_clone_fields!(Banner, "openrtb2.Banner", { "id" => id as "ID" });

    /// `openrtb2.Imp`, the fields the test uses, its pointer to a struct shared as typegen holds
    /// it.
    #[derive(Debug, Clone, Default)]
    struct Imp {
        id: String,
        banner: Option<Arc<Banner>>,
        ext: Option<Box<RawValue>>,
    }

    merge_clone_fields!(Imp, "openrtb2.Imp", {
        "id" => id as "ID",
        "banner" => banner as "Banner",
        "ext" => ext as "Ext",
    });

    /// Go's `mergeImpFPD`: the bidder's first-party data merged into the imp's copy, its error
    /// told apart by its text.
    fn merge_imp_fpd(imp: &mut Imp, fpd: &RawValue, index: usize) -> Result<(), GoError> {
        merge_clone(imp, fpd.get().as_bytes()).map_err(|error| {
            if error.message().contains("invalid json on existing object") {
                GoError::generic(format!("invalid imp ext for imp[{index}]"))
            } else {
                GoError::generic(format!("invalid first party data for imp[{index}]"))
            }
        })
    }

    /// The bidder's first-party data merges into a copy of the imp that shares its pointers,
    /// leaving the imp and the banner it shares as they were, and its ext by merge patch; the
    /// sanitized ext is written compacted into the copy; an ext the merge cannot read (here a
    /// number beyond `f64`'s range, `json.merge-patch-limits`) and a patch of the wrong type give
    /// Prebid's two texts.
    // Go: exchange/utils.go:686-710 (impCopy, sanitizedImpExt), :719-727 (mergeImpFPD)
    #[test]
    fn imp_first_party_data_merged_into_copy() -> TestResult {
        let banner = Arc::new(Banner { id: "b".to_owned() });
        let imp = Imp {
            id: "1".to_owned(),
            banner: Some(Arc::clone(&banner)),
            ext: Some(raw(r#"{"prebid":{"bidder":{"a":{}}}}"#)?),
        };
        let mut copy = imp.clone();
        let fpd = raw(r#"{"banner":{"id":"fpd"},"ext":{"data":{"k":1}}}"#)?;
        merge_imp_fpd(&mut copy, &fpd, 0)?;
        let merged = copy.banner.as_ref().map(|merged| merged.id.as_str());
        ensure_eq(&merged, &Some("fpd"), "the copy's banner")?;
        ensure_eq(&banner.id.as_str(), &"b", "the shared banner")?;
        let shared = imp
            .banner
            .as_ref()
            .is_some_and(|kept| Arc::ptr_eq(kept, &banner));
        ensure_eq(&shared, &true, "the imp's banner")?;
        ensure_eq(
            &text_of(copy.ext.as_deref()),
            &Some(r#"{"data":{"k":1},"prebid":{"bidder":{"a":{}}}}"#),
            "the merged ext",
        )?;

        let sanitized = BTreeMap::from([
            ("bidder".to_owned(), raw(r#"{ "placement" : 5 }"#)?),
            ("data".to_owned(), raw(r#"{"k" : 1}"#)?),
        ]);
        copy.ext = Some(marshal_raw(&sanitized)?);
        ensure_eq(
            &text_of(copy.ext.as_deref()),
            &Some(r#"{"bidder":{"placement":5},"data":{"k":1}}"#),
            "the sanitized ext",
        )?;

        let mut beyond = Imp {
            ext: Some(raw(r#"{"a":1e400}"#)?),
            ..Imp::default()
        };
        let ext_error = merge_imp_fpd(&mut beyond, &raw(r#"{"ext":{"b":1}}"#)?, 2)
            .err()
            .ok_or("merged into 1e400")?;
        ensure_eq(
            &(ext_error.message(), ext_error.code()),
            &("invalid imp ext for imp[2]", 999),
            "ext",
        )?;
        let mut mistyped = imp;
        let fpd_error = merge_imp_fpd(&mut mistyped, &raw(r#"{"banner":"x"}"#)?, 3)
            .err()
            .ok_or("merged a string into a banner")?;
        ensure_eq(
            &fpd_error.message(),
            &"invalid first party data for imp[3]",
            "first-party data",
        )
    }

    /// Go's `formatMergePatchError`: which input `MergePatch` rejected decides Prebid's error.
    fn format_merge_patch_error(error: MergePatchError) -> GoError {
        match error {
            MergePatchError::BadDoc => GoError::generic("invalid request ext"),
            MergePatchError::BadPatch => GoError::generic("invalid first party data ext"),
        }
    }

    /// Go's global first-party data in `resolveUser`: merged into the user's ext when it has one,
    /// taken as it is when not.
    fn merge_user_ext(ext: &mut Option<Box<RawValue>>, ext_data: &[u8]) -> Result<(), GoError> {
        let merged = match ext.as_deref() {
            Some(existing) => merge_patch(existing.get().as_bytes(), ext_data)
                .map_err(format_merge_patch_error)?,
            None => ext_data.to_vec(),
        };
        let parsed =
            serde_json::from_slice(&merged).map_err(|e| GoError::generic(e.to_string()))?;
        *ext = Some(parsed);
        Ok(())
    }

    /// The global first-party data merges into the user's ext by merge patch, or becomes it; a
    /// document or a patch the merge cannot read (numbers beyond `f64`'s range here,
    /// `json.merge-patch-limits`) gives Prebid's two errors.
    // Go: firstpartydata/first_party_data.go:250-262 (resolveUser), :336-346
    // (formatMergePatchError)
    #[test]
    fn first_party_data_ext_merges() -> TestResult {
        let mut ext = Some(raw(r#"{"a":1}"#)?);
        merge_user_ext(&mut ext, br#"{"data":{"k":"v"}}"#)?;
        ensure_eq(
            &text_of(ext.as_deref()),
            &Some(r#"{"a":1,"data":{"k":"v"}}"#),
            "merged",
        )?;
        let mut none = None;
        merge_user_ext(&mut none, br#"{"data":{"k":"v"}}"#)?;
        ensure_eq(
            &text_of(none.as_deref()),
            &Some(r#"{"data":{"k":"v"}}"#),
            "taken",
        )?;
        let mut beyond = Some(raw(r#"{"a":1e400}"#)?);
        let doc_error = merge_user_ext(&mut beyond, br#"{"data":{}}"#)
            .err()
            .ok_or("merged into 1e400")?;
        ensure_eq(&doc_error.message(), &"invalid request ext", "the document")?;
        let patch_error = merge_user_ext(&mut ext, br#"{"data":1e400}"#)
            .err()
            .ok_or("merged 1e400")?;
        ensure_eq(
            &patch_error.message(),
            &"invalid first party data ext",
            "the patch",
        )
    }

    /// `openrtb_ext.ExtUserPrebid`.
    #[derive(Debug, Default, Serialize, Deserialize)]
    #[serde(default)]
    struct ExtUserPrebid {
        #[serde(skip_serializing_if = "is_nil_or_empty")]
        buyeruids: Option<BTreeMap<String, String>>,
    }

    /// Go's `jsonEmptyObjectLength`.
    const JSON_EMPTY_OBJECT_LENGTH: usize = 2;

    /// Go's `UserExt`: the ext's members as raw values, `None` for a member that is null, and the
    /// members Prebid reads.
    #[derive(Debug, Default)]
    struct UserExt {
        ext: BTreeMap<String, Option<Box<RawValue>>>,
        consent: Option<String>,
        prebid: Option<ExtUserPrebid>,
        prebid_dirty: bool,
    }

    impl UserExt {
        /// Go's `unmarshal`, the consent and prebid members. A null ext reads as empty here; Go
        /// leaves the map nil (jsoniter's `mapDecoder`), which `unmarshal::<Option<_>>` tells
        /// apart for the port.
        fn unmarshal(&mut self, ext: Option<&RawValue>) -> Result<(), GoError> {
            let Some(json) = ext else {
                return Ok(());
            };
            self.ext = unmarshal::<Option<_>>(json.get().as_bytes())?.unwrap_or_default();
            if let Some(Some(consent)) = self.ext.get("consent") {
                self.consent = unmarshal(consent.get().as_bytes())?;
            }
            if let Some(member) = self.ext.get("prebid") {
                let prebid = self.prebid.insert(ExtUserPrebid::default());
                if let Some(prebid_json) = member {
                    *prebid = unmarshal(prebid_json.get().as_bytes())?;
                }
            }
            Ok(())
        }

        /// Go's `marshal`, the prebid member.
        fn marshal(&mut self) -> Result<Option<Box<RawValue>>, GoError> {
            if self.prebid_dirty {
                match self.prebid.as_ref().map(marshal_raw).transpose()? {
                    Some(prebid) if prebid.get().len() > JSON_EMPTY_OBJECT_LENGTH => {
                        let _previous = self.ext.insert("prebid".to_owned(), Some(prebid));
                    }
                    Some(_) | None => {
                        let _previous = self.ext.remove("prebid");
                    }
                }
                self.prebid_dirty = false;
            }
            if self.ext.is_empty() {
                return Ok(None);
            }
            marshal_raw(&self.ext).map(Some)
        }
    }

    /// The user ext reads into raw members, a null one kept as `None`; a prebid member that is
    /// null still makes `prebid` (Go: `&ExtUserPrebid{}`); written back, a prebid that marshals to
    /// `{}` is dropped, a set one written, and the members Prebid did not touch are compacted as
    /// `jsonutil.Marshal` writes them, Go's bytes for the decoded map of `raw_containers`.
    // Go: openrtb_ext/request_wrapper.go:465-613 (UserExt.unmarshal, UserExt.marshal);
    // gen/decode.go rawContainers
    #[test]
    fn user_ext_round_trip() -> TestResult {
        let doc = golden("decode.json")?;
        let decoded = list(&doc, "/raw_containers")?
            .iter()
            .find(|row| text(row, "/name").is_ok_and(|name| name == "decoded map"))
            .ok_or("no decoded map row")?;
        let mut untouched = UserExt::default();
        untouched.unmarshal(Some(&raw(text(decoded, "/input")?)?))?;
        ensure_eq(&untouched.consent.as_deref(), &Some("c o"), "Go's consent")?;
        let rewritten = untouched.marshal()?;
        ensure_eq(
            &text_of(rewritten.as_deref()),
            &Some(text(decoded, "/result")?),
            "Go's bytes",
        )?;

        let input = raw(r#"{"consent": "CO" , "prebid": null, "eids": [ {"source" : "s"} ]}"#)?;
        let mut user = UserExt::default();
        user.unmarshal(Some(&input))?;
        ensure_eq(&user.consent.as_deref(), &Some("CO"), "consent")?;
        ensure_eq(
            &user.ext.get("prebid").map(Option::is_none),
            &Some(true),
            "nil prebid",
        )?;
        ensure_eq(&user.prebid.is_some(), &true, "prebid made")?;
        user.prebid_dirty = true;
        let without_prebid = user.marshal()?;
        ensure_eq(
            &text_of(without_prebid.as_deref()),
            &Some(r#"{"consent":"CO","eids":[{"source":"s"}]}"#),
            "an empty prebid dropped",
        )?;
        user.prebid = Some(ExtUserPrebid {
            buyeruids: Some(BTreeMap::from([("x".to_owned(), "y".to_owned())])),
        });
        user.prebid_dirty = true;
        let with_prebid = user.marshal()?;
        ensure_eq(
            &text_of(with_prebid.as_deref()),
            &Some(r#"{"consent":"CO","eids":[{"source":"s"}],"prebid":{"buyeruids":{"x":"y"}}}"#),
            "a prebid written",
        )
    }

    /// The regs ext's `gpc` reads a string or any other JSON value as its text, and a null member
    /// leaves it unset.
    // Go: openrtb_ext/request_wrapper.go:1279-1282 (RegExt.unmarshal, gpc)
    #[test]
    fn regs_ext_gpc_read_as_string() -> TestResult {
        let mut read = Vec::new();
        for input in [
            br#"{"gpc": "1"}"#.as_slice(),
            br#"{"gpc": 1}"#,
            br#"{"gpc": null}"#,
        ] {
            let ext: BTreeMap<String, Option<Box<RawValue>>> = unmarshal(input)?;
            let mut gpc = None;
            if let Some(Some(json)) = ext.get("gpc") {
                parse_into_string(json.get().as_bytes(), Some(&mut gpc))?;
            }
            read.push(gpc);
        }
        ensure_eq(
            &read,
            &vec![Some("1".to_owned()), Some("1".to_owned()), None],
            "gpc",
        )
    }

    /// Go's `NotFoundError` text.
    fn not_found(data_type: &str, id: &str) -> String {
        format!("Stored {data_type} with ID=\"{id}\" not found.")
    }

    /// Go's `responseContract`: a null entry is a nil `json.RawMessage`.
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct ResponseContract {
        #[serde(deserialize_with = "null_default")]
        requests: BTreeMap<String, Option<Box<RawValue>>>,
        #[serde(deserialize_with = "null_default")]
        imps: BTreeMap<String, Option<Box<RawValue>>>,
    }

    /// Go's `convertNullsToErrs`: each null entry dropped and reported as not found.
    fn convert_nulls_to_errs(
        data: &mut BTreeMap<String, Option<Box<RawValue>>>,
        data_type: &str,
        errs: &mut Vec<String>,
    ) {
        data.retain(|id, value| {
            let found = value.is_some();
            if !found {
                errs.push(not_found(data_type, id));
            }
            found
        });
    }

    /// The HTTP fetcher reads the requests and imps of its response as raw values, a null one as
    /// `None`, which becomes a not-found error; a null map is an empty one.
    // Go: stored_requests/backends/http_fetcher/fetcher.go:278-317 (unpackResponse,
    // convertNullsToErrs)
    #[test]
    fn http_fetcher_nulls_not_found() -> TestResult {
        let body = br#"{"requests":{"a":{"id":"a"},"b":null},"imps":null}"#;
        let mut contract: ResponseContract = unmarshal_valid(body)?;
        let mut errs = Vec::new();
        convert_nulls_to_errs(&mut contract.requests, "Request", &mut errs);
        convert_nulls_to_errs(&mut contract.imps, "Imp", &mut errs);
        let requests: Vec<(&str, &str)> = contract
            .requests
            .iter()
            .filter_map(|(id, json)| Some((id.as_str(), json.as_deref()?.get())))
            .collect();
        ensure_eq(&requests, &vec![("a", r#"{"id":"a"}"#)], "requests")?;
        ensure_eq(&contract.imps.is_empty(), &true, "imps")?;
        ensure_eq(&errs, &vec![not_found("Request", "b")], "errors")
    }

    /// A stored request merges under the request; the HTTP fetcher returns the plain Go error
    /// evanphx gives for an account it cannot merge (code 999, its text), as the auction endpoint
    /// does for a default request or a stored imp neither of whose documents has a syntax error;
    /// and the resolved imps, the request's own bytes among them, are written compacted.
    // Go: endpoints/openrtb2/auction.go:1722 (MergePatch), :1799-1803 (Marshal(resolvedImps));
    // stored_requests/backends/http_fetcher/fetcher.go:193-196 (FetchAccount)
    #[test]
    fn stored_request_merges() -> TestResult {
        let resolved = merge_patch(br#"{"id":"stored","tmax":500}"#, br#"{"tmax":1000}"#)?;
        ensure_eq(
            &resolved.as_slice(),
            &br#"{"id":"stored","tmax":1000}"#.as_slice(),
            "resolved",
        )?;
        let error = merge_patch(br#"{"disabled":false}"#, br#"{"id":"acc"#)
            .map_err(GoError::from)
            .err()
            .ok_or("merged an account that is not JSON")?;
        ensure_eq(
            &(error.message(), error.code(), error.is_fatal()),
            &("Invalid JSON Patch", 999, true),
            "a plain error",
        )?;
        let imps = vec![raw(r#"{ "id" : "1" }"#)?, raw(r#"{"id":"2","tagid":"t"}"#)?];
        ensure_eq(
            &marshal(&imps)?.as_slice(),
            &br#"[{"id":"1"},{"id":"2","tagid":"t"}]"#.as_slice(),
            "resolved imps",
        )
    }

    /// Go's `UIDEntry`, its uid.
    #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(default)]
    struct UidEntry {
        #[serde(deserialize_with = "null_default")]
        uid: String,
    }

    /// Go's `cookieJson`.
    #[derive(Debug, Default, Serialize, Deserialize)]
    #[serde(default)]
    struct CookieJson {
        #[serde(rename = "tempUIDs", skip_serializing_if = "is_nil_or_empty")]
        uids: Option<BTreeMap<String, UidEntry>>,
        #[serde(skip_serializing_if = "is_zero", deserialize_with = "null_default")]
        optout: bool,
    }

    /// Go's `Cookie`, written and read through `cookieJson`.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Cookie {
        uids: BTreeMap<String, UidEntry>,
        opt_out: bool,
    }

    impl Serialize for Cookie {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            CookieJson {
                uids: Some(self.uids.clone()),
                optout: self.opt_out,
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Cookie {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let contract = CookieJson::deserialize(deserializer)?;
            let mut uids = if contract.optout {
                BTreeMap::new()
            } else {
                contract.uids.unwrap_or_default()
            };
            if uids
                .get("audienceNetwork")
                .is_some_and(|entry| entry.uid == "0")
            {
                let _dropped = uids.remove("audienceNetwork");
            }
            Ok(Self {
                uids,
                opt_out: contract.optout,
            })
        }
    }

    /// The cookie reads through its JSON contract, the Audience Network's `0` dropped and the uids
    /// of an opted-out cookie cleared; it writes without empty members; and a value that is not a
    /// cookie gives a new one.
    // Go: usersync/cookie.go:223-260 (cookieJson, MarshalJSON, UnmarshalJSON), decoder.go:15-27
    #[test]
    fn cookie_round_trip() -> TestResult {
        let read: Cookie = unmarshal_valid(
            br#"{"tempUIDs":{"audienceNetwork":{"uid":"0"},"adnxs":{"uid":"123"}},"optout":false}"#,
        )?;
        let adnxs = UidEntry {
            uid: "123".to_owned(),
        };
        let want = BTreeMap::from([("adnxs".to_owned(), adnxs)]);
        ensure_eq(&read.uids, &want, "uids")?;
        ensure_eq(
            &marshal(&read)?.as_slice(),
            &br#"{"tempUIDs":{"adnxs":{"uid":"123"}}}"#.as_slice(),
            "written",
        )?;
        let opted: Cookie = unmarshal_valid(br#"{"tempUIDs":{"a":{"uid":"1"}},"optout":true}"#)?;
        ensure_eq(
            &marshal(&opted)?.as_slice(),
            &br#"{"optout":true}"#.as_slice(),
            "opt-out",
        )?;
        let broken = unmarshal_valid::<Cookie>(b"{\"tempUIDs\":").unwrap_or_default();
        ensure_eq(&broken, &Cookie::default(), "a new cookie")
    }

    /// The bidder error `DropElement`'s error becomes, or the panic Go recovers from in
    /// `recoverSafely`.
    fn bidder_error(error: ElementError) -> Result<GoError, String> {
        match error {
            ElementError::Decode(text) => Ok(GoError::generic(text)),
            ElementError::Panic(text) => Err(text),
        }
    }

    /// The Audience Network body has `consented_providers_settings` cut out with its comma, the
    /// rest of the bytes as they were; a body the walk cannot read gives a bidder error.
    // Go: adapters/audienceNetwork/facebook.go:101-105; util/jsonutil/jsonutil_test.go:73-79,
    // 170-176 (TestDropElement)
    #[test]
    fn audience_network_body_dropped() -> TestResult {
        let body = concat!(
            r#"{"consent": "TESTCONSENT","consented_providers_settings": "#,
            r#"{"consented_providers": [1608,765,492], "test": 1}}"#,
        );
        let dropped = drop_element(body.as_bytes(), &["consented_providers_settings"])?;
        ensure_eq(
            &dropped.as_slice(),
            &br#"{"consent": "TESTCONSENT"}"#.as_slice(),
            "dropped",
        )?;
        let malformed = concat!(
            r#"{consented_providers_settings: "#,
            r#"{"consented_providers": [1365,5678,1545,2563,1411], "test": 1}}"#,
        );
        let failure = drop_element(malformed.as_bytes(), &["consented_providers"])
            .err()
            .ok_or("dropped from a malformed body")?;
        let error = bidder_error(failure)?;
        ensure_eq(&error.code(), &999, "a plain bidder error")
    }

    /// The WURFL module's `config`, the field the test uses.
    #[derive(Debug, Default, Deserialize)]
    #[serde(default)]
    struct WurflConfig {
        #[serde(deserialize_with = "null_default")]
        wurfl_file_path: String,
    }

    /// Go's `newConfig`, before `validate`.
    fn new_config(data: Option<&RawValue>) -> Result<WurflConfig, GoError> {
        let bytes = data.map_or(b"".as_slice(), |json| json.get().as_bytes());
        unmarshal_valid(bytes).map_err(|e| GoError::generic(format!("failed to parse config: {e}")))
    }

    /// A module's config is its raw `json.RawMessage` read into the module's type; a missing
    /// config or one of the wrong type fails with the module's wrapping text.
    // Go: modules/scientiamobile/wurfl_devicedetection/config.go:18-25 (newConfig)
    #[test]
    fn module_config_decoded() -> TestResult {
        let config = new_config(Some(&raw(r#"{"wurfl_file_path":"/w.zip","other":1}"#)?))?;
        ensure_eq(&config.wurfl_file_path.as_str(), &"/w.zip", "path")?;
        for data in [None, Some(raw(r#"{"wurfl_file_path":1}"#)?)] {
            let error = new_config(data.as_deref()).err().ok_or("decoded")?;
            ensure_eq(
                &error.message().starts_with("failed to parse config: "),
                &true,
                "text",
            )?;
        }
        Ok(())
    }

    /// A boxed future, as the exchange's `hold_auction` and the hooks return one.
    type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// Go's `MergeClone(v any, data)` with the value as a trait object, inside a boxed future.
    fn merge_later<'a>(
        value: &'a mut (dyn MergeClone + Send),
        data: &'a [u8],
    ) -> BoxFuture<'a, Result<(), GoError>> {
        Box::pin(async move { merge_clone(value, data) })
    }

    /// Whether `T` may cross threads and live in a `'static` future.
    const fn send_sync<T: Send + Sync + 'static>() {}

    /// `MergeClone` is dyn compatible and merges inside a `Send` future, and every error and value
    /// type the crate returns may cross threads, so the units' `BoxFuture`s can return them.
    // Go: util/jsonutil/merge.go:31-40 (MergeClone takes any)
    #[test]
    fn async_boundaries() -> TestResult {
        send_sync::<GoError>();
        send_sync::<MergeCloneError>();
        send_sync::<MergePatchError>();
        send_sync::<ElementError>();
        send_sync::<ElementSpan>();
        send_sync::<StringInt>();
        send_sync::<IntString>();
        send_sync::<Box<RawValue>>();
        let mut banner = Banner::default();
        let data = br#"{"id":"x"}"#;
        let mut future = merge_later(&mut banner, data);
        let mut context = Context::from_waker(Waker::noop());
        let Poll::Ready(merged) = future.as_mut().poll(&mut context) else {
            return Err("pending".into());
        };
        merged?;
        drop(future);
        ensure_eq(&banner.id.as_str(), &"x", "merged")
    }
}
