//! Differential replay of the goldens that `testdata/go` writes with Go 1.26.2, yaml.v3 v3.0.1
//! and mapstructure v1.5.0: every value and error must match Go byte for byte.
//!
//! Values are compared through Go's `dump` rendering (`testdata/go/main.go`), which this file
//! reproduces from the Rust values; the structs below mirror the generator's Go types.
//!
//! Regenerate the goldens from the repository root with
//! `GOARCH=amd64 go run ./rust/crates/goyaml/testdata/go`.

#[cfg(test)]
mod golden {
    use std::{cmp::Ordering, collections::BTreeMap, fs, sync::Arc, thread};

    use pbs_gocompat::{
        math, strconv,
        time::{DateFields, Duration, Location, Time},
    };
    use pbs_gojson::{GoJson, GoKind, GoString, RawJson, jsonutil, stdjson};
    use pbs_goyaml::{
        GoMapstructure, GoYaml, Type, Value,
        mapstructure::{DecodeError, DecodeHook, Decoder, DecoderConfig, HookTarget, MsDecode},
        yaml::{self, StreamDecoder, UnmarshalError, YamlDecode},
    };

    // ---- Go's dump ----------------------------------------------------------------------------

    /// Go's `dump` of a value of the Go type the Rust type stands for.
    trait Dump {
        /// Whether `Option<Self>` stands for a nil-able `Self` (a slice or a map), not `*Self`.
        const NILABLE: bool = false;

        /// Appends the rendering.
        fn dump(&self, out: &mut String);

        /// Go: `reflect.Value.IsZero`.
        fn is_zero(&self) -> bool;
    }

    fn dump<T: Dump + ?Sized>(v: &T) -> String {
        let mut out = String::new();
        v.dump(&mut out);
        out
    }

    macro_rules! dump_integers {
        ($($ty:ty),*) => {$(
            impl Dump for $ty {
                fn dump(&self, out: &mut String) {
                    out.push_str(&self.to_string());
                }

                fn is_zero(&self) -> bool {
                    *self == 0
                }
            }
        )*};
    }

    dump_integers!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

    impl Dump for f64 {
        fn dump(&self, out: &mut String) {
            out.push_str(&strconv::format_float(*self, b'g', -1, 64));
        }

        fn is_zero(&self) -> bool {
            // Go: `v.Float() == 0` (-0 is zero, NaN is not).
            self.partial_cmp(&0.0) == Some(Ordering::Equal)
        }
    }

    impl Dump for f32 {
        fn dump(&self, out: &mut String) {
            out.push_str(&strconv::format_float(f64::from(*self), b'g', -1, 32));
        }

        fn is_zero(&self) -> bool {
            // Go: `v.Float() == 0` (-0 is zero, NaN is not).
            self.partial_cmp(&0.0) == Some(Ordering::Equal)
        }
    }

    impl Dump for bool {
        fn dump(&self, out: &mut String) {
            out.push_str(if *self { "true" } else { "false" });
        }

        fn is_zero(&self) -> bool {
            !*self
        }
    }

    impl Dump for String {
        fn dump(&self, out: &mut String) {
            out.push_str(&strconv::quote(self));
        }

        fn is_zero(&self) -> bool {
            self.is_empty()
        }
    }

    impl Dump for GoString {
        fn dump(&self, out: &mut String) {
            out.push_str(&strconv::quote_bytes(self.as_bytes()));
        }

        fn is_zero(&self) -> bool {
            self.as_bytes().is_empty()
        }
    }

    impl Dump for Duration {
        fn dump(&self, out: &mut String) {
            self.0.dump(out);
        }

        fn is_zero(&self) -> bool {
            self.0 == 0
        }
    }

