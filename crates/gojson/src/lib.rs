//! Go-compatible JSON for the Prebid Server Rust port.
//!
//! Prebid Server's Go code reaches JSON through three libraries: json-iterator (behind
//! `util/jsonutil`), `encoding/json`'s `Marshal` and `Unmarshal`, and a `json.Encoder` with
//! `SetEscapeHTML(false)`. Their output and their error texts differ in many small ways, and
//! those bytes reach bidders, publishers and the adapter test fixtures. This crate reproduces
//! each library exactly. Go's behaviour depends on reflection (type and field names, struct
//! tags, pointers, nil slices), so every Rust type declares the Go type it stands for, and
//! [`#[derive(GoJson)]`](GoJson) turns that declaration into Go-faithful encoders and decoders.
//!
//! Parity is tested against Go itself: `testdata/go` is a Go program, built against this
//! repository's Go code, that writes the goldens under `testdata/golden`, and `tests/golden.rs`
//! replays them byte for byte (see [Testing against Go](#testing-against-go)).
//!
//! # Call what the Go code calls
//!
//! | Go | Rust |
//! |---|---|
//! | `jsonutil.Unmarshal(data, &v)` | [`jsonutil::unmarshal`] |
//! | `jsonutil.UnmarshalValid(data, &v)` | [`jsonutil::unmarshal_valid`] |
//! | `jsonutil.Marshal(v)` | [`jsonutil::marshal`] |
//! | `json.Unmarshal(data, &v)` | [`stdjson::unmarshal`] |
//! | `json.Marshal(v)` | [`stdjson::marshal`] |
//! | `NewEncoder(w)` + `SetEscapeHTML(false)` + `Encode(v)` | [`stdjson::encode_no_escape_html`] |
//! | `json.NewEncoder(w).Encode(v)` | [`stdjson::marshal`] followed by `b'\n'` |
//! | `json.Compact(&buf, src)`, `json.Valid(data)` | [`stdjson::compact`], [`stdjson::valid`] |
//! | jsoniter's own error text | [`jsoniter::unmarshal`] |
//! | `json.NewDecoder(r).Decode(&v)` | [`stream::decode_std`] (or a [`stream::Decoder`]) |
//! | jsoniter's `NewDecoder(r).Decode(&v)` | [`stream::decode_jsoniter`] |
//! | `jsonpatch.MergePatch(doc, patch)` | [`mergepatch::merge_patch`] |
//! | `jsonpatch.Equal(a, b)` | [`mergepatch::equal`] |
//! | `jsonparser.Get`, `GetString`, `Set`, `Delete`, `ArrayEach`, ... | [`jsonparser`] |
//! | `jsonutil.FindElement(ext, names...)` | [`jsonutil::find_element`] |
//! | `jsonutil.DropElement(ext, names...)` | [`jsonutil::drop_element`] |
//! | `jsonutil.MergeClone(v, data)` | [`jsonutil::merge_clone`] |
//!
//! `mergepatch` is the root package of `gopkg.in/evanphx/json-patch.v5` v5.9.0, the map-based
//! one: a merged document comes out compacted, with object keys sorted, deduplicated and
//! HTML-escaped as `json.Marshal` writes them.
//!
//! The functions are not interchangeable. For the same value:
//!
//! * `jsonutil.Marshal` writes floats the jsoniter way (`1e-07`), control characters as `\u0008`, a
//!   `json.RawMessage` compacted and a nil or invalid one as *nothing* (`{"ext":}`), and its errors
//!   name the failing field (`openrtb2.Bid.Price: unsupported value: NaN`);
//! * `json.Marshal` writes `1e-7`, `\b`, a nil `json.RawMessage` as `null`, rejects an invalid one,
//!   and stops at the first error (`json: unsupported value: NaN`);
//! * both write `<`, `>` and `&` as `\u003c`, `\u003e` and `\u0026`. The `json.Encoder` with
//!   `SetEscapeHTML(false)` writes what `json.Marshal` writes, except that it leaves those three
//!   characters as they are, and it appends a newline.
//!
//! The decoders differ as much: jsoniter accepts some invalid input, decodes `null` into a
//! `json.RawMessage` as nil (`encoding/json` keeps the four bytes), matches field names a little
//! differently, and words every error differently.
//!
//! # Declaring Go types
//!
//! ## The type mapping
//!
//! | Go | Rust |
//! |---|---|
//! | `bool`, `string` | `bool`; `String` or [`GoString`] |
//! | `int`, `int8` … `int64` | `isize`, `i8` … `i64` |
//! | `uint`, `uint8` … `uint64` | `usize`, `u8` … `u64` |
//! | `float32`, `float64` | `f32`, `f64` |
//! | `*T` | `Option<T>`, or `Option<Box<T>>` |
//! | `[]T`, `map[K]V` | `Option<Vec<T>>`, `Option<BTreeMap<K, V>>` (`None` is nil) |
//! | `[]T`, `map[K]V` that are never nil | `Vec<T>`, `BTreeMap<K, V>` or `HashMap<K, V>` |
//! | `[]byte` (base64 in JSON) | `Option<Vec<u8>>` |
//! | `json.RawMessage` | [`RawJson`] |
//! | `json.Number` | [`JsonNumber`] |
//! | `interface{}` | `serde_json::Value` |
//! | `map[string]interface{}` | `Option<BTreeMap<String, serde_json::Value>>` |
//! | a struct type | a `#[derive(GoJson)]` struct |
//! | a named type (`type BidType string`) | a `#[derive(GoJson)]` single-field tuple struct |
//!
//! Go distinguishes a nil slice or map (`null`) from an empty one (`[]`, `{}`), and so does
//! `Option<Vec<T>>`; a plain `Vec<T>` is never nil, so it encodes `[]` and decoding `null`
//! empties it. Use `Option` for every slice and map that can be nil when it is encoded, which
//! is nearly all of them. Map keys are strings, integers, or named types marked `map_key`.
//!
//! Use `String` for ordinary strings and [`GoString`] where the exact output bytes matter. Go
//! strings are byte strings: jsoniter keeps invalid UTF-8 from its input, and both encoders later
//! write each invalid byte as the six characters `\ufffd`. A `String` can only hold the
//! replacement character itself, which is written as its three UTF-8 bytes: the same JSON string,
//! but different bytes. (`encoding/json` replaces invalid UTF-8 while decoding, so after
//! [`stdjson::unmarshal`] the two agree.)
//!
//! ## Structs
//!
//! ```
//! use pbs_gojson::{GoJson, RawJson, jsonutil};
//!
//! // Go:
//! //
//! //     type ExtImpExample struct {
//! //         PlacementID string          `json:"placementId"`
//! //         Floor       float64         `json:"floor,omitempty"`
//! //         Sizes       []int           `json:"sizes,omitempty"`
//! //         Ext         json.RawMessage `json:"ext,omitempty"`
//! //     }
//! #[derive(Clone, Debug, Default, GoJson)]
//! #[gojson(go_type = "openrtb_ext.ExtImpExample")]
//! pub struct ExtImpExample {
//!     #[gojson(go = "PlacementID", json = "placementId")]
//!     pub placement_id: String,
//!     #[gojson(json = "floor,omitempty")]
//!     pub floor: f64,
//!     #[gojson(json = "sizes,omitempty")]
//!     pub sizes: Option<Vec<isize>>,
//!     #[gojson(json = "ext,omitempty")]
//!     pub ext: RawJson,
//! }
//!
//! # fn check(got: &[u8], want: &[u8]) -> Result<(), String> {
//! #     if got == want { Ok(()) } else { Err(String::from_utf8_lossy(got).into_owned()) }
//! # }
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut ext = ExtImpExample::default();
//! jsonutil::unmarshal(
//!     br#"{"placementId": "p1", "sizes": [300, 250], "ext": {"a" : 1}}"#,
//!     &mut ext,
//! )?;
//! // The raw message holds the input bytes exactly; jsonutil.Marshal compacts it.
//! # check(ext.ext.as_bytes(), br#"{"a" : 1}"#)?;
//! let out = jsonutil::marshal(&ext)?;
//! // {"placementId":"p1","sizes":[300,250],"ext":{"a":1}}
//! # check(&out, br#"{"placementId":"p1","sizes":[300,250],"ext":{"a":1}}"#)?;
//!
//! let err = jsonutil::unmarshal(br#"{"placementId": 5}"#, &mut ext).err();
//! // cannot unmarshal openrtb_ext.ExtImpExample.PlacementID: expects " or n, but found 5
//! # let text = err.map(|e| e.message.into_bytes()).unwrap_or_default();
//! # let want = concat!(
//! #     "cannot unmarshal openrtb_ext.ExtImpExample.PlacementID: ",
//! #     "expects \" or n, but found 5",
//! # );
//! # check(&text, want.as_bytes())?;
//! # Ok(())
//! # }
//! ```
//!
//! Container attributes, `#[gojson(...)]` on the type:
//!
//! * `go_type = "pkg.Name"` (required): Go's `reflect.Type.String()`, the package *name* and the
//!   type name (`openrtb2.Bid`, `openrtb_ext.ExtImpAppnexus`). Errors print it.
//! * `anonymous`: the Go type is an unnamed struct, as in `var ext struct { ... }`, a common
//!   adapter decode target; `go_type` is then the struct's `reflect.Type.String()`, tags included
//!   (see [Unnamed struct types](#unnamed-struct-types)).
//! * `marshaler`, `unmarshaler`: the Go type has a `MarshalJSON` (value receiver) or
//!   `UnmarshalJSON` method; implement [`GoMarshaler`] or [`GoUnmarshaler`].
//! * `map_key` (named types only): the type is used as a map key, as `openrtb_ext.BidderName` is in
//!   `map[openrtb_ext.BidderName][]ExtBidderMessage`.
//!
//! Field attributes, `#[gojson(...)]` on a field:
//!
//! * `go = "PlacementID"`: the Go field name. The default is the Rust name in `UpperCamelCase`
//!   (`placement_id` becomes `PlacementId`), so initialisms need it. jsoniter errors print it, and
//!   it is the JSON key of a field without a `json` tag.
//! * `json = "placementId,omitempty"`: the value of the field's `json:"..."` tag, verbatim; `-`,
//!   `omitempty` and `string` mean what they mean in Go. Leave it out if the Go field has no `json`
//!   tag.
//! * `embedded`: an embedded struct field (`Base`, or `*Base` as `Option<Base>` or
//!   `Option<Box<Base>>`); its fields are promoted by Go's depth, tag and ambiguity rules.
//! * `skip`: a Rust-only field that JSON never sees.
//!
//! Declare the fields in Go's order: it is the output order, and it decides between duplicate
//! names. Declare an unexported Go field with its lower-case Go name (`go = "secret"`), or skip
//! it.
//!
//! ## Unnamed struct types
//!
//! Go code often decodes into a struct type that has no name. Its `reflect.Type.String()` spells
//! out every field and tag, and Go prints it in errors, so `go_type` must match it exactly; a
//! Rust string can continue it across lines. jsoniter's decode errors then leave the type out,
//! `encoding/json` names the struct as empty (`Go struct field .b`), and jsoniter's encode errors
//! print the whole `struct { ... }` text.
//!
//! ```
//! use pbs_gojson::{GoJson, jsonutil, stdjson};
//!
//! // var ext struct {
//! //     A int      `json:"a"`
//! //     B []string `json:"b,omitempty"`
//! // }
//! #[derive(Clone, Debug, Default, GoJson)]
//! #[gojson(
//!     go_type = "struct { A int \"json:\\\"a\\\"\"; \
//!                B []string \"json:\\\"b,omitempty\\\"\" }",
//!     anonymous
//! )]
//! pub struct Ext {
//!     #[gojson(json = "a")]
//!     pub a: isize,
//!     #[gojson(json = "b,omitempty")]
//!     pub b: Option<Vec<String>>,
//! }
//!
//! # fn check(got: &[u8], want: &[u8]) -> Result<(), String> {
//! #     if got == want { Ok(()) } else { Err(String::from_utf8_lossy(got).into_owned()) }
//! # }
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut ext = Ext::default();
//! let err = jsonutil::unmarshal(br#"{"b":"x"}"#, &mut ext).err();
//! // cannot unmarshal B: decode slice: expect [ or n, but found "
//! # let text = err.map(|e| e.message.into_bytes()).unwrap_or_default();
//! # check(&text, br#"cannot unmarshal B: decode slice: expect [ or n, but found ""#)?;
//! let err = stdjson::unmarshal(br#"{"b":"x"}"#, &mut ext).err();
//! // json: cannot unmarshal string into Go struct field .b of type []string
//! # let text = err.map(|e| e.go_text()).unwrap_or_default();
//! # check(&text, b"json: cannot unmarshal string into Go struct field .b of type []string")?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Named types and embedded structs
//!
//! A single-field tuple struct is a Go named type. It encodes and decodes as its underlying
//! type, but errors use its own name, and `marshaler`/`unmarshaler` apply to it alone.
//!
//! ```
//! use std::collections::BTreeMap;
//!
//! use pbs_gojson::{GoJson, stdjson};
//!
//! // type BidderName string
//! #[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, GoJson)]
//! #[gojson(go_type = "openrtb_ext.BidderName", map_key)]
//! pub struct BidderName(pub String);
//!
//! // type Base struct {
//! //     ID string `json:"id"`
//! // }
//! #[derive(Clone, Debug, Default, GoJson)]
//! #[gojson(go_type = "example.Base")]
//! pub struct Base {
//!     #[gojson(go = "ID", json = "id")]
//!     pub id: String,
//! }
//!
//! // type Response struct {
//! //     Base
//! //     Times map[BidderName]int `json:"times,omitempty"`
//! // }
//! #[derive(Clone, Debug, Default, GoJson)]
//! #[gojson(go_type = "example.Response")]
//! pub struct Response {
//!     #[gojson(embedded)]
//!     pub base: Base,
//!     #[gojson(json = "times,omitempty")]
//!     pub times: Option<BTreeMap<BidderName, isize>>,
//! }
//!
//! # fn check(got: &[u8], want: &[u8]) -> Result<(), String> {
//! #     if got == want { Ok(()) } else { Err(String::from_utf8_lossy(got).into_owned()) }
//! # }
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let response = Response {
//!     base: Base {
//!         id: "r1".to_owned(),
//!     },
//!     times: Some(BTreeMap::from([(BidderName("appnexus".to_owned()), 12)])),
//! };
//! let out = stdjson::marshal(&response)?;
//! // {"id":"r1","times":{"appnexus":12}}
//! # check(&out, br#"{"id":"r1","times":{"appnexus":12}}"#)?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Custom `MarshalJSON` and `UnmarshalJSON`
//!
//! ```
//! use pbs_gojson::{GoJson, GoMarshaler, GoUnmarshaler, JsonError, jsonutil, stdjson};
//!
//! // type Flag bool, with methods that write and accept 1 and 0.
//! #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, GoJson)]
//! #[gojson(go_type = "example.Flag", marshaler, unmarshaler)]
//! pub struct Flag(pub bool);
//!
//! impl GoMarshaler for Flag {
//!     fn marshal_json(&self) -> Result<Vec<u8>, JsonError> {
//!         Ok(if self.0 { b"1".to_vec() } else { b"0".to_vec() })
//!     }
//! }
//!
//! impl GoUnmarshaler for Flag {
//!     fn unmarshal_json(&mut self, data: &[u8]) -> Result<(), JsonError> {
//!         match data {
//!             b"1" => self.0 = true,
//!             b"0" => self.0 = false,
//!             _ => return Err(JsonError::msg("flag must be 0 or 1")),
//!         }
//!         Ok(())
//!     }
//! }
//!
//! # fn check(got: &[u8], want: &[u8]) -> Result<(), String> {
//! #     if got == want { Ok(()) } else { Err(String::from_utf8_lossy(got).into_owned()) }
//! # }
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut flag = Flag::default();
//! jsonutil::unmarshal(b"1", &mut flag)?;
//! let out = stdjson::marshal(&flag)?;
//! // 1
//! # check(&out, b"1")?;
//! let err = jsonutil::unmarshal(b"2", &mut flag).err();
//! // flag must be 0 or 1
//! # check(&err.map(|e| e.message.into_bytes()).unwrap_or_default(), b"flag must be 0 or 1")?;
//! let err = stdjson::unmarshal(b"2", &mut flag).err();
//! // flag must be 0 or 1
//! # check(&err.map(|e| e.go_text()).unwrap_or_default(), b"flag must be 0 or 1")?;
//! # Ok(())
//! # }
//! ```
//!
//! Both decoders hand the method the value's raw bytes, `null` included, except that `null` into a
//! pointer (`Option<Flag>`) sets it to nil without calling the method. `encoding/json` compacts
//! what `MarshalJSON` returns and rejects invalid JSON with a `MarshalerError`; jsoniter writes it
//! as it is, minus one trailing newline.
//!
//! # Decoding and encoding
//!
//! ```
//! use pbs_gojson::{GoJson, RawJson, jsonutil, stdjson};
//!
//! #[derive(Clone, Debug, Default, GoJson)]
//! #[gojson(go_type = "example.Creative")]
//! pub struct Creative {
//!     #[gojson(json = "adm")]
//!     pub adm: String,
//!     #[gojson(json = "price")]
//!     pub price: f64,
//!     #[gojson(json = "ext")]
//!     pub ext: RawJson,
//! }
//!
//! # fn check(got: &[u8], want: &[u8]) -> Result<(), String> {
//! #     if got == want { Ok(()) } else { Err(String::from_utf8_lossy(got).into_owned()) }
//! # }
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut creative = Creative::default();
//! jsonutil::unmarshal(br#"{"adm": "<b>", "price": 1e-7, "ext": null}"#, &mut creative)?;
//! // jsoniter decodes null into a nil json.RawMessage...
//! # check(&[u8::from(creative.ext.is_nil())], &[1])?;
//! let mut std_creative = Creative::default();
//! stdjson::unmarshal(br#"{"adm": "<b>", "price": 1e-7, "ext": null}"#, &mut std_creative)?;
//! // ...while encoding/json keeps the bytes `null`.
//! # check(std_creative.ext.as_bytes(), b"null")?;
//!
//! // jsoniter floats, and a nil json.RawMessage written as nothing (as Go does):
//! // {"adm":"\u003cb\u003e","price":1e-07,"ext":}
//! # check(&jsonutil::marshal(&creative)?, br#"{"adm":"\u003cb\u003e","price":1e-07,"ext":}"#)?;
//! // {"adm":"\u003cb\u003e","price":1e-7,"ext":null}
//! # check(&stdjson::marshal(&creative)?, br#"{"adm":"\u003cb\u003e","price":1e-7,"ext":null}"#)?;
//! // {"adm":"<b>","price":1e-7,"ext":null} and a newline
//! # let out = stdjson::encode_no_escape_html(&creative)?;
//! # check(&out, b"{\"adm\":\"<b>\",\"price\":1e-7,\"ext\":null}\n")?;
//!
//! let err = jsonutil::unmarshal(br#"{"price": "1"}"#, &mut creative).err();
//! // cannot unmarshal example.Creative.Price: invalid number
//! # let text = err.map(|e| e.message.into_bytes()).unwrap_or_default();
//! # check(&text, b"cannot unmarshal example.Creative.Price: invalid number")?;
//! let err = stdjson::unmarshal(br#"{"price": "1"}"#, &mut creative).err();
//! // json: cannot unmarshal string into Go struct field Creative.price of type float64
//! # let text = err.map(|e| e.go_text()).unwrap_or_default();
//! # check(
//! #     &text,
//! #     b"json: cannot unmarshal string into Go struct field Creative.price of type float64",
//! # )?;
//! # Ok(())
//! # }
//! ```
//!
//! # `json.RawMessage`: [`RawJson`]
//!
//! [`RawJson`] holds a JSON value's exact bytes and distinguishes nil (an absent field) from
//! present bytes; cloning it is cheap. Its helpers reproduce the Go idioms around
//! `json.RawMessage`, each through `jsonutil` as the Go code usually does:
//!
//! ```
//! use pbs_gojson::RawJson;
//!
//! # fn check(got: &[u8], want: &[u8]) -> Result<(), String> {
//! #     if got == want { Ok(()) } else { Err(String::from_utf8_lossy(got).into_owned()) }
//! # }
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let ext = RawJson::from(r#"{"prebid": {"is_rewarded_inventory": 1}, "bidder": {"zone": "z"}}"#);
//!
//! // var m map[string]json.RawMessage; jsonutil.Unmarshal(ext, &m); m["bidder"]
//! let bidder = ext.get("bidder")?.unwrap_or_default();
//! // {"zone": "z"}
//! # check(bidder.as_bytes(), br#"{"zone": "z"}"#)?;
//!
//! // m["bidder"] = updated; ext, err = jsonutil.Marshal(m): sorted keys, compacted values
//! let edited = ext.with_key("bidder", RawJson::from(r#"{"zone": "y"}"#))?;
//! // {"bidder":{"zone":"y"},"prebid":{"is_rewarded_inventory":1}}
//! # check(edited.as_bytes(), br#"{"bidder":{"zone":"y"},"prebid":{"is_rewarded_inventory":1}}"#)?;
//!
//! // delete(m, "prebid")
//! let without = ext.without_key("prebid")?;
//! // {"bidder":{"zone":"z"}}
//! # check(without.as_bytes(), br#"{"bidder":{"zone":"z"}}"#)?;
//!
//! // var v interface{}; jsonutil.Unmarshal(ext, &v), and jsonutil.Marshal(v) back
//! let value = ext.to_value()?;
//! let back = RawJson::from_value(&value);
//! // {"bidder":{"zone":"z"},"prebid":{"is_rewarded_inventory":1}}
//! # check(back.as_bytes(), br#"{"bidder":{"zone":"z"},"prebid":{"is_rewarded_inventory":1}}"#)?;
//! # Ok(())
//! # }
//! ```
//!
//! [`RawJson::marshal`] and [`RawJson::unmarshal`] are `jsonutil.Marshal` and
//! `jsonutil.Unmarshal` for typed values. When the Go code uses `json.Marshal` instead, call
//! [`stdjson::marshal`] and wrap the bytes with [`RawJson::from_vec`].
//!
//! # `json.Number`: [`JsonNumber`]
//!
//! jsoniter stores whatever string it is given in a `json.Number`; `json.Marshal` then rejects
//! it. Several adapters depend on exactly this:
//!
//! ```
//! use pbs_gojson::{GoJson, JsonNumber, jsonutil, stdjson};
//!
//! // openrtb_ext.ExtImpAdtarget
//! #[derive(Clone, Debug, Default, GoJson)]
//! #[gojson(go_type = "openrtb_ext.ExtImpAdtarget")]
//! pub struct ExtImpAdtarget {
//!     #[gojson(go = "SourceId", json = "aid")]
//!     pub source_id: JsonNumber,
//!     #[gojson(go = "PlacementId", json = "placementId,omitempty")]
//!     pub placement_id: isize,
//!     #[gojson(go = "SiteId", json = "siteId,omitempty")]
//!     pub site_id: isize,
//!     #[gojson(go = "BidFloor", json = "bidFloor,omitempty")]
//!     pub bid_floor: f64,
//! }
//!
//! # fn check(got: &[u8], want: &[u8]) -> Result<(), String> {
//! #     if got == want { Ok(()) } else { Err(String::from_utf8_lossy(got).into_owned()) }
//! # }
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut ext = ExtImpAdtarget::default();
//! jsonutil::unmarshal(br#"{"aid": "some string instead of int"}"#, &mut ext)?;
//! let err = stdjson::marshal(&ext).err();
//! // json: invalid number literal "some string instead of int"
//! # let text = err.map(|e| e.go_text()).unwrap_or_default();
//! # check(&text, br#"json: invalid number literal "some string instead of int""#)?;
//! let err = ext.source_id.int64().err();
//! // strconv.ParseInt: parsing "some string instead of int": invalid syntax
//! # let text = err.map(|e| e.go_text()).unwrap_or_default();
//! # check(&text, br#"strconv.ParseInt: parsing "some string instead of int": invalid syntax"#)?;
//! # Ok(())
//! # }
//! ```
//!
//! # `jsonutil`'s helper types
//!
//! [`jsonutil::StringInt`] (`jsonutil.StringInt`: an `int` that also accepts a quoted number),
//! [`jsonutil::IntString`] (`jsonutil.IntString`: a `string` that also accepts a bare number) and
//! [`jsonutil::parse_into_string`] (`jsonutil.ParseIntoString`) behave as their Go
//! counterparts, error texts included.
//!
//! # Errors
//!
//! Every error carries the bytes of Go's `err.Error()`:
//!
//! * [`jsonutil::unmarshal`] returns [`jsonutil::FailedToUnmarshal`] and [`jsonutil::marshal`]
//!   returns [`jsonutil::FailedToMarshal`] (Go's `errortypes` values), whose `message` is a
//!   [`GoString`];
//! * everything else returns [`JsonError`], whose [`JsonError::go_text`] is the text.
//!
//! The texts can contain invalid UTF-8: jsoniter copies input bytes into its messages, and its
//! integer decoders report any unexpected character as the byte 0xFF. `Display` replaces such
//! bytes with U+FFFD; where Go formats an error into a message, use the exact bytes. Go's texts
//! for some inputs:
//!
//! * `jsonutil.Unmarshal` of `{"seatbid":{}}` into an `openrtb2.BidResponse`: `cannot unmarshal
//!   openrtb2.BidResponse.SeatBid: decode slice: expect [ or n, but found {`; of `"x"`: `expect {
//!   or n, but found "`;
//! * jsoniter itself ([`jsoniter::unmarshal`]) for `{"w":"x"}` into an `openrtb2.Bid`:
//!   `openrtb2.Bid.W: readUint64: unexpected character: \xff, error found in #6 byte of
//!   ...|{"w":"x"}|..., bigger context ...|{"w":"x"}|...`, which `jsonutil.Unmarshal` shortens to
//!   `cannot unmarshal openrtb2.Bid.W: unexpected character: \xff`;
//! * `json.Unmarshal` of `{"seatbid":[{"bid":[{"w":"1"}]}]}` into an `openrtb2.BidResponse`: `json:
//!   cannot unmarshal string into Go struct field Bid.seatbid.bid.w of type int64`; of `x`:
//!   `invalid character 'x' looking for beginning of value`;
//! * `jsonutil.Marshal` of an `openrtb2.Bid` with a `NaN` price: `openrtb2.Bid.Price: unsupported
//!   value: NaN`; `json.Marshal`: `json: unsupported value: NaN`;
//! * `json.Marshal` of `json.RawMessage("{bad}")`: `json: error calling MarshalJSON for type
//!   json.RawMessage: invalid character 'b' looking for beginning of object key string`.
//!
//! # Lower-level API
//!
//! [`encode::marshal_with`] encodes with an explicit [`Flavor`] through an [`Encoder`]. The
//! traits behind the derive, [`GoType`], [`GoEncode`], [`JsoniterDecode`] and [`StdDecode`],
//! can be implemented by hand for a type the derive cannot describe; their hidden methods are
//! internal hooks with working defaults.
//!
//! # Testing against Go
//!
//! When unsure what Go does, run Go. To add a differential case here:
//!
//! 1. add the Go type to `testdata/go/types.go` (or use a real one from this repository) and the
//!    inputs to the tables in `testdata/go/main.go`;
//! 2. from the repository root, run `go run ./rust/crates/gojson/testdata/go` (keep the program
//!    `gofmt` and `go vet` clean; it also rewrites `src/unicode/tables.rs`, the Unicode tables of
//!    the Go version that runs it);
//! 3. mirror the type in `tests/golden.rs` and register its Go name in `decode_by_name` or
//!    `encode_by_name`;
//! 4. run `cargo test -p pbs-gojson`.
//!
//! The rest of the toolkit (json-patch, jsonparser, the streaming decoders, `FindElement`,
//! `DropElement` and `MergeClone`) has its own generator: `go run
//! ./rust/crates/gojson/testdata/go/toolkit` writes `testdata/go/toolkit/*.txt` from the Go
//! packages' own test tables and seeded random corpora, and `tests/toolkit.rs` replays them.
//! `go run ./rust/crates/gojson/testdata/go/toolkit_review` writes adversarial corpora for the
//! same packages (escaped keys, every array index spelling, `Set`/`Delete` layouts, `Unescape`
//! over every surrogate pair class, `MergeClone` over every existing shape), which
//! `tests/toolkit_review.rs` replays.
//!
//! `go run ./rust/crates/gojson/testdata/go/mergeclone` writes `jsonutil.MergeClone` goldens
//! for real openrtb2 values (corpus `Imp`, `Site`, `App`, `User`, `Device` and `BidRequest`
//! objects merged with first-party data and generated overlays, some over a nil or malformed
//! `Ext`) and for a struct with every field shape, which `tests/mergeclone.rs` replays.
//!
//! Other crates can follow the same pattern with a generator under their own `testdata/go`.
//!
//! # Known differences from Go
//!
//! * `String` and `serde_json::Value` cannot hold invalid UTF-8 (use [`GoString`] or [`RawJson`]),
//!   and `Value` cannot hold `NaN` or infinities. Numbers decoded into `Value` are Go `float64`s;
//!   integers stored in a `Value` encode as Go integers.
//! * Where Go's result depends on its random map iteration order (jsoniter binding fields whose
//!   names differ only in case), the Rust result is one of Go's possible results, chosen
//!   deterministically.
//! * Not modelled: fixed-size arrays, `encoding.TextMarshaler` and `TextUnmarshaler`, `omitzero`,
//!   `MarshalJSON` with a pointer receiver, and interfaces with methods.
//! * Inputs on which Go panics (a lone `"` into `jsonutil.StringInt`, malformed messages in
//!   `tryExtractErrorMessage`) return an error or the message unchanged instead.
//! * [`RawJson::to_object`] uses `String` keys and treats nil and empty messages as an empty
//!   object, as Go code guarded by `len(ext) > 0` does.
//! * Where Go panics in jsonparser (an empty key where an array is, `Delete` on some malformed
//!   input), `jsonparser` takes the fallback documented on each function; `mergepatch::equal`
//!   returns `false` where Go's `Equal` panics on a `null` array element; `FindElement`'s panics
//!   come back as [`jsonutil::ElementError::Panic`] with Go's runtime error text.
//! * Go's `jsonparser.Set` can write into its input's spare capacity; [`jsonparser::set`] never
//!   changes its input.
//! * [`stream::decode_jsoniter`] reads the whole body as `jsoniter.Unmarshal` does. Go's `Decoder`
//!   reads through a 512-byte buffer and leaves its position in place at the end of the input, so
//!   where a decode runs into the end of the input (or the body is longer than 512 bytes) its error
//!   reports another position and context; both fail.
//! * [`jsonutil::merge_clone`] does not reset a slice- or map-kind type that has its own
//!   `UnmarshalJSON` ([`GoUnmarshaler`]): Go's `sliceCloneDecoder` sets such a slice to nil before
//!   calling the method, and both clone decoders set it to nil on `null` without calling it; here
//!   the method is called as `jsonutil.Unmarshal` calls it. Prebid's `MergeClone` targets (openrtb2
//!   values, `config.Account`) have no such field.
//! * Decoding into an existing slice never revives elements beyond its length. Go keeps the backing
//!   array: when a slice was shortened (a second decode with fewer elements) and is then decoded
//!   again with more elements within its capacity, Go decodes the extra elements over the old
//!   values left in the array (`encoding/json`'s `decodeState.array` grows the length with
//!   `SetLen`, jsoniter's `sliceDecoder` with `UnsafeGrow`), so fields absent from the new input
//!   keep the old values. Rust drops truncated elements, so they start from zero values. Only
//!   repeated decodes into the same value can tell the difference; `MergeClone` resets slices
//!   first, so it never can.
//! * Not provided: `json.MarshalIndent`, which only Go tests call.

#[doc(hidden)]
pub mod base64;
pub mod custom;
pub mod encode;
mod error;
mod gostring;
pub mod gotype;
pub mod jsoniter;
pub mod jsonparser;
pub mod jsonutil;
pub mod mergepatch;
mod number;
mod raw;
pub mod stdjson;
#[doc(hidden)]
pub mod strconv;
pub mod stream;
pub mod structs;
#[doc(hidden)]
pub mod unicode;

#[doc(hidden)]
pub mod __private;

pub use custom::{GoMarshaler, GoUnmarshaler};
pub use encode::{Encoder, Flavor, GoEncode, GoMapKey};
pub use error::JsonError;
pub use gostring::GoString;
pub use gotype::{GoKind, GoType};
pub use jsoniter::JsoniterDecode;
pub use number::JsonNumber;
pub use pbs_gojson_derive::GoJson;
pub use raw::RawJson;
pub use stdjson::StdDecode;