    impl Dump for RawJson {
        fn dump(&self, out: &mut String) {
            // Go: a []byte-kind slice, as its bytes.
            if self.is_nil() {
                out.push_str("nil");
                return;
            }
            out.push('[');
            for (i, b) in self.as_bytes().iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&b.to_string());
            }
            out.push(']');
        }

        fn is_zero(&self) -> bool {
            self.is_nil()
        }
    }

    impl Dump for Time {
        fn dump(&self, out: &mut String) {
            out.push_str("time(");
            out.push_str(&self.to_string());
            out.push(')');
        }

        fn is_zero(&self) -> bool {
            *self == Self::default()
        }
    }

    impl Dump for Value {
        fn dump(&self, out: &mut String) {
            dump_value(Type::Interface, self, out);
        }

        fn is_zero(&self) -> bool {
            self.is_nil()
        }
    }

    impl<T: Dump> Dump for Option<T> {
        fn dump(&self, out: &mut String) {
            match self {
                None => out.push_str("nil"),
                Some(v) => {
                    if !T::NILABLE {
                        out.push('&');
                    }
                    v.dump(out);
                }
            }
        }

        fn is_zero(&self) -> bool {
            self.is_none()
        }
    }

    impl<T: Dump> Dump for Box<T> {
        const NILABLE: bool = T::NILABLE;

        fn dump(&self, out: &mut String) {
            (**self).dump(out);
        }

        fn is_zero(&self) -> bool {
            (**self).is_zero()
        }
    }

    impl<T: Dump> Dump for Vec<T> {
        const NILABLE: bool = true;

        fn dump(&self, out: &mut String) {
            out.push('[');
            for (i, item) in self.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                item.dump(out);
            }
            out.push(']');
        }

        fn is_zero(&self) -> bool {
            false
        }
    }

    impl<K: Dump, V: Dump> Dump for BTreeMap<K, V> {
        const NILABLE: bool = true;

        fn dump(&self, out: &mut String) {
            let entries = self
                .iter()
                .map(|(k, v)| {
                    let mut entry = dump(k);
                    entry.push_str(": ");
                    v.dump(&mut entry);
                    entry
                })
                .collect();
            push_map(Some(entries), out);
        }

        fn is_zero(&self) -> bool {
            false
        }
    }

    /// Go's rendering of a map from its rendered entries: sorted, or `nil`.
    fn push_map(entries: Option<Vec<String>>, out: &mut String) {
        let Some(mut entries) = entries else {
            out.push_str("nil");
            return;
        };
        entries.sort();
        out.push('{');
        out.push_str(&entries.join(", "));
        out.push('}');
    }

    /// Go's `dump` of `v` held in a value of the static type `ty`: an interface shows the
    /// dynamic type.
    fn dump_value(ty: Type, v: &Value, out: &mut String) {
        if ty == Type::Interface {
            let Some(dynamic) = v.type_of() else {
                out.push_str("nil");
                return;
            };
            out.push('<');
            dynamic.write_name(out);
            out.push('>');
            dump_value(dynamic, v, out);
            return;
        }
        match v {
            Value::Nil => out.push_str("nil"),
            Value::Bool(b) => b.dump(out),
            Value::Int(i) | Value::Int64(i) => i.dump(out),
            Value::Int8(i) => i.dump(out),
            Value::Int16(i) => i.dump(out),
            Value::Int32(i) => i.dump(out),
            Value::Uint(u) | Value::Uint64(u) => u.dump(out),
            Value::Uint8(u) => u.dump(out),
            Value::Uint16(u) => u.dump(out),
            Value::Uint32(u) => u.dump(out),
            Value::Float32(f) => f.dump(out),
            Value::Float64(f) => f.dump(out),
            Value::String(s) => s.dump(out),
            Value::Time(t) => t.dump(out),
            Value::Duration(d) => d.dump(out),
            Value::Slice(elem, items) => {
                let Some(items) = items else {
                    out.push_str("nil");
                    return;
                };
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    dump_value(**elem, item, out);
                }
                out.push(']');
            }
            Value::StringMap(elem, entries) => {
                let rendered = entries.as_ref().map(|m| {
                    m.iter()
                        .map(|(k, item)| {
                            let mut entry = strconv::quote(k);
                            entry.push_str(": ");
                            dump_value(**elem, item, &mut entry);
                            entry
                        })
                        .collect()
                });
                push_map(rendered, out);
            }
            Value::Map(key, elem, entries) => {
                let rendered = entries.as_ref().map(|m| {
                    m.iter()
                        .map(|(k, item)| {
                            let mut entry = String::new();
                            dump_value(**key, k, &mut entry);
                            entry.push_str(": ");
                            dump_value(**elem, item, &mut entry);
                            entry
                        })
                        .collect()
                });
                push_map(rendered, out);
            }
        }
    }

    /// Whether a Go field name is exported.
    fn exported(go_name: &str) -> bool {
        go_name.chars().next().is_some_and(char::is_uppercase)
    }

    /// Declares the mirror of a Go struct: the struct, deriving `GoYaml` and `GoMapstructure`
    /// (and `GoJson` after `json`), and its [`Dump`]. Each field gives its Go name and, when
    /// its `yaml` and `mapstructure` tags are the same plain key, that key.
    macro_rules! mirror {
        (json $(#[$meta:meta])* struct $name:ident {
            $($(#[$fmeta:meta])* $field:ident: $ty:ty => $go:literal $($tag:literal)?,)*
        }) => {
            mirror! {
                #[derive(GoJson)]
                $(#[$meta])*
                struct $name {
                    $($(#[$fmeta])* #[gojson(go = $go)] $field: $ty => $go $($tag)?,)*
                }
            }
        };
        ($(#[$meta:meta])* struct $name:ident {
            $($(#[$fmeta:meta])* $field:ident: $ty:ty => $go:literal $($tag:literal)?,)*
        }) => {
            #[derive(Debug, Default, GoYaml, GoMapstructure)]
            $(#[$meta])*
            struct $name {
                $(
                    $(#[$fmeta])*
                    #[goyaml(go = $go)]
                    $(#[goyaml(yaml = $tag, mapstructure = $tag)])?
                    $field: $ty,
                )*
            }

            impl Dump for $name {
                fn dump(&self, out: &mut String) {
                    let mut fields = Vec::new();
                    $(
                        if exported($go) && !self.$field.is_zero() {
                            let mut text = String::from($go);
                            text.push(':');
                            self.$field.dump(&mut text);
                            fields.push(text);
                        }
                    )*
                    out.push('{');
                    out.push_str(&fields.join(" "));
                    out.push('}');
                }

                fn is_zero(&self) -> bool {
                    true $(&& self.$field.is_zero())*
                }
            }
        };
    }

    /// Declares the mirror of a Go named type over another type.
    macro_rules! named {
        ($(#[$meta:meta])* struct $name:ident($inner:ty);) => {
            #[derive(Debug, Default, GoYaml, GoMapstructure)]
            $(#[$meta])*
            struct $name($inner);

            impl Dump for $name {
                fn dump(&self, out: &mut String) {
                    self.0.dump(out);
                }

                fn is_zero(&self) -> bool {
                    self.0.is_zero()
                }
            }
        };
    }

    // ---- the generator's Go types (types.go) --------------------------------------------------

    mirror! {
        #[goyaml(go_type = "main.Kitchen")]
        struct Kitchen {
            int: isize => "Int" "int",
            int8: i8 => "Int8" "int8",
            int16: i16 => "Int16" "int16",
            int32: i32 => "Int32" "int32",
            int64: i64 => "Int64" "int64",
            uint: usize => "Uint" "uint",
            uint8: u8 => "Uint8" "uint8",
            uint16: u16 => "Uint16" "uint16",
            uint32: u32 => "Uint32" "uint32",
            uint64: u64 => "Uint64" "uint64",
            float32: f32 => "Float32" "float32",
            float64: f64 => "Float64" "float64",
            bool: bool => "Bool" "bool",
            string: GoString => "String" "string",
            duration: Duration => "Duration" "duration",
            time: Time => "Time" "time",
            ptr_int: Option<isize> => "PtrInt" "ptrint",
            ptr_str: Option<GoString> => "PtrStr" "ptrstr",
            ptr_bool: Option<bool> => "PtrBool" "ptrbool",
            ptr_dur: Option<Duration> => "PtrDur" "ptrdur",
            ptr_ptr: Option<Option<isize>> => "PtrPtr" "ptrptr",
            ints: Option<Vec<isize>> => "Ints" "ints",
            strings: Option<Vec<String>> => "Strings" "strings",
            bytes: Option<Vec<u8>> => "Bytes" "bytes",
            any: Value => "Any" "any",
            anys: Option<Vec<Value>> => "Anys" "anys",
            str_map: Option<BTreeMap<String, String>> => "StrMap" "strmap",
            int_map: Option<BTreeMap<String, isize>> => "IntMap" "intmap",
            any_map: Option<BTreeMap<String, Value>> => "AnyMap" "anymap",
            key_map: Option<BTreeMap<isize, String>> => "KeyMap" "keymap",
            nested: Option<Box<Nested>> => "Nested" "nested",
            value: Nested => "Value" "value",
            nesteds: Option<Vec<Nested>> => "Nesteds" "nesteds",
            bid_type: BidType => "BidType" "bidtype",
            bid_types: Option<Vec<BidType>> => "BidTypes" "bidtypes",
            plain: String => "Plain",
            #[goyaml(yaml = ",inline", mapstructure = ",squash", embedded)]
            inline: Inline => "Inline",
            #[goyaml(yaml = ",inline", mapstructure = ",remain")]
            extra: Option<BTreeMap<String, Value>> => "Extra",
            #[goyaml(yaml = "-", mapstructure = "-")]
            skipped: String => "Skipped",
            hidden: String => "hidden",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.Nested")]
        struct Nested {
            a: isize => "A" "a",
            b: Option<Vec<String>> => "B" "b",
            ptr: Option<Box<Nested>> => "Ptr" "ptr",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.Inline")]
        struct Inline {
            in_a: String => "InA" "ina",
            in_b: Option<isize> => "InB" "inb",
        }
    }

    named! {
        #[goyaml(go_type = "main.BidType")]
        struct BidType(String);
    }

    mirror! {
        #[goyaml(go_type = "main.aliasNillableFields")]
        struct AliasNillableFields {
            disabled: Option<bool> => "Disabled" "disabled",
            modifying_vast: Option<bool> => "ModifyingVastXmlAllowed" "modifyingVastXmlAllowed",
            experiment: Option<Box<BidderInfoExperiment>> => "Experiment" "experiment",
            xapi: Option<Box<AdapterXapi>> => "XAPI" "xapi",
        }
    }

    // ---- config.AccountModules (config/account.go) --------------------------------------------

    /// Go: `config.AccountModules`.
    #[derive(Debug, Default, GoMapstructure)]
    #[goyaml(go_type = "config.AccountModules")]
    struct AccountModules(Option<BTreeMap<String, Option<BTreeMap<String, RawJson>>>>);

    impl Dump for AccountModules {
        fn dump(&self, out: &mut String) {
            self.0.dump(out);
        }

        fn is_zero(&self) -> bool {
            self.0.is_zero()
        }
    }

    /// Go: `modulesHolder` (types.go).
    #[derive(Debug, Default, GoMapstructure)]
    #[goyaml(go_type = "main.modulesHolder")]
    struct ModulesHolder {
        #[goyaml(go = "Modules")]
        #[goyaml(mapstructure = "modules")]
        modules: AccountModules,
    }

    impl Dump for ModulesHolder {
        fn dump(&self, out: &mut String) {
            out.push('{');
            if !self.modules.is_zero() {
                out.push_str("Modules:");
                self.modules.dump(out);
            }
            out.push('}');
        }

        fn is_zero(&self) -> bool {
            self.modules.is_zero()
        }
    }

    /// `json.RawMessage`'s underlying `[]byte`, as [`Value::bytes`] types it.
    const RAW_MESSAGE: Type = Type::Slice(&Type::Uint8);

    /// `map[string]json.RawMessage`.
    const RAW_MESSAGE_MAP: Type = Type::Map(&Type::String, &RAW_MESSAGE);

    /// The prefix of `AccountModulesHookFunc`'s errors.
    const CONVERT_MODULES: &str = "failed to convert account modules map";

    /// Prebid's `config.AccountModulesHookFunc` (`config/account_module_decoder.go`), as the
    /// config crate would port it. Go ranges over the maps in random order, so of several
    /// entries that fail it reports any one; this port goes in key order (the goldens have at
    /// most one). Data of another map type fails Go's `data.(map[string]interface{})`
    /// assertion, a panic that viper's maps never cause; it is an error here.
    fn account_modules_hook() -> DecodeHook {
        Arc::new(|to: HookTarget, data: &Value| {
            if data.kind() != Some(GoKind::Map) || to.type_name() != "config.AccountModules" {
                return Ok(None);
            }
            let Value::StringMap(Type::Interface, input) = data else {
                return Err(format!(
                    "interface conversion: interface {{}} is {}, not map[string]interface {{}}",
                    data.type_name()
                ));
            };
            let mut result = BTreeMap::new();
            for (outer_key, outer_value) in input.iter().flatten() {
                let inner = if outer_value.is_nil() {
                    None
                } else if let Value::StringMap(Type::Interface, inner) = outer_value {
                    let mut converted = BTreeMap::new();
                    for (inner_key, inner_value) in inner.iter().flatten() {
                        let raw = jsonutil::marshal(inner_value).map_err(|e| {
                            format!(
                                "{CONVERT_MODULES}: failed to marshal value for \
                                 {outer_key}.{inner_key}: {e}"
                            )
                        })?;
                        let _previous = converted.insert(inner_key.clone(), Value::bytes(&raw));
                    }
                    Some(converted)
                } else {
                    return Err(format!(
                        "{CONVERT_MODULES}: failed to convert inner map for {outer_key}"
                    ));
                };
                let _previous =
                    result.insert(outer_key.clone(), Value::StringMap(&RAW_MESSAGE, inner));
            }
            Ok(Some(Value::StringMap(&RAW_MESSAGE_MAP, Some(result))))
        })
    }

    // ---- config.BidderInfo (config/bidderinfo.go) ----------------------------------------------

    mirror! {
        json
        #[gojson(go_type = "config.BidderInfo")]
        struct BidderInfo {
            alias_of: String => "AliasOf" "aliasOf",
            white_label_only: bool => "WhiteLabelOnly" "whiteLabelOnly",
            disabled: bool => "Disabled" "disabled",
            endpoint: String => "Endpoint" "endpoint",
            extra_adapter_info: String => "ExtraAdapterInfo" "extra_info",
            open_rtb: Option<Box<OpenRtbInfo>> => "OpenRTB" "openrtb",
            maintainer: Option<Box<MaintainerInfo>> => "Maintainer" "maintainer",
            capabilities: Option<Box<CapabilitiesInfo>> => "Capabilities" "capabilities",
            modifying_vast_xml_allowed: bool => "ModifyingVastXmlAllowed" "modifyingVastXmlAllowed",
            debug: Option<Box<DebugInfo>> => "Debug" "debug",
            geoscope: Option<Vec<String>> => "Geoscope" "geoscope",
            gvl_vendor_id: u16 => "GVLVendorID" "gvlVendorID",
            syncer: Option<Box<Syncer>> => "Syncer" "userSync",
            experiment: BidderInfoExperiment => "Experiment" "experiment",
            xapi: AdapterXapi => "XAPI" "xapi",
            platform_id: String => "PlatformID" "platform_id",
            app_secret: String => "AppSecret" "app_secret",
            endpoint_compression: String => "EndpointCompression" "endpointCompression",
        }
    }

    mirror! {
        json
        #[gojson(go_type = "config.BidderInfoExperiment")]
        struct BidderInfoExperiment {
            ads_cert: BidderAdsCert => "AdsCert" "adsCert",
        }
    }

    mirror! {
        json
        #[gojson(go_type = "config.BidderAdsCert")]
        struct BidderAdsCert {
            enabled: bool => "Enabled" "enabled",
        }
    }

    mirror! {
        json
        #[gojson(go_type = "config.MaintainerInfo")]
        struct MaintainerInfo {
            email: String => "Email" "email",
        }
    }

    mirror! {
        json
        #[gojson(go_type = "config.CapabilitiesInfo")]
        struct CapabilitiesInfo {
            app: Option<Box<PlatformInfo>> => "App" "app",
            site: Option<Box<PlatformInfo>> => "Site" "site",
            dooh: Option<Box<PlatformInfo>> => "DOOH" "dooh",
        }
    }

    mirror! {
        json
        #[gojson(go_type = "config.PlatformInfo")]
        struct PlatformInfo {
            media_types: Option<Vec<MediaType>> => "MediaTypes" "mediaTypes",
        }
    }

    named! {
        #[derive(GoJson)]
        #[gojson(go_type = "openrtb_ext.BidType")]
        struct MediaType(String);
    }

    mirror! {
        json
        #[gojson(go_type = "config.DebugInfo")]
        struct DebugInfo {
            allow: bool => "Allow" "allow",
        }
    }

    mirror! {
        json
        #[gojson(go_type = "config.AdapterXAPI")]
        struct AdapterXapi {
            username: String => "Username" "username",
            password: String => "Password" "password",
            tracker: String => "Tracker" "tracker",
        }
    }

    mirror! {
        json
        #[gojson(go_type = "config.OpenRTBInfo")]
        struct OpenRtbInfo {
            version: String => "Version" "version",
            gpp_supported: bool => "GPPSupported" "gpp-supported",
            multiformat_supported: Option<bool> => "MultiformatSupported" "multiformat-supported",
        }
    }

    mirror! {
        json
        #[gojson(go_type = "config.Syncer")]
        struct Syncer {
            key: String => "Key" "key",
            supports: Option<Vec<String>> => "Supports" "supports",
            iframe: Option<Box<SyncerEndpoint>> => "IFrame" "iframe",
            redirect: Option<Box<SyncerEndpoint>> => "Redirect" "redirect",
            #[goyaml(yaml = "externalUrl", mapstructure = "external_url")]
            external_url: String => "ExternalURL",
            #[goyaml(yaml = "formatOverride", mapstructure = "format_override")]
            format_override: String => "FormatOverride",
            enabled: Option<bool> => "Enabled" "enabled",
            skip_when: Option<Box<SkipWhen>> => "SkipWhen" "skipwhen",
        }
    }

    mirror! {
        json
        #[gojson(go_type = "config.SkipWhen")]
        struct SkipWhen {
            gdpr: bool => "GDPR" "gdpr",
            gpp_sid: Option<Vec<String>> => "GPPSID" "gpp_sid",
        }
    }

    mirror! {
        json
        #[gojson(go_type = "config.SyncerEndpoint")]
        struct SyncerEndpoint {
            url: String => "URL" "url",
            #[goyaml(yaml = "redirectUrl", mapstructure = "redirect_url")]
            redirect_url: String => "RedirectURL",
            #[goyaml(yaml = "externalUrl", mapstructure = "external_url")]
            external_url: String => "ExternalURL",
            #[goyaml(yaml = "userMacro", mapstructure = "user_macro")]
            user_macro: String => "UserMacro",
        }
    }

    // ---- the mapstructure v1.5.0 test types (types.go) ----------------------------------------

    mirror! {
        #[goyaml(go_type = "main.Basic")]
        struct Basic {
            vstring: String => "Vstring",
            vint: isize => "Vint",
            vint8: i8 => "Vint8",
            vint16: i16 => "Vint16",
            vint32: i32 => "Vint32",
            vint64: i64 => "Vint64",
            vuint: usize => "Vuint",
            vbool: bool => "Vbool",
            vfloat: f64 => "Vfloat",
            vextra: String => "Vextra",
            vsilent: bool => "vsilent",
            vdata: Value => "Vdata",
            vjson_int: isize => "VjsonInt",
            vjson_uint: usize => "VjsonUint",
            vjson_uint64: u64 => "VjsonUint64",
            vjson_float: f64 => "VjsonFloat",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.BasicPointer")]
        struct BasicPointer {
            vstring: Option<String> => "Vstring",
            vint: Option<isize> => "Vint",
            vuint: Option<usize> => "Vuint",
            vbool: Option<bool> => "Vbool",
            vfloat: Option<f64> => "Vfloat",
            vextra: Option<String> => "Vextra",
            vsilent: Option<bool> => "vsilent",
            vdata: Option<Value> => "Vdata",
            vjson_int: Option<isize> => "VjsonInt",
            vjson_float: Option<f64> => "VjsonFloat",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.BasicSquash")]
        struct BasicSquash {
            #[goyaml(mapstructure = ",squash")]
            test: Basic => "Test",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.Embedded")]
        struct Embedded {
            #[goyaml(embedded)]
            basic: Basic => "Basic",
            vunique: String => "Vunique",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.EmbeddedPointer")]
        struct EmbeddedPointer {
            #[goyaml(embedded)]
            basic: Option<Box<Basic>> => "Basic",
            vunique: String => "Vunique",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.EmbeddedSquash")]
        struct EmbeddedSquash {
            #[goyaml(embedded, mapstructure = ",squash")]
            basic: Basic => "Basic",
            vunique: String => "Vunique",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.EmbeddedAndNamed")]
        struct EmbeddedAndNamed {
            #[goyaml(embedded)]
            basic: Basic => "Basic",
            named: Basic => "Named",
            vunique: String => "Vunique",
        }
    }

    named! {
        #[goyaml(go_type = "main.SliceAlias")]
        struct SliceAlias(Option<Vec<String>>);
    }

    mirror! {
        #[goyaml(go_type = "main.EmbeddedSlice")]
        struct EmbeddedSlice {
            #[goyaml(embedded, mapstructure = "slice_alias")]
            slice_alias: SliceAlias => "SliceAlias",
            vunique: String => "Vunique",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.Map")]
        struct Map {
            vfoo: String => "Vfoo",
            vother: Option<BTreeMap<String, String>> => "Vother",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.MapOfStruct")]
        struct MapOfStruct {
            value: Option<BTreeMap<String, Basic>> => "Value",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.NestedBasic")]
        struct NestedBasic {
            vfoo: String => "Vfoo",
            vbar: Basic => "Vbar",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.NestedPointer")]
        struct NestedPointer {
            vfoo: String => "Vfoo",
            vbar: Option<Box<Basic>> => "Vbar",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.Slice")]
        struct Slice {
            vfoo: String => "Vfoo",
            vbar: Option<Vec<String>> => "Vbar",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.SliceOfAlias")]
        struct SliceOfAlias {
            vfoo: String => "Vfoo",
            vbar: SliceAlias => "Vbar",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.SliceOfStruct")]
        struct SliceOfStruct {
            value: Option<Vec<Basic>> => "Value",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.SlicePointer")]
        struct SlicePointer {
            vbar: Option<Option<Vec<String>>> => "Vbar",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.Tagged")]
        struct Tagged {
            #[goyaml(mapstructure = "bar,what,what")]
            extra: String => "Extra",
            #[goyaml(mapstructure = "foo")]
            value: String => "Value",
        }
    }

    mirror! {
        #[goyaml(go_type = "main.Remainder")]
        struct Remainder {
            a: String => "A",
            #[goyaml(mapstructure = ",remain")]
            extra: Option<BTreeMap<String, Value>> => "Extra",
        }
    }

    named! {
        /// Go's `bool`, as a named Rust type: the four `bool` fields of `TypeConversionResult`
        /// decode through the newtype path.
        #[goyaml(go_type = "bool")]
        struct GoBool(bool);
    }

    mirror! {
        #[goyaml(go_type = "main.TypeConversionResult")]
        struct TypeConversionResult {
            int_to_float: f32 => "IntToFloat",
            int_to_uint: usize => "IntToUint",
            int_to_bool: GoBool => "IntToBool",
            int_to_string: String => "IntToString",
            uint_to_int: isize => "UintToInt",
            uint_to_float: f32 => "UintToFloat",
            uint_to_bool: GoBool => "UintToBool",
            uint_to_string: String => "UintToString",
            bool_to_int: isize => "BoolToInt",
            bool_to_uint: usize => "BoolToUint",
            bool_to_float: f32 => "BoolToFloat",
            bool_to_string: String => "BoolToString",
            float_to_int: isize => "FloatToInt",
            float_to_uint: usize => "FloatToUint",
            float_to_bool: GoBool => "FloatToBool",
            float_to_string: String => "FloatToString",
            slice_uint8_to_string: String => "SliceUint8ToString",
            string_to_slice_uint8: Option<Vec<u8>> => "StringToSliceUint8",
            string_to_int: isize => "StringToInt",
            string_to_uint: usize => "StringToUint",
            string_to_bool: GoBool => "StringToBool",
            string_to_float: f32 => "StringToFloat",
            string_to_str_slice: Option<Vec<String>> => "StringToStrSlice",
            string_to_int_slice: Option<Vec<isize>> => "StringToIntSlice",
            slice_to_map: Option<BTreeMap<String, Value>> => "SliceToMap",
            map_to_slice: Option<Vec<Value>> => "MapToSlice",
        }
    }

    // ---- golden files -------------------------------------------------------------------------

    /// The rows of a golden file (the header skipped), each a list of unquoted fields.
    fn rows(name: &str) -> Result<Vec<Vec<Vec<u8>>>, String> {
        let path = format!("{}/testdata/golden/{name}", env!("CARGO_MANIFEST_DIR"));
        let text = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
        text.lines()
            .enumerate()
            .filter(|(_, line)| !line.starts_with('#') && !line.is_empty())
            .map(|(n, line)| {
                line.split('\t')
                    .map(|field| {
                        strconv::unquote_bytes(field)
                            .map_err(|e| format!("{name} line {}: {e}", n + 1))
                    })
                    .collect()
            })
            .collect()
    }

    fn col(row: &[Vec<u8>], index: usize) -> Result<&[u8], String> {
        row.get(index)
            .map(Vec::as_slice)
            .ok_or_else(|| format!("golden row has {} fields, wanted {}", row.len(), index + 1))
    }

    fn lossy(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// A Go-quoted rendering, cut short for failure reports.
    fn shown(bytes: &[u8]) -> String {
        const MAX: usize = 400;
        let quoted = strconv::quote_to_ascii_bytes(bytes);
        if quoted.len() <= MAX {
            return quoted;
        }
        let mut cut: String = quoted.chars().take(MAX).collect();
        cut.push_str("...");
        cut
    }

    /// The outputs that differ from Go by design, as (golden file, case, column, reason). Each
    /// must still differ, so that the list stays accurate.
    const KNOWN_GAPS: [(&str, &str, &str, &str); 6] = [
        (
            "corpus.txt",
            "yaml.v3/308",
            "interface{} value",
            "!!binary data that is not UTF-8: Value::String holds a Rust String (the bytes are \
             kept when decoding into GoString)",
        ),
        (
            "corpus.txt",
            "yaml.v3/308",
            "jsonutil.Marshal(interface{})",
            "the same: the encoders write the replacement characters, not \\ufffd escapes",
        ),
        (
            "corpus.txt",
            "yaml.v3/308",
            "json.Marshal(interface{})",
            "the same",
        ),
        (
            "corpus.txt",
            "yaml.v3/308",
            "Kitchen value",
            "the same, in the ,inline map",
        ),
        (
            "mapstructure.txt",
            "kitchen/prefilled",
            "strict value",
            "decoding a map into a non-nil interface{} holding a map replaces it; Go decodes \
             into the same map, merging",
        ),
        (
            "mapstructure.txt",
            "kitchen/prefilled",
            "viper value",
            "the same",
        ),
    ];

    /// The mismatches of one golden file.
    struct Report {
        file: &'static str,
        outputs: usize,
        failures: Vec<String>,
        gaps_seen: usize,
    }

    impl Report {
        const fn new(file: &'static str) -> Self {
            Self {
                file,
                outputs: 0,
                failures: Vec::new(),
                gaps_seen: 0,
            }
        }

        /// Compares Rust's outputs for `case` with the Go outputs of `row`, starting at
        /// column `first`.
        fn check(
            &mut self,
            case: &str,
            what: &str,
            row: &[Vec<u8>],
            first: usize,
            outputs: &[(&str, &[u8])],
        ) -> Result<(), String> {
            for (i, (name, got)) in outputs.iter().enumerate() {
                let want = col(row, first + i)?;
                self.outputs += 1;
                let gap = KNOWN_GAPS
                    .iter()
                    .any(|&(file, c, column, _)| file == self.file && c == case && column == *name);
                if gap {
                    if want == *got {
                        return Err(format!("{what}: the known gap in {name} now matches Go"));
                    }
                    self.gaps_seen += 1;
                } else if want != *got {
                    self.failures.push(format!(
                        "{what}\n    {name}:\n      go:   {}\n      rust: {}",
                        shown(want),
                        shown(got)
                    ));
                }
            }
            Ok(())
        }

        fn finish(self) -> Result<(), String> {
            let gaps = KNOWN_GAPS.iter().filter(|gap| gap.0 == self.file).count();
            if self.gaps_seen != gaps {
                return Err(format!(
                    "{}: {} of the {gaps} known gaps found",
                    self.file, self.gaps_seen
                ));
            }
            if self.failures.is_empty() {
                return Ok(());
            }
            let mut report = format!(
                "{}: {} of {} outputs differ from Go\n",
                self.file,
                self.failures.len(),
                self.outputs
            );
            for failure in self.failures.iter().take(40) {
                report.push_str(failure);
                report.push('\n');
            }
            Err(report)
        }
    }

    /// Runs `f` on a thread with a large stack: decoding is recursive, and the limit cases nest
    /// 10000 levels deep.
    fn on_big_stack(f: fn() -> Result<(), String>) -> Result<(), String> {
        thread::Builder::new()
            .stack_size(512 << 20)
            .spawn(f)
            .map_err(|e| e.to_string())?
            .join()
            .map_err(|_panic| "the replay thread panicked".to_owned())?
    }

    // ---- yaml.v3 ------------------------------------------------------------------------------

    /// A decoded value and its error, as the generator writes them (Go's `outcome`): a panic
    /// leaves no value.
    struct Outcome {
        value: Vec<u8>,
        error: Vec<u8>,
        panicked: bool,
    }

    fn panic_text(text: &str) -> Vec<u8> {
        format!("panic: {text}").into_bytes()
    }

    fn yaml_outcome<T: Dump>(out: &T, result: Result<(), UnmarshalError>) -> Outcome {
        match result {
            Ok(()) => Outcome {
                value: dump(out).into_bytes(),
                error: Vec::new(),
                panicked: false,
            },
            Err(UnmarshalError::Panic(text)) => Outcome {
                value: Vec::new(),
                error: panic_text(&text),
                panicked: true,
            },
            Err(e) => Outcome {
                value: dump(out).into_bytes(),
                error: e.text(),
                panicked: false,
            },
        }
    }

    /// Go: `yamlInto`: `yaml.Unmarshal` into a new `T`.
    fn yaml_into<T: YamlDecode + Dump + Default>(input: &[u8]) -> (T, Outcome) {
        let mut out = T::default();
        let result = yaml::unmarshal(input, &mut out);
        let outcome = yaml_outcome(&out, result);
        (out, outcome)
    }

    /// Go: `yamlKnown`: the error of a `KnownFields` decoder into a new `config.BidderInfo`.
    fn yaml_known(input: &[u8]) -> Vec<u8> {
        let mut decoder = StreamDecoder::new(input);
        decoder.known_fields(true);
        match decoder.decode(&mut BidderInfo::default()) {
            Ok(true) => Vec::new(),
            Ok(false) => b"EOF".to_vec(),
            Err(UnmarshalError::Panic(text)) => panic_text(&text),
            Err(e) => e.text(),
        }
    }

    /// Go: `yamlStream`: the documents of a stream decoded into `interface{}` one by one.
    fn yaml_stream(input: &[u8], first: &[u8]) -> Vec<u8> {
        const MAX_DOCS: usize = 16;
        let mut decoder = StreamDecoder::new(input);
        let mut lines: Vec<Vec<u8>> = Vec::new();
        for i in 0..MAX_DOCS {
            let mut v = Value::Nil;
            let result = match decoder.decode(&mut v) {
                Ok(false) => {
                    lines.push(b"EOF".to_vec());
                    break;
                }
                Ok(true) => Ok(()),
                Err(e) => Err(e),
            };
            let outcome = yaml_outcome(&v, result);
            let mut line = outcome.value;
            line.extend_from_slice(b" | ");
            line.extend_from_slice(&outcome.error);
            if i == 0 && line == first {
                line = b"=".to_vec();
            }
            lines.push(line);
            if outcome.panicked {
                break;
            }
        }
        lines.join(&b'\n')
    }

    /// The text that ends a Rust encoding error standing for jsoniter's nil-key panic.
    const NIL_KEY_PANIC: &[u8] =
        b"runtime error: invalid memory address or nil pointer dereference";

    /// Go: `marshalJSON`: the JSON, `error: ` and the error, or `panic: ` and the panic (the
    /// nil map key jsoniter panics on, which the Rust encoder reports as an error).
    fn json_outcome(result: Result<Vec<u8>, Vec<u8>>) -> Vec<u8> {
        match result {
            Ok(json) => json,
            Err(text) if text.ends_with(NIL_KEY_PANIC) => [b"panic: ", NIL_KEY_PANIC].concat(),
            Err(text) => [b"error: ".as_slice(), &text].concat(),
        }
    }

    /// `jsonutil.Marshal` and `json.Marshal` of an `interface{}` tree, compared with the Go
    /// outputs in columns `first` and `first + 1`: where Go's output depends on its random map
    /// iteration, the golden lists every outcome (`one of:`), and Rust's must be one of them.
    fn marshal_tree(
        tree: &Value,
        row: &[Vec<u8>],
        first: usize,
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        let jsonutil_out =
            json_outcome(jsonutil::marshal(tree).map_err(|e| e.message.as_bytes().to_vec()));
        let json_out = json_outcome(stdjson::marshal(tree).map_err(|e| e.go_text()));
        Ok((
            settle(col(row, first)?, jsonutil_out),
            settle(col(row, first + 1)?, json_out),
        ))
    }

    /// `got`, or Go's `one of:` list when `got` is one of its outcomes.
    fn settle(want: &[u8], got: Vec<u8>) -> Vec<u8> {
        match want.strip_prefix(b"one of:\n".as_slice()) {
            Some(list) if list.split(|&b| b == b'\n').any(|outcome| outcome == got) => {
                want.to_vec()
            }
            Some(_) | None => got,
        }
    }

    fn replay_files() -> Result<(), String> {
        let mut report = Report::new("files.txt");
        let viper = DecoderConfig::viper();
        for row in rows("files.txt")? {
            let path = lossy(col(&row, 0)?);
            let input = col(&row, 1)?;
            let (tree, tree_out) = yaml_into::<Value>(input);
            let (info, info_out) = yaml_into::<BidderInfo>(input);
            let json = stdjson::marshal(&info).unwrap_or_else(|e| {
                let mut text = b"!".to_vec();
                text.extend_from_slice(&e.go_text());
                text
            });
            let (_, alias_out) = yaml_into::<AliasNillableFields>(input);
            let known = yaml_known(input);
            let (ms_value, ms_error) = if tree_out.panicked {
                (Vec::new(), Vec::new())
            } else {
                ms_outcome::<BidderInfo>(&viper, None, &tree)
            };
            let (jsonutil_out, json_out) = marshal_tree(&tree, &row, 12)?;
            report.check(
                &path,
                &format!("file {path}"),
                &row,
                2,
                &[
                    ("interface{} value", &tree_out.value),
                    ("interface{} error", &tree_out.error),
                    ("config.BidderInfo value", &info_out.value),
                    ("config.BidderInfo error", &info_out.error),
                    ("json.Marshal", &json),
                    ("aliasNillableFields value", &alias_out.value),
                    ("aliasNillableFields error", &alias_out.error),
                    ("BidderInfo KnownFields error", &known),
                    ("BidderInfo via mapstructure value", &ms_value),
                    ("BidderInfo via mapstructure error", &ms_error),
                    ("jsonutil.Marshal(interface{})", &jsonutil_out),
                    ("json.Marshal(interface{})", &json_out),
                ],
            )?;
        }
        report.finish()
    }

    fn replay_corpus() -> Result<(), String> {
        let mut report = Report::new("corpus.txt");
        for row in rows("corpus.txt")? {
            let name = lossy(col(&row, 0)?);
            let input = col(&row, 1)?;
            let (tree_value, tree) = yaml_into::<Value>(input);
            let (_, kitchen) = yaml_into::<Kitchen>(input);
            let mut first = tree.value.clone();
            first.extend_from_slice(b" | ");
            first.extend_from_slice(&tree.error);
            let stream = yaml_stream(input, &first);
            let (_, info) = yaml_into::<BidderInfo>(input);
            let (jsonutil_out, json_out) = marshal_tree(&tree_value, &row, 9)?;
            report.check(
                &name,
                &format!("corpus {name}: input {}", shown(input)),
                &row,
                2,
                &[
                    ("interface{} value", &tree.value),
                    ("interface{} error", &tree.error),
                    ("Kitchen value", &kitchen.value),
                    ("Kitchen error", &kitchen.error),
                    ("stream", &stream),
                    ("config.BidderInfo value", &info.value),
                    ("config.BidderInfo error", &info.error),
                    ("jsonutil.Marshal(interface{})", &jsonutil_out),
                    ("json.Marshal(interface{})", &json_out),
                ],
            )?;
        }
        report.finish()
    }

    #[test]
    fn files_golden() -> Result<(), String> {
        on_big_stack(replay_files)
    }

    #[test]
    fn corpus_golden() -> Result<(), String> {
        on_big_stack(replay_corpus)
    }

    /// yaml.v3's `TestLimits` (`limit_test.go`): its inputs and expected errors.
    fn limits() -> Result<(), String> {
        let maps = |count: usize| format!("a: &a [{{a}}{}]", ",{a}".repeat(count)).into_bytes();
        let cases: [(&str, Vec<u8>, &str); 12] = [
            (
                "1000kb of maps with 100 aliases",
                format!(
                    "{{a: &a [{{a}}{}], b: &b [*a{}]}}",
                    ",{a}".repeat(1000 * 1024 / 4 - 100),
                    ",*a".repeat(99)
                )
                .into_bytes(),
                "yaml: document contains excessive aliasing",
            ),
            (
                "1000kb of deeply nested slices",
                "[".repeat(1000 * 1024).into_bytes(),
                "yaml: exceeded max depth of 10000",
            ),
            (
                "1000kb of deeply nested maps",
                format!("x: {}", "{".repeat(1000 * 1024)).into_bytes(),
                "yaml: exceeded max depth of 10000",
            ),
            (
                "1000kb of deeply nested indents",
                "- ".repeat(1000 * 1024).into_bytes(),
                "yaml: exceeded max depth of 10000",
            ),
            (
                "1000kb of 1000-indent lines",
                format!("{}\n", "- ".repeat(1000))
                    .repeat(1024 / 2)
                    .into_bytes(),
                "",
            ),
            ("1kb of maps", maps(1024 / 4 - 1), ""),
            ("10kb of maps", maps(10 * 1024 / 4 - 1), ""),
            ("100kb of maps", maps(100 * 1024 / 4 - 1), ""),
            ("1000kb of maps", maps(1000 * 1024 / 4 - 1), ""),
            (
                "1000kb slice nested at max-depth",
                format!(
                    "{}1{}{}",
                    "[".repeat(10000),
                    ",1".repeat(1000 * 1024 / 2 - 20000 - 1),
                    "]".repeat(10000)
                )
                .into_bytes(),
                "",
            ),
            (
                "1000kb slice nested in maps at max-depth",
                format!(
                    "{{a,b:\n{} [1{}]{}",
                    " {a,b:".repeat(10000 - 2),
                    ",1".repeat(1000 * 1024 / 2 - 6 * 10000 - 1),
                    "}".repeat(10000 - 1)
                )
                .into_bytes(),
                "",
            ),
            (
                "1000kb of 10000-nested lines",
                format!("- {}{}\n", "[".repeat(10000), "]".repeat(10000))
                    .repeat(1000 * 1024 / 20000)
                    .into_bytes(),
                "",
            ),
        ];
        for (name, data, want) in cases {
            let mut v = Value::Nil;
            let got = yaml::unmarshal(&data, &mut v)
                .err()
                .map_or_default(|e| e.to_string());
            if got != want {
                return Err(format!("{name}: got {got:?}, want {want:?}"));
            }
        }
        Ok(())
    }

    #[test]
    fn yaml_limits() -> Result<(), String> {
        on_big_stack(limits)
    }

    // ---- mapstructure ---------------------------------------------------------------------------

    /// Parses Go's `dump` of an `interface{}` value back into a [`Value`].
    struct Parser<'a> {
        text: &'a [u8],
        pos: usize,
    }

    /// A composite type for a [`Value`] (types are `'static` there).
    fn leak(ty: Type) -> &'static Type {
        Box::leak(Box::new(ty))
    }

    /// Go's type name back into a [`Type`].
    fn parse_type(name: &str) -> Result<Type, String> {
        Ok(match name {
            "interface {}" => Type::Interface,
            "bool" => Type::Bool,
            "int" => Type::Int,
            "int8" => Type::Int8,
            "int16" => Type::Int16,
            "int32" => Type::Int32,
            "int64" => Type::Int64,
            "uint" => Type::Uint,
            "uint8" => Type::Uint8,
            "uint16" => Type::Uint16,
            "uint32" => Type::Uint32,
            "uint64" => Type::Uint64,
            "float32" => Type::Float32,
            "float64" => Type::Float64,
            "string" => Type::String,
            "time.Duration" => Type::Duration,
            "time.Time" => Type::Time,
            _ => {
                if let Some(elem) = name.strip_prefix("[]") {
                    return Ok(Type::Slice(leak(parse_type(elem)?)));
                }
                let rest = name
                    .strip_prefix("map[")
                    .ok_or_else(|| format!("unknown type {name}"))?;
                let mut depth = 1;
                let end = rest
                    .char_indices()
                    .find(|&(_, c)| {
                        match c {
                            '[' => depth += 1,
                            ']' => depth -= 1,
                            _ => {}
                        }
                        depth == 0
                    })
                    .map(|(i, _)| i)
                    .ok_or_else(|| format!("unbalanced map type {name}"))?;
                let key = rest.get(..end).unwrap_or_default();
                let elem = rest.get(end + 1..).unwrap_or_default();
                Type::Map(leak(parse_type(key)?), leak(parse_type(elem)?))
            }
        })
    }

    impl Parser<'_> {
        fn error(&self, msg: &str) -> String {
            format!("{msg} at byte {} of {}", self.pos, lossy(self.text))
        }

        fn peek(&self) -> Option<u8> {
            self.text.get(self.pos).copied()
        }

        fn eat(&mut self, token: &str) -> bool {
            let found = self
                .text
                .get(self.pos..)
                .is_some_and(|rest| rest.starts_with(token.as_bytes()));
            if found {
                self.pos += token.len();
            }
            found
        }

        fn expect(&mut self, token: &str) -> Result<(), String> {
            if self.eat(token) {
                Ok(())
            } else {
                Err(self.error(&format!("expected {token:?}")))
            }
        }

        /// The bytes up to the first one that `keep` rejects.
        fn take_while(&mut self, keep: fn(u8) -> bool) -> String {
            let start = self.pos;
            while self.peek().is_some_and(keep) {
                self.pos += 1;
            }
            lossy(self.text.get(start..self.pos).unwrap_or_default())
        }

        /// A number as `FormatInt`, `FormatUint` or `FormatFloat` write it.
        fn number(&mut self) -> String {
            self.take_while(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'+' || b == b'-')
        }

        fn int<T: TryFrom<i64>>(&mut self) -> Result<T, String> {
            let token = self.number();
            token
                .parse::<i64>()
                .ok()
                .and_then(|v| T::try_from(v).ok())
                .ok_or_else(|| self.error(&format!("bad integer {token:?}")))
        }

        fn uint<T: TryFrom<u64>>(&mut self) -> Result<T, String> {
            let token = self.number();
            token
                .parse::<u64>()
                .ok()
                .and_then(|v| T::try_from(v).ok())
                .ok_or_else(|| self.error(&format!("bad unsigned integer {token:?}")))
        }

        fn float(&mut self, bits: i64) -> Result<f64, String> {
            let token = self.number();
            strconv::parse_float(&token, bits).map_err(|e| self.error(&e.to_string()))
        }

        /// A string as `strconv.Quote` writes it.
        fn string(&mut self) -> Result<String, String> {
            let start = self.pos;
            self.expect("\"")?;
            loop {
                match self.peek() {
                    None => return Err(self.error("unterminated string")),
                    Some(b'\\') => self.pos += 2,
                    Some(b'"') => break,
                    Some(_) => self.pos += 1,
                }
            }
            self.pos += 1;
            let quoted = lossy(self.text.get(start..self.pos).unwrap_or_default());
            strconv::unquote(&quoted).map_err(|e| self.error(&e.to_string()))
        }

        /// The entries of a map rendering, after its `{`.
        fn entries(&mut self, key: Type, elem: Type) -> Result<Vec<(Value, Value)>, String> {
            let mut entries = Vec::new();
            if self.eat("}") {
                return Ok(entries);
            }
            loop {
                let k = self.value(key)?;
                self.expect(": ")?;
                entries.push((k, self.value(elem)?));
                if self.eat("}") {
                    return Ok(entries);
                }
                self.expect(", ")?;
            }
        }

        /// A value held in a value of the static type `ty`.
        fn value(&mut self, ty: Type) -> Result<Value, String> {
            Ok(match ty {
                Type::Interface => {
                    if self.eat("nil") {
                        return Ok(Value::Nil);
                    }
                    self.expect("<")?;
                    let name = self.take_while(|b| b != b'>');
                    self.expect(">")?;
                    let dynamic = parse_type(&name)?;
                    return self.value(dynamic);
                }
                Type::Bool => {
                    if self.eat("true") {
                        Value::Bool(true)
                    } else {
                        self.expect("false")?;
                        Value::Bool(false)
                    }
                }
                Type::Int => Value::Int(self.int()?),
                Type::Int8 => Value::Int8(self.int()?),
                Type::Int16 => Value::Int16(self.int()?),
                Type::Int32 => Value::Int32(self.int()?),
                Type::Int64 => Value::Int64(self.int()?),
                Type::Duration => Value::Duration(Duration(self.int()?)),
                Type::Uint => Value::Uint(self.uint()?),
                Type::Uint8 => Value::Uint8(self.uint()?),
                Type::Uint16 => Value::Uint16(self.uint()?),
                Type::Uint32 => Value::Uint32(self.uint()?),
                Type::Uint64 => Value::Uint64(self.uint()?),
                Type::Float32 => Value::Float32(math::float32(self.float(32)?)),
                Type::Float64 => Value::Float64(self.float(64)?),
                Type::String => Value::String(self.string()?),
                Type::Time => {
                    self.expect("time(")?;
                    let text = self.take_while(|b| b != b')');
                    self.expect(")")?;
                    Value::Time(parse_time(&text).map_err(|e| self.error(&e))?)
                }
                Type::Slice(elem) => {
                    if self.eat("nil") {
                        return Ok(Value::Slice(elem, None));
                    }
                    self.expect("[")?;
                    let mut items = Vec::new();
                    if !self.eat("]") {
                        loop {
                            items.push(self.value(*elem)?);
                            if self.eat("]") {
                                break;
                            }
                            self.expect(", ")?;
                        }
                    }
                    Value::Slice(elem, Some(items))
                }
                Type::Map(key, elem) => {
                    let nil = self.eat("nil");
                    let entries = if nil {
                        None
                    } else {
                        self.expect("{")?;
                        Some(self.entries(*key, *elem)?)
                    };
                    if *key == Type::String {
                        let map = entries.map(|entries| {
                            entries
                                .into_iter()
                                .map(|(k, v)| (k.as_str().unwrap_or_default().to_owned(), v))
                                .collect()
                        });
                        Value::StringMap(elem, map)
                    } else {
                        Value::Map(key, elem, entries)
                    }
                }
            })
        }
    }

    /// Go's `Time.String()` (`2006-01-02 15:04:05.999999999 -0700 MST`) back into a [`Time`]:
    /// `UTC` is `time.UTC`, a zone named by its offset an unnamed fixed zone.
    fn parse_time(text: &str) -> Result<Time, String> {
        let bad = || format!("bad time {text:?}");
        let num = |s: &str| {
            s.parse::<i64>()
                .map_err(|e| format!("bad time {text:?}: {e}"))
        };
        let parts: Vec<&str> = text.split(' ').collect();
        let [date, clock, offset, zone] = parts.as_slice() else {
            return Err(bad());
        };
        let date: Vec<&str> = date.split('-').collect();
        let [year, month, day] = date.as_slice() else {
            return Err(bad());
        };
        let (hms, frac) = clock.split_once('.').unwrap_or((clock, ""));
        let hms: Vec<&str> = hms.split(':').collect();
        let [hour, minute, second] = hms.as_slice() else {
            return Err(bad());
        };
        let nanosecond = if frac.is_empty() {
            0
        } else {
            num(&format!("{frac:0<9}"))?
        };
        let sign = match offset.get(..1) {
            Some("+") => 1,
            Some("-") => -1,
            _ => return Err(bad()),
        };
        let hours = num(offset.get(1..3).ok_or_else(bad)?)?;
        let minutes = num(offset.get(3..).ok_or_else(bad)?)?;
        let seconds = sign * (hours * 3600 + minutes * 60);
        let loc = if *zone == "UTC" && seconds == 0 {
            Location::Utc
        } else if zone == offset {
            Location::fixed_zone("", seconds)
        } else {
            Location::fixed_zone(zone, seconds)
        };
        let fields = DateFields {
            year: num(year)?,
            month: num(month)?,
            day: num(day)?,
            hour: num(hour)?,
            minute: num(minute)?,
            second: num(second)?,
            nanosecond,
        };
        Ok(Time::date(&fields, loc))
    }

    /// Parses a `dump` of an `interface{}` value; the parse must render back to `text`.
    fn parse_dump(text: &[u8]) -> Result<Value, String> {
        let mut parser = Parser { text, pos: 0 };
        let value = parser.value(Type::Interface)?;
        if parser.pos != text.len() {
            return Err(parser.error("trailing input"));
        }
        let back = dump(&value);
        if back.as_bytes() != text {
            return Err(format!("{} parses as {}", lossy(text), back));
        }
        Ok(value)
    }

    /// Go: `msConfig`.
    fn ms_config(name: &str) -> DecoderConfig {
        match name {
            "strict" => DecoderConfig::default(),
            "weak" => DecoderConfig {
                weakly_typed_input: true,
                ..DecoderConfig::default()
            },
            "viper-unused" => DecoderConfig {
                error_unused: true,
                ..DecoderConfig::viper()
            },
            "viper-zero" => DecoderConfig {
                zero_fields: true,
                ..DecoderConfig::viper()
            },
            "prebid" => DecoderConfig {
                decode_hook: Some(account_modules_hook()),
                weakly_typed_input: true,
                ..DecoderConfig::default()
            },
            _ => DecoderConfig::viper(),
        }
    }

    /// Go: `msDecode` into a new `T`, after decoding `prefill` into it.
    fn ms_outcome<T: MsDecode + Dump + Default>(
        config: &DecoderConfig,
        prefill: Option<&Value>,
        input: &Value,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut out = T::default();
        if let Some(prefill) = prefill {
            let _ignored = Decoder::new(config.clone()).decode(prefill, &mut out);
        }
        match Decoder::new(config.clone()).decode(input, &mut out) {
            Ok(()) => (dump(&out).into_bytes(), Vec::new()),
            Err(DecodeError::Panic(text)) => (Vec::new(), panic_text(&text)),
            Err(e) => (dump(&out).into_bytes(), e.to_string().into_bytes()),
        }
    }

    /// Go: `msTargets`: [`ms_outcome`] for the target type named `target`.
    fn ms_target(
        target: &str,
        config: &DecoderConfig,
        prefill: Option<&Value>,
        input: &Value,
    ) -> Option<(Vec<u8>, Vec<u8>)> {
        let (c, p, i) = (config, prefill, input);
        Some(match target {
            "Kitchen" => ms_outcome::<Kitchen>(c, p, i),
            "BidderInfo" => ms_outcome::<BidderInfo>(c, p, i),
            "Basic" => ms_outcome::<Basic>(c, p, i),
            "BasicPointer" => ms_outcome::<BasicPointer>(c, p, i),
            "BasicSquash" => ms_outcome::<BasicSquash>(c, p, i),
            "Embedded" => ms_outcome::<Embedded>(c, p, i),
            "EmbeddedPointer" => ms_outcome::<EmbeddedPointer>(c, p, i),
            "EmbeddedSquash" => ms_outcome::<EmbeddedSquash>(c, p, i),
            "EmbeddedAndNamed" => ms_outcome::<EmbeddedAndNamed>(c, p, i),
            "EmbeddedSlice" => ms_outcome::<EmbeddedSlice>(c, p, i),
            "Map" => ms_outcome::<Map>(c, p, i),
            "MapOfStruct" => ms_outcome::<MapOfStruct>(c, p, i),
            "NestedBasic" => ms_outcome::<NestedBasic>(c, p, i),
            "NestedPointer" => ms_outcome::<NestedPointer>(c, p, i),
            "Slice" => ms_outcome::<Slice>(c, p, i),
            "SliceOfAlias" => ms_outcome::<SliceOfAlias>(c, p, i),
            "SliceOfStruct" => ms_outcome::<SliceOfStruct>(c, p, i),
            "SlicePointer" => ms_outcome::<SlicePointer>(c, p, i),
            "Tagged" => ms_outcome::<Tagged>(c, p, i),
            "Remainder" => ms_outcome::<Remainder>(c, p, i),
            "TypeConversionResult" => ms_outcome::<TypeConversionResult>(c, p, i),
            "modulesHolder" => ms_outcome::<ModulesHolder>(c, p, i),
            "map[string]interface {}" => ms_outcome::<Option<BTreeMap<String, Value>>>(c, p, i),
            "map[string]string" => ms_outcome::<Option<BTreeMap<String, String>>>(c, p, i),
            "[]string" => ms_outcome::<Option<Vec<String>>>(c, p, i),
            "[]interface {}" => ms_outcome::<Option<Vec<Value>>>(c, p, i),
            "interface {}" => ms_outcome::<Value>(c, p, i),
            "string" => ms_outcome::<String>(c, p, i),
            "int" => ms_outcome::<isize>(c, p, i),
            "*int" => ms_outcome::<Option<isize>>(c, p, i),
            "bool" => ms_outcome::<bool>(c, p, i),
            "float64" => ms_outcome::<f64>(c, p, i),
            "time.Duration" => ms_outcome::<Duration>(c, p, i),
            "int8" => ms_outcome::<i8>(c, p, i),
            "uint8" => ms_outcome::<u8>(c, p, i),
            "uint" => ms_outcome::<usize>(c, p, i),
            "float32" => ms_outcome::<f32>(c, p, i),
            "[]int" => ms_outcome::<Option<Vec<isize>>>(c, p, i),
            "[]uint8" => ms_outcome::<Option<Vec<u8>>>(c, p, i),
            "map[string]int" => ms_outcome::<Option<BTreeMap<String, isize>>>(c, p, i),
            "map[int]string" => ms_outcome::<Option<BTreeMap<isize, String>>>(c, p, i),
            "map[string][]string" => {
                ms_outcome::<Option<BTreeMap<String, Option<Vec<String>>>>>(c, p, i)
            }
            "[]map[string]string" => {
                ms_outcome::<Option<Vec<Option<BTreeMap<String, String>>>>>(c, p, i)
            }
            "time.Time" => ms_outcome::<Time>(c, p, i),
            "*time.Time" => ms_outcome::<Option<Time>>(c, p, i),
            "Nested" => ms_outcome::<Nested>(c, p, i),
            "*Nested" => ms_outcome::<Option<Box<Nested>>>(c, p, i),
            "**int" => ms_outcome::<Option<Option<isize>>>(c, p, i),
            _ => return None,
        })
    }

    /// Go: `msConfigNames`.
    const MS_CONFIGS: [&str; 6] = [
        "strict",
        "weak",
        "viper",
        "viper-unused",
        "viper-zero",
        "prebid",
    ];

    fn replay_mapstructure() -> Result<(), String> {
        let mut report = Report::new("mapstructure.txt");
        let configs: Vec<DecoderConfig> = MS_CONFIGS.iter().map(|name| ms_config(name)).collect();
        for row in rows("mapstructure.txt")? {
            let name = lossy(col(&row, 0)?);
            let target = lossy(col(&row, 1)?);
            let prefill_text = col(&row, 2)?;
            let prefill = if prefill_text.is_empty() {
                None
            } else {
                Some(parse_dump(prefill_text)?)
            };
            let input = parse_dump(col(&row, 3)?)?;
            let mut results = Vec::with_capacity(configs.len());
            for config in &configs {
                results.push(
                    ms_target(&target, config, prefill.as_ref(), &input)
                        .ok_or_else(|| format!("{name}: no Rust type for {target}"))?,
                );
            }
            let viper = results.get(2).cloned().unwrap_or_default();
            let mut outputs = Vec::with_capacity(2 * results.len());
            for (config, result) in MS_CONFIGS.iter().zip(results) {
                let (value, error) = if *config != "viper" && result == viper {
                    (b"=".to_vec(), Vec::new())
                } else {
                    result
                };
                outputs.push((format!("{config} value"), value));
                outputs.push((format!("{config} error"), error));
            }
            let outputs: Vec<(&str, &[u8])> = outputs
                .iter()
                .map(|(column, bytes)| (column.as_str(), bytes.as_slice()))
                .collect();
            report.check(
                &name,
                &format!(
                    "mapstructure {name} into {target}: input {}",
                    lossy(col(&row, 3)?)
                ),
                &row,
                4,
                &outputs,
            )?;
        }
        report.finish()
    }

    #[test]
    fn mapstructure_golden() -> Result<(), String> {
        on_big_stack(replay_mapstructure)
    }
}
