//! The decoder (Go: `decoder` of `decode.go`, and `getStructInfo` of `yaml.go`): decodes a
//! node tree into Rust values standing for Go values.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    hash::{BuildHasher, Hash},
    sync::OnceLock,
};

use pbs_gocompat::{
    base64, math, strconv,
    time::{Duration, Time, parse_duration},
};
use pbs_gojson::{GoKind, GoString};

use super::{
    TypeError, UnmarshalError,
    compose::{Kind, Node},
    resolve::{
        BINARY_TAG, MAP_TAG, MERGE_TAG, NULL_TAG, Resolved, SEQ_TAG, STR_TAG, resolve, short_tag,
    },
};
use crate::{
    TWO_POW_63, TWO_POW_64, go_uint64, int_to_float, uint_to_float,
    value::{Type, Value},
};

/// A node of the composed document.
pub type NodeId = usize;

/// The node read for an id past the arena (never produced by the composer).
static EMPTY_NODE: Node = Node {
    kind: Kind::Scalar,
    quoted: false,
    tag: String::new(),
    value: String::new(),
    alias: 0,
    content: Vec::new(),
    line: 0,
};

/// A key of Go's `mergedFields`: `k.Interface()`, the dynamic type's name and value.
#[derive(Debug)]
struct MergeKey {
    /// Go's `reflect.Type.String()` of the dynamic type.
    ty: String,
    /// The value, compared with Go's `==`.
    value: Value,
}

impl MergeKey {
    /// Go's `==` on the interface values.
    fn go_eq(&self, other: &Self) -> bool {
        self.ty == other.ty && self.value.decoded_key_eq(&other.value)
    }
}

/// Go: `decoder`: the state of one `Unmarshal` (or `Decoder.Decode`) call.
#[derive(Debug)]
pub struct Decoder<'n> {
    /// The node arena.
    nodes: &'n [Node],
    /// Go: `aliases`: the alias nodes being decoded.
    aliases: HashSet<NodeId>,
    /// Go: `terrors`.
    terrors: Vec<Vec<u8>>,
    /// Go: `knownFields`.
    known_fields: bool,
    /// Go: `decodeCount`.
    decode_count: i64,
    /// Go: `aliasCount`.
    alias_count: i64,
    /// Go: `aliasDepth`.
    alias_depth: i64,
    /// The `mergedFields` maps created so far (Go shares them by reference).
    merge_sets: Vec<Vec<MergeKey>>,
    /// Go: `mergedFields`, an index into `merge_sets` (`None` is a nil map).
    merged: Option<usize>,
}

/// A resolved scalar, as a [`YamlDecode`] implementation receives it.
#[derive(Debug)]
pub struct Scalar<'a> {
    /// The resolved tag.
    tag: &'a [u8],
    /// The resolved value (never `Resolved::Null`).
    resolved: Resolved<'a>,
    /// Go: `n.Value`.
    value: &'a str,
}

/// Go: `OverflowInt` for a `bits`-bit signed integer.
const fn fits_int(v: i64, bits: u32) -> bool {
    if bits >= 64 {
        return true;
    }
    let half = 1_i64 << (bits - 1);
    -half <= v && v < half
}

/// Go: `OverflowUint` for a `bits`-bit unsigned integer.
const fn fits_uint(v: u64, bits: u32) -> bool {
    bits >= 64 || v >> bits == 0
}

/// A non-negative count as a Go `float64` (exact below `2^53`).
fn count_to_float(n: i64) -> f64 {
    let hi = i32::try_from(n >> 32).unwrap_or(i32::MAX);
    let lo = u32::try_from(n & 0xFFFF_FFFF).unwrap_or(0);
    f64::from(hi).mul_add(4_294_967_296.0, f64::from(lo))
}

/// Go: `allowedAliasRatio`.
fn allowed_alias_ratio(decode_count: i64) -> f64 {
    const RANGE_LOW: i64 = 400_000;
    const RANGE_HIGH: i64 = 4_000_000;
    if decode_count <= RANGE_LOW {
        // 400,000 decode operations is ~500kb of dense object declarations, or
        // ~5kb of dense object declarations with 10000% alias expansion.
        0.99
    } else if decode_count >= RANGE_HIGH {
        // 4,000,000 decode operations is ~5MB of dense object declarations, or
        // ~4.5MB of dense object declarations with 10% alias expansion.
        0.10
    } else {
        // Scale between 99% at 400kb and 10% at 4MB. Go rounds the product before the
        // subtraction (no fused multiply-add on amd64).
        let scaled = 0.89 * (count_to_float(decode_count - RANGE_LOW) / 3_600_000.0);
        0.99 - scaled
    }
}

impl Scalar<'_> {
    /// The value of a Go string target (Go: the `reflect.String` case).
    fn string_bytes(&self) -> Cow<'_, [u8]> {
        match &self.resolved {
            Resolved::Binary(data) if self.tag == BINARY_TAG => Cow::Borrowed(data),
            Resolved::Null
            | Resolved::Bool(_)
            | Resolved::Int(_)
            | Resolved::Uint(_)
            | Resolved::Float(_)
            | Resolved::Str(_)
            | Resolved::Time(_)
            | Resolved::Binary(_) => Cow::Borrowed(self.value.as_bytes()),
        }
    }

    /// The value as a Go `string`, if it resolved to one.
    fn resolved_string(&self) -> Option<Cow<'_, str>> {
        match &self.resolved {
            Resolved::Str(s) => Some(Cow::Borrowed(s)),
            Resolved::Binary(data) => Some(String::from_utf8_lossy(data)),
            Resolved::Null
            | Resolved::Bool(_)
            | Resolved::Int(_)
            | Resolved::Uint(_)
            | Resolved::Float(_)
            | Resolved::Time(_) => None,
        }
    }

    /// Go: the signed integer case of `d.scalar` for a `bits`-bit target.
    fn int(&self, bits: u32) -> Option<i64> {
        let v = match self.resolved {
            Resolved::Int(v) => v,
            Resolved::Uint(v) => i64::try_from(v).ok()?,
            Resolved::Float(f) if f <= TWO_POW_63 => math::int64(f),
            Resolved::Float(_)
            | Resolved::Null
            | Resolved::Bool(_)
            | Resolved::Str(_)
            | Resolved::Time(_)
            | Resolved::Binary(_) => return None,
        };
        fits_int(v, bits).then_some(v)
    }

    /// Go: the unsigned integer case of `d.scalar` for a `bits`-bit target.
    fn uint(&self, bits: u32) -> Option<u64> {
        let v = match self.resolved {
            Resolved::Int(v) => u64::try_from(v).ok()?,
            Resolved::Uint(v) => v,
            Resolved::Float(f) if f <= TWO_POW_64 => go_uint64(f),
            Resolved::Float(_)
            | Resolved::Null
            | Resolved::Bool(_)
            | Resolved::Str(_)
            | Resolved::Time(_)
            | Resolved::Binary(_) => return None,
        };
        fits_uint(v, bits).then_some(v)
    }

    /// Go: the float case of `d.scalar`.
    fn float(&self) -> Option<f64> {
        match self.resolved {
            Resolved::Int(v) => Some(int_to_float(v)),
            Resolved::Uint(v) => Some(uint_to_float(v)),
            Resolved::Float(f) => Some(f),
            Resolved::Null
            | Resolved::Bool(_)
            | Resolved::Str(_)
            | Resolved::Time(_)
            | Resolved::Binary(_) => None,
        }
    }

    /// Go: the bool case of `d.scalar`, which accepts YAML 1.1 words from strings.
    fn bool(&self) -> Option<bool> {
        if let Resolved::Bool(b) = self.resolved {
            return Some(b);
        }
        match self.resolved_string()?.as_ref() {
            "y" | "Y" | "yes" | "Yes" | "YES" | "on" | "On" | "ON" => Some(true),
            "n" | "N" | "no" | "No" | "NO" | "off" | "Off" | "OFF" => Some(false),
            _ => None,
        }
    }

    /// The value as Go's `interface{}` holds it.
    fn interface(&self) -> Value {
        match &self.resolved {
            Resolved::Null => Value::Nil,
            Resolved::Bool(b) => Value::Bool(*b),
            Resolved::Int(v) => Value::Int(*v),
            Resolved::Uint(v) => Value::Uint64(*v),
            Resolved::Float(f) => Value::Float64(*f),
            Resolved::Str(s) => Value::String((*s).to_owned()),
            Resolved::Time(t) => Value::Time(t.clone()),
            Resolved::Binary(data) => Value::String(String::from_utf8_lossy(data).into_owned()),
        }
    }
}

/// A Rust type yaml.v3 can decode into, standing for a Go type.
///
/// Implemented for the types of the [module](super) table, and by
/// [`#[derive(GoYaml)]`](crate::GoYaml) for structs and newtypes. The `#[doc(hidden)]` methods
/// are the decoder's per-kind entry points.
pub trait YamlDecode {
    /// Go's `reflect.Kind` of the type.
    const YAML_KIND: GoKind;
    /// Whether `Option<Self>` stands for `*Self` (slices and maps make it a nil-able value of
    /// the same kind instead).
    #[doc(hidden)]
    const YAML_OPTION_IS_PTR: bool = true;

    /// Appends Go's `reflect.Type.String()` of the type, which the type errors name.
    fn yaml_type_name(out: &mut String);

    /// Go: `d.unmarshal` after the document and alias cases: `prepare` and the dispatch on
    /// the node's kind.
    #[doc(hidden)]
    fn yaml_decode(&mut self, d: &mut Decoder<'_>, n: NodeId) -> Result<bool, UnmarshalError> {
        d.dispatch(n, self)
    }

    /// Go: `d.null`: nil-able kinds take their zero value (`true`), others are left alone.
    #[doc(hidden)]
    fn yaml_null(&mut self) -> bool {
        false
    }

    /// Go: the kind switch of `d.scalar`; `false` makes the decoder report a type error.
    ///
    /// # Errors
    ///
    /// A fatal error, such as a failing `UnmarshalText`.
    #[doc(hidden)]
    fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
        let _ = s;
        Ok(false)
    }

    /// Go: `d.sequence`; `None` makes the decoder report a type error.
    ///
    /// # Errors
    ///
    /// A fatal error of an element.
    #[doc(hidden)]
    fn yaml_sequence(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        let _ = (d, n);
        Ok(None)
    }

    /// Go: the kind switch of `d.mapping`; `None` makes the decoder report a type error.
    ///
    /// # Errors
    ///
    /// A fatal error of a key or value.
    #[doc(hidden)]
    fn yaml_mapping(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        let _ = (d, n);
        Ok(None)
    }

    /// [`YamlDecode::yaml_mapping`] when the caller knows whether Go's map was nil.
    ///
    /// # Errors
    ///
    /// A fatal error of a key or value.
    #[doc(hidden)]
    fn yaml_mapping_nil(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
        was_nil: bool,
    ) -> Result<Option<bool>, UnmarshalError> {
        let _ = was_nil;
        self.yaml_mapping(d, n)
    }

    /// Go: `k.Interface()` of a decoded map key, as the merge bookkeeping compares it: the
    /// dynamic type's name and value. `None` keeps the key out of that bookkeeping.
    #[doc(hidden)]
    fn yaml_key_interface(&self) -> Option<(String, Value)> {
        None
    }

    /// The kind of a decoded map key, looking through interfaces (`None` for a nil one).
    #[doc(hidden)]
    fn yaml_key_kind(&self) -> Option<GoKind> {
        Some(Self::YAML_KIND)
    }
}

/// The Go type name of `T`.
fn type_name<T: YamlDecode + ?Sized>() -> String {
    let mut out = String::new();
    T::yaml_type_name(&mut out);
    out
}

/// A map target of `d.mapping` (Go: `reflect.Map` values).
trait MapTarget: YamlDecode {
    /// The key type.
    type Key: YamlDecode + Default;
    /// The element type.
    type Elem: YamlDecode + Default;
    /// Go: `out.MapIndex(k).IsValid()`.
    fn map_contains(&self, key: &Self::Key) -> bool;
    /// Go: `out.SetMapIndex(k, e)`.
    fn map_insert(&mut self, key: Self::Key, elem: Self::Elem);
}

/// Go: `isMerge`.
fn is_merge(n: &Node) -> bool {
    n.kind == Kind::Scalar
        && n.value == "<<"
        && (n.tag.is_empty() || n.tag == "!" || *short_tag(n.tag.as_bytes()) == *MERGE_TAG)
}

/// Go: `Node.indicatedString`.
fn indicated_string(n: &Node) -> bool {
    n.kind == Kind::Scalar
        && (*short_tag(n.tag.as_bytes()) == *STR_TAG
            || (n.tag.is_empty() || n.tag == "!") && n.quoted)
}

/// Go: `failWantMap`.
fn fail_want_map() -> UnmarshalError {
    UnmarshalError::failf("map merge requires map or sequence of maps as the value")
}

impl<'n> Decoder<'n> {
    /// Go: `newDecoder`.
    pub(super) fn new(nodes: &'n [Node], known_fields: bool) -> Self {
        Self {
            nodes,
            aliases: HashSet::new(),
            terrors: Vec::new(),
            known_fields,
            decode_count: 0,
            alias_count: 0,
            alias_depth: 0,
            merge_sets: Vec::new(),
            merged: None,
        }
    }

    /// The collected type errors, as Go's `unmarshal` returns them.
    pub(super) fn finish(self) -> Result<(), UnmarshalError> {
        if self.terrors.is_empty() {
            Ok(())
        } else {
            Err(UnmarshalError::Type(TypeError {
                errors: self.terrors,
            }))
        }
    }

    /// The node `n`.
    fn node(&self, n: NodeId) -> &'n Node {
        self.nodes.get(n).unwrap_or(&EMPTY_NODE)
    }

    /// The children of node `n` (Go: `n.Content`).
    #[must_use]
    pub fn content(&self, n: NodeId) -> &'n [NodeId] {
        &self.node(n).content
    }

    /// Go: `n.ShortTag()`.
    fn short_tag_of(&self, n: NodeId) -> Cow<'n, [u8]> {
        let mut node = self.node(n);
        // An alias refers to a non-alias node, so this follows at most one alias.
        for _ in 0..2 {
            if indicated_string(node) {
                return Cow::Borrowed(STR_TAG);
            }
            if !node.tag.is_empty() && node.tag != "!" {
                return short_tag(node.tag.as_bytes());
            }
            match node.kind {
                Kind::Mapping => return Cow::Borrowed(MAP_TAG),
                Kind::Sequence => return Cow::Borrowed(SEQ_TAG),
                Kind::Scalar => {
                    return resolve(b"", &node.value)
                        .map_or(Cow::Borrowed(STR_TAG), |(tag, _)| tag);
                }
                Kind::Alias => node = self.node(node.alias),
                Kind::Document => return Cow::Borrowed(b""),
            }
        }
        Cow::Borrowed(b"")
    }

    /// Whether node `n` is a null (Go: `n.ShortTag() == nullTag`).
    #[must_use]
    pub fn is_null(&self, n: NodeId) -> bool {
        *self.short_tag_of(n) == *NULL_TAG
    }

    /// Go: `d.terror`.
    fn terror(&mut self, n: NodeId, tag: &[u8], out_type: &str) {
        let node = self.node(n);
        let tag = if node.tag.is_empty() {
            tag
        } else {
            node.tag.as_bytes()
        };
        let mut msg = format!("line {}: cannot unmarshal ", node.line).into_bytes();
        msg.extend_from_slice(&short_tag(tag));
        let value = node.value.as_bytes();
        if tag != SEQ_TAG && tag != MAP_TAG {
            msg.extend_from_slice(b" `");
            if value.len() > 10 {
                msg.extend_from_slice(value.get(..7).unwrap_or(value));
                msg.extend_from_slice(b"...");
            } else {
                msg.extend_from_slice(value);
            }
            msg.push(b'`');
        } else {
            // Go writes the value unquoted and whole after a !!seq or !!map tag: empty for a
            // collection, the text of a scalar tagged so (`!!map1h30m`).
            msg.extend_from_slice(value);
        }
        msg.extend_from_slice(b" into ");
        msg.extend_from_slice(out_type.as_bytes());
        self.terrors.push(msg);
    }

    /// Go: `d.unmarshal(n, out)`: decodes node `n` into `out`, returning whether it was set.
    ///
    /// # Errors
    ///
    /// A fatal error, which stops the decoding.
    pub fn unmarshal<T: YamlDecode + ?Sized>(
        &mut self,
        n: NodeId,
        out: &mut T,
    ) -> Result<bool, UnmarshalError> {
        self.decode_count += 1;
        if self.alias_depth > 0 {
            self.alias_count += 1;
        }
        if self.alias_count > 100
            && self.decode_count > 1000
            && count_to_float(self.alias_count) / count_to_float(self.decode_count)
                > allowed_alias_ratio(self.decode_count)
        {
            return Err(UnmarshalError::failf(
                "document contains excessive aliasing",
            ));
        }
        let node = self.node(n);
        match node.kind {
            Kind::Document => {
                if let [child] = node.content.as_slice() {
                    let _good = self.unmarshal(*child, out)?;
                    return Ok(true);
                }
                Ok(false)
            }
            Kind::Alias => {
                if !self.aliases.insert(n) {
                    return Err(UnmarshalError::failf(&format!(
                        "anchor '{}' value contains itself",
                        node.value
                    )));
                }
                self.alias_depth += 1;
                let good = self.unmarshal(node.alias, out)?;
                self.alias_depth -= 1;
                let _removed = self.aliases.remove(&n);
                Ok(good)
            }
            Kind::Scalar | Kind::Mapping | Kind::Sequence => out.yaml_decode(self, n),
        }
    }

    /// The dispatch on the node's kind of Go's `d.unmarshal`, for a target that `prepare` does
    /// not dereference.
    ///
    /// # Errors
    ///
    /// A fatal error, which stops the decoding.
    pub fn dispatch<T: YamlDecode + ?Sized>(
        &mut self,
        n: NodeId,
        out: &mut T,
    ) -> Result<bool, UnmarshalError> {
        match self.node(n).kind {
            Kind::Scalar => self.scalar(n, out),
            Kind::Mapping => self.mapping(n, out),
            Kind::Sequence => {
                if let Some(good) = out.yaml_sequence(self, n)? {
                    return Ok(good);
                }
                self.terror(n, SEQ_TAG, &type_name::<T>());
                Ok(false)
            }
            Kind::Document | Kind::Alias => self.unmarshal(n, out),
        }
    }

    /// Go: `d.scalar`.
    fn scalar<T: YamlDecode + ?Sized>(
        &mut self,
        n: NodeId,
        out: &mut T,
    ) -> Result<bool, UnmarshalError> {
        let node = self.node(n);
        let (tag, resolved) = if indicated_string(node) {
            (Cow::Borrowed(STR_TAG), Resolved::Str(&node.value))
        } else {
            let (tag, resolved) = resolve(node.tag.as_bytes(), &node.value)?;
            if *tag == *BINARY_TAG {
                let data = base64::STD_ENCODING
                    .decode_string(&node.value)
                    .ok()
                    .ok_or_else(|| {
                        UnmarshalError::failf("!!binary value contains invalid base64 data")
                    })?;
                (tag, Resolved::Binary(data))
            } else {
                (tag, resolved)
            }
        };
        if matches!(resolved, Resolved::Null) {
            return Ok(out.yaml_null());
        }
        let scalar = Scalar {
            tag: &tag,
            resolved,
            value: &node.value,
        };
        if out.yaml_scalar(&scalar)? {
            return Ok(true);
        }
        self.terror(n, &tag, &type_name::<T>());
        Ok(false)
    }

    /// Go: `d.mapping` up to its kind switch.
    fn mapping<T: YamlDecode + ?Sized>(
        &mut self,
        n: NodeId,
        out: &mut T,
    ) -> Result<bool, UnmarshalError> {
        let content = self.content(n);
        let nerrs = self.terrors.len();
        for (i, &ki) in content.iter().enumerate().step_by(2) {
            let ni = self.node(ki);
            for &kj in content.iter().skip(i + 2).step_by(2) {
                let nj = self.node(kj);
                if ni.kind == nj.kind && ni.value == nj.value {
                    self.terrors.push(
                        format!(
                            "line {}: mapping key {} already defined at line {}",
                            nj.line,
                            strconv::quote(&nj.value),
                            ni.line
                        )
                        .into_bytes(),
                    );
                }
            }
        }
        if self.terrors.len() > nerrs {
            return Ok(false);
        }
        if let Some(good) = out.yaml_mapping(self, n)? {
            return Ok(good);
        }
        self.terror(n, MAP_TAG, &type_name::<T>());
        Ok(false)
    }

    /// Go: `isStringMap`.
    fn is_string_map(&self, n: NodeId) -> bool {
        self.content(n).iter().step_by(2).all(|&k| {
            let tag = self.short_tag_of(k);
            *tag == *STR_TAG || *tag == *MERGE_TAG
        })
    }

    /// The `(key, value)` pairs of a mapping node.
    fn pairs(&self, n: NodeId) -> impl Iterator<Item = (NodeId, NodeId)> + use<'n> {
        let content = self.content(n);
        content
            .iter()
            .step_by(2)
            .zip(content.iter().skip(1).step_by(2))
            .map(|(&k, &v)| (k, v))
    }

    /// Starts a new `mergedFields` map.
    fn new_merge_set(&mut self) -> usize {
        self.merge_sets.push(Vec::new());
        self.merge_sets.len() - 1
    }

    /// Go: `mergedFields[k] = true` after the `mergedFields[k]` lookup: `false` when the key
    /// was already merged.
    fn merge_insert(&mut self, set: usize, key: MergeKey) -> bool {
        let Some(keys) = self.merge_sets.get_mut(set) else {
            return true;
        };
        if keys.iter().any(|k| k.go_eq(&key)) {
            return false;
        }
        keys.push(key);
        true
    }

    /// Go's `k.Interface()` as a `mergedFields` key (Go panics hashing slices and maps).
    fn merge_key<K: YamlDecode + ?Sized>(key: &K) -> Result<Option<MergeKey>, UnmarshalError> {
        let Some((ty, value)) = key.yaml_key_interface() else {
            return Ok(None);
        };
        if matches!(value.kind(), Some(GoKind::Map | GoKind::Slice)) {
            return Err(UnmarshalError::Panic(format!(
                "runtime error: hash of unhashable type {}",
                value.type_name()
            )));
        }
        Ok(Some(MergeKey { ty, value }))
    }

    /// Go: the map case of `d.mapping`; `was_nil` is Go's `out.IsNil()`.
    fn decode_map<M: MapTarget + ?Sized>(
        &mut self,
        n: NodeId,
        out: &mut M,
        was_nil: bool,
    ) -> Result<bool, UnmarshalError> {
        let merged = self.merged.take();
        let mut merge_node = None;
        for (kn, vn) in self.pairs(n) {
            if is_merge(self.node(kn)) {
                merge_node = Some(vn);
                continue;
            }
            let mut k = M::Key::default();
            if !self.unmarshal(kn, &mut k)? {
                continue;
            }
            if let Some(set) = merged
                && let Some(key) = Self::merge_key(&k)?
                && !self.merge_insert(set, key)
            {
                continue;
            }
            if matches!(k.yaml_key_kind(), Some(GoKind::Map | GoKind::Slice)) {
                let shown = k.yaml_key_interface().map_or_default(|(_, v)| v);
                return Err(UnmarshalError::failf(&format!(
                    "invalid map key: {}",
                    shown.go_syntax()
                )));
            }
            let mut e = M::Elem::default();
            if self.unmarshal(vn, &mut e)? || self.is_null(vn) && (was_nil || !out.map_contains(&k))
            {
                out.map_insert(k, e);
            }
        }
        self.merged = merged;
        if let Some(m) = merge_node {
            self.merge(n, m, out)?;
        }
        Ok(true)
    }

    /// Go: `d.mappingStruct`.
    ///
    /// # Errors
    ///
    /// A fatal error of a key or field, or [`UnmarshalError::Panic`] for an invalid struct
    /// definition.
    pub fn decode_struct<S: YamlStruct + ?Sized>(
        &mut self,
        n: NodeId,
        out: &mut S,
    ) -> Result<bool, UnmarshalError> {
        let info = S::yaml_struct_info()?;
        let merged = self.merged.take();
        let mut merge_node = None;
        let mut done = vec![false; info.fields.len()];
        let mut name = String::new();
        for (kn, vn) in self.pairs(n) {
            let ni = self.node(kn);
            if is_merge(ni) {
                merge_node = Some(vn);
                continue;
            }
            if !self.unmarshal(kn, &mut name)? {
                continue;
            }
            if let Some(set) = merged {
                let key = MergeKey {
                    ty: "string".to_owned(),
                    value: Value::String(name.clone()),
                };
                if !self.merge_insert(set, key) {
                    continue;
                }
            }
            if let Some(field) = info.field(&name) {
                if let Some(slot) = done.get_mut(field.id) {
                    if *slot {
                        self.terrors.push(
                            format!(
                                "line {}: field {} already set in type {}",
                                ni.line,
                                name,
                                type_name::<S>()
                            )
                            .into_bytes(),
                        );
                        continue;
                    }
                    *slot = true;
                }
                out.yaml_field(self, &field.path, vn)?;
            } else if info.inline_map {
                out.yaml_inline_map(self, name.clone(), vn)?;
            } else if self.known_fields {
                self.terrors.push(
                    format!(
                        "line {}: field {} not found in type {}",
                        ni.line,
                        name,
                        type_name::<S>()
                    )
                    .into_bytes(),
                );
            }
        }
        self.merged = merged;
        if let Some(m) = merge_node {
            self.merge(n, m, out)?;
        }
        Ok(true)
    }

    /// Go: `d.merge`.
    fn merge<M: YamlDecode + ?Sized>(
        &mut self,
        parent: NodeId,
        merge: NodeId,
        out: &mut M,
    ) -> Result<(), UnmarshalError> {
        let merged = self.merged;
        if merged.is_none() {
            let set = self.new_merge_set();
            self.merged = Some(set);
            for &kn in self.content(parent).iter().step_by(2) {
                let mut k = Value::Nil;
                if self.unmarshal(kn, &mut k)?
                    && let Some(key) = Self::merge_key(&k)?
                {
                    let _new = self.merge_insert(set, key);
                }
            }
        }

        let node = self.node(merge);
        match node.kind {
            Kind::Mapping => {
                let _good = self.unmarshal(merge, out)?;
            }
            Kind::Alias => {
                if self.node(node.alias).kind != Kind::Mapping {
                    return Err(fail_want_map());
                }
                let _good = self.unmarshal(merge, out)?;
            }
            Kind::Sequence => {
                for &ni in &node.content {
                    let item = self.node(ni);
                    if item.kind == Kind::Alias {
                        if self.node(item.alias).kind != Kind::Mapping {
                            return Err(fail_want_map());
                        }
                    } else if item.kind != Kind::Mapping {
                        return Err(fail_want_map());
                    }
                    let _good = self.unmarshal(ni, out)?;
                }
            }
            Kind::Scalar | Kind::Document => return Err(fail_want_map()),
        }

        self.merged = merged;
        Ok(())
    }
}

/// A field of a struct's decoding plan (Go: `fieldInfo`).
#[derive(Clone, Debug)]
pub(super) struct FieldInfo {
    /// Go: `Key`.
    key: String,
    /// Go: `Id`.
    id: usize,
    /// Go: `Num` for a direct field, or `Inline`: the field indexes through inlined structs.
    path: Vec<usize>,
}

/// Go: `structInfo`: how a struct's fields are decoded.
#[derive(Clone, Debug)]
pub struct StructInfo {
    /// Go: `FieldsList`.
    fields: Vec<FieldInfo>,
    /// Go: `FieldsMap`, indexing `fields`.
    by_key: HashMap<String, usize>,
    /// Whether the struct has a `,inline` map (Go: `InlineMap != -1`).
    inline_map: bool,
}

/// A struct field as [`#[derive(GoYaml)]`](crate::GoYaml) describes it to
/// [`StructInfo::build`].
#[derive(Clone, Copy, Debug)]
pub enum FieldDesc {
    /// A field decoded under `key` (the tag name, or the lowercased Go field name).
    Field {
        /// The field's index.
        num: usize,
        /// The mapping key.
        key: &'static str,
    },
    /// A `,inline` struct (or pointer to struct) field, whose fields join this struct's.
    Inline {
        /// The field's index.
        num: usize,
        /// The inlined struct's plan.
        info: fn() -> Result<&'static StructInfo, UnmarshalError>,
    },
    /// The `,inline` map field, which receives the keys no field takes.
    InlineMap,
}

impl StructInfo {
    /// Go: `getStructInfo` for the struct named `type_name` with the given fields.
    ///
    /// # Errors
    ///
    /// Go's panic texts: `duplicated key 'k' in struct T` and `multiple ,inline maps in struct
    /// T`, as [`UnmarshalError::Panic`].
    pub fn build(type_name: &str, descs: &[FieldDesc]) -> Result<Self, UnmarshalError> {
        let mut info = Self {
            fields: Vec::new(),
            by_key: HashMap::new(),
            inline_map: false,
        };
        for desc in descs {
            match *desc {
                FieldDesc::InlineMap => {
                    if info.inline_map {
                        return Err(UnmarshalError::Panic(format!(
                            "multiple ,inline maps in struct {type_name}"
                        )));
                    }
                    info.inline_map = true;
                }
                FieldDesc::Inline { num, info: inner } => {
                    for field in &inner()?.fields {
                        let mut path = vec![num];
                        path.extend_from_slice(&field.path);
                        info.push(type_name, field.key.clone(), path)?;
                    }
                }
                FieldDesc::Field { num, key } => info.push(type_name, key.to_owned(), vec![num])?,
            }
        }
        Ok(info)
    }

    /// Appends a field, rejecting a duplicate key.
    fn push(
        &mut self,
        type_name: &str,
        key: String,
        path: Vec<usize>,
    ) -> Result<(), UnmarshalError> {
        if self.by_key.contains_key(&key) {
            return Err(UnmarshalError::Panic(format!(
                "duplicated key '{key}' in struct {type_name}"
            )));
        }
        let id = self.fields.len();
        let _previous = self.by_key.insert(key.clone(), id);
        self.fields.push(FieldInfo { key, id, path });
        Ok(())
    }

    /// The field decoded under `key`.
    fn field(&self, key: &str) -> Option<&FieldInfo> {
        self.by_key.get(key).and_then(|&i| self.fields.get(i))
    }
}

/// A struct decoded from a mapping (Go: `d.mappingStruct`), implemented by
/// [`#[derive(GoYaml)]`](crate::GoYaml).
pub trait YamlStruct: YamlDecode {
    /// Go: `getStructInfo` of the type.
    ///
    /// # Errors
    ///
    /// The [`UnmarshalError::Panic`] of an invalid struct definition.
    fn yaml_struct_info() -> Result<&'static StructInfo, UnmarshalError>;

    /// Decodes node `n` into the field at `path` (Go: `out.Field(num)`, or `fieldByIndex`
    /// through inlined structs, allocating nil pointers).
    ///
    /// # Errors
    ///
    /// A fatal error of the field.
    fn yaml_field(
        &mut self,
        d: &mut Decoder<'_>,
        path: &[usize],
        n: NodeId,
    ) -> Result<(), UnmarshalError>;

    /// Go: the `,inline` map's `SetMapIndex(name, value)`.
    ///
    /// # Errors
    ///
    /// A fatal error of the value.
    fn yaml_inline_map(
        &mut self,
        d: &mut Decoder<'_>,
        key: String,
        n: NodeId,
    ) -> Result<(), UnmarshalError> {
        drop(key);
        let _ = (d, n);
        Ok(())
    }
}

/// A `,inline` struct field: a struct, or a pointer to one.
pub trait YamlInline {
    /// The inlined struct's plan.
    ///
    /// # Errors
    ///
    /// The [`UnmarshalError::Panic`] of an invalid struct definition.
    fn yaml_inline_info() -> Result<&'static StructInfo, UnmarshalError>;

    /// Decodes into the inlined field at `path`, allocating a nil pointer first.
    ///
    /// # Errors
    ///
    /// A fatal error of the field.
    fn yaml_inline_field(
        &mut self,
        d: &mut Decoder<'_>,
        path: &[usize],
        n: NodeId,
    ) -> Result<(), UnmarshalError>;
}

impl<T: YamlInline + Default> YamlInline for Option<T> {
    fn yaml_inline_info() -> Result<&'static StructInfo, UnmarshalError> {
        T::yaml_inline_info()
    }

    fn yaml_inline_field(
        &mut self,
        d: &mut Decoder<'_>,
        path: &[usize],
        n: NodeId,
    ) -> Result<(), UnmarshalError> {
        self.get_or_insert_with(T::default)
            .yaml_inline_field(d, path, n)
    }
}

impl<T: YamlInline + ?Sized> YamlInline for Box<T> {
    fn yaml_inline_info() -> Result<&'static StructInfo, UnmarshalError> {
        T::yaml_inline_info()
    }

    fn yaml_inline_field(
        &mut self,
        d: &mut Decoder<'_>,
        path: &[usize],
        n: NodeId,
    ) -> Result<(), UnmarshalError> {
        (**self).yaml_inline_field(d, path, n)
    }
}

/// A `,inline` map field: a map with `string` keys (Go rejects other key types).
pub trait YamlInlineMap {
    /// Go: `inlineMap.SetMapIndex(name, value)` with the decoded value, allocating a nil map.
    ///
    /// # Errors
    ///
    /// A fatal error of the value.
    fn yaml_inline_insert(
        &mut self,
        d: &mut Decoder<'_>,
        key: String,
        n: NodeId,
    ) -> Result<(), UnmarshalError>;
}

impl<V: YamlDecode + Default> YamlInlineMap for BTreeMap<String, V> {
    fn yaml_inline_insert(
        &mut self,
        d: &mut Decoder<'_>,
        key: String,
        n: NodeId,
    ) -> Result<(), UnmarshalError> {
        let mut value = V::default();
        let _good = d.unmarshal(n, &mut value)?;
        let _previous = self.insert(key, value);
        Ok(())
    }
}

impl<V: YamlDecode + Default, S: BuildHasher + Default> YamlInlineMap for HashMap<String, V, S> {
    fn yaml_inline_insert(
        &mut self,
        d: &mut Decoder<'_>,
        key: String,
        n: NodeId,
    ) -> Result<(), UnmarshalError> {
        let mut value = V::default();
        let _good = d.unmarshal(n, &mut value)?;
        let _previous = self.insert(key, value);
        Ok(())
    }
}

impl<M: YamlInlineMap + Default> YamlInlineMap for Option<M> {
    fn yaml_inline_insert(
        &mut self,
        d: &mut Decoder<'_>,
        key: String,
        n: NodeId,
    ) -> Result<(), UnmarshalError> {
        self.get_or_insert_with(M::default)
            .yaml_inline_insert(d, key, n)
    }
}

/// Integer impls: the Go kind and name, the bit size, and the `interface{}` value of a key.
macro_rules! int_impls {
    ($(
        $ty:ty => $kind:ident,
        $name:literal,
        $bits:literal,
        $method:ident,
        $v:ident => $value:expr;
    )*) => {
        $(
            impl YamlDecode for $ty {
                const YAML_KIND: GoKind = GoKind::$kind;

                fn yaml_type_name(out: &mut String) {
                    out.push_str($name);
                }

                fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
                    let Some(v) = s.$method($bits).and_then(|v| <$ty>::try_from(v).ok()) else {
                        return Ok(false);
                    };
                    *self = v;
                    Ok(true)
                }

                fn yaml_key_interface(&self) -> Option<(String, Value)> {
                    let $v = *self;
                    Some(($name.to_owned(), $value?))
                }
            }
        )*
    };
}

int_impls! {
    i8 => Int8, "int8", 8, int, v => Some(Value::Int8(v));
    i16 => Int16, "int16", 16, int, v => Some(Value::Int16(v));
    i32 => Int32, "int32", 32, int, v => Some(Value::Int32(v));
    i64 => Int64, "int64", 64, int, v => Some(Value::Int64(v));
    isize => Int, "int", 64, int, v => i64::try_from(v).ok().map(Value::Int);
    u8 => Uint8, "uint8", 8, uint, v => Some(Value::Uint8(v));
    u16 => Uint16, "uint16", 16, uint, v => Some(Value::Uint16(v));
    u32 => Uint32, "uint32", 32, uint, v => Some(Value::Uint32(v));
    u64 => Uint64, "uint64", 64, uint, v => Some(Value::Uint64(v));
    usize => Uint, "uint", 64, uint, v => u64::try_from(v).ok().map(Value::Uint);
}

impl YamlDecode for bool {
    const YAML_KIND: GoKind = GoKind::Bool;

    fn yaml_type_name(out: &mut String) {
        out.push_str("bool");
    }

    fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
        let Some(v) = s.bool() else {
            return Ok(false);
        };
        *self = v;
        Ok(true)
    }

    fn yaml_key_interface(&self) -> Option<(String, Value)> {
        Some(("bool".to_owned(), Value::Bool(*self)))
    }
}

impl YamlDecode for f64 {
    const YAML_KIND: GoKind = GoKind::Float64;

    fn yaml_type_name(out: &mut String) {
        out.push_str("float64");
    }

    fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
        let Some(v) = s.float() else {
            return Ok(false);
        };
        *self = v;
        Ok(true)
    }

    fn yaml_key_interface(&self) -> Option<(String, Value)> {
        Some(("float64".to_owned(), Value::Float64(*self)))
    }
}

impl YamlDecode for f32 {
    const YAML_KIND: GoKind = GoKind::Float32;

    fn yaml_type_name(out: &mut String) {
        out.push_str("float32");
    }

    fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
        let Some(v) = s.float() else {
            return Ok(false);
        };
        // Go: SetFloat converts to float32 (no overflow check).
        *self = math::float32(v);
        Ok(true)
    }

    fn yaml_key_interface(&self) -> Option<(String, Value)> {
        Some(("float32".to_owned(), Value::Float32(*self)))
    }
}

impl YamlDecode for String {
    const YAML_KIND: GoKind = GoKind::String;

    fn yaml_type_name(out: &mut String) {
        out.push_str("string");
    }

    fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
        *self = Self::from_utf8_lossy(&s.string_bytes()).into_owned();
        Ok(true)
    }

    fn yaml_key_interface(&self) -> Option<(String, Value)> {
        Some(("string".to_owned(), Value::String(self.clone())))
    }
}

impl YamlDecode for GoString {
    const YAML_KIND: GoKind = GoKind::String;

    fn yaml_type_name(out: &mut String) {
        out.push_str("string");
    }

    fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
        *self = Self::from_bytes(s.string_bytes().into_owned());
        Ok(true)
    }

    fn yaml_key_interface(&self) -> Option<(String, Value)> {
        Some(("string".to_owned(), Value::String(self.to_string_lossy())))
    }
}

impl YamlDecode for Duration {
    const YAML_KIND: GoKind = GoKind::Int64;

    fn yaml_type_name(out: &mut String) {
        out.push_str("time.Duration");
    }

    fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
        // Go: a time.Duration only decodes from a string, through time.ParseDuration.
        if let Some(text) = s.resolved_string()
            && let Ok(d) = parse_duration(&text)
        {
            *self = d;
            return Ok(true);
        }
        Ok(false)
    }

    fn yaml_key_interface(&self) -> Option<(String, Value)> {
        Some(("time.Duration".to_owned(), Value::Duration(*self)))
    }
}

impl YamlDecode for Time {
    const YAML_KIND: GoKind = GoKind::Struct;

    fn yaml_type_name(out: &mut String) {
        out.push_str("time.Time");
    }

    fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
        if let Resolved::Time(t) = &s.resolved {
            *self = t.clone();
            return Ok(true);
        }
        // Go: *time.Time is an encoding.TextUnmarshaler; its error is returned as is.
        let text = String::from_utf8_lossy(&s.string_bytes()).into_owned();
        match Self::unmarshal_text(&text) {
            Ok(t) => {
                *self = t;
                Ok(true)
            }
            Err(e) => Err(UnmarshalError::Fatal(e.to_string().into_bytes())),
        }
    }

    fn yaml_mapping(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        // Go: a time.Time has no exported fields, so a mapping sets nothing.
        d.decode_struct(n, self).map(Some)
    }

    fn yaml_key_interface(&self) -> Option<(String, Value)> {
        Some(("time.Time".to_owned(), Value::Time(self.clone())))
    }
}

impl YamlStruct for Time {
    fn yaml_struct_info() -> Result<&'static StructInfo, UnmarshalError> {
        static INFO: OnceLock<StructInfo> = OnceLock::new();
        Ok(INFO.get_or_init(|| StructInfo {
            fields: Vec::new(),
            by_key: HashMap::new(),
            inline_map: false,
        }))
    }

    fn yaml_field(
        &mut self,
        _: &mut Decoder<'_>,
        _: &[usize],
        _: NodeId,
    ) -> Result<(), UnmarshalError> {
        Ok(())
    }
}

/// `map[string]interface {}`, as decoding into an `interface{}` makes it.
#[derive(Debug, Default)]
struct StringMap(BTreeMap<String, Value>);

/// `map[interface {}]interface {}`, as decoding into an `interface{}` makes it.
#[derive(Debug, Default)]
struct GeneralMap(Vec<(Value, Value)>);

impl YamlDecode for StringMap {
    const YAML_KIND: GoKind = GoKind::Map;

    fn yaml_type_name(out: &mut String) {
        out.push_str("map[string]interface {}");
    }

    fn yaml_mapping(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        d.decode_map(n, self, false).map(Some)
    }
}

impl MapTarget for StringMap {
    type Elem = Value;
    type Key = String;

    fn map_contains(&self, key: &String) -> bool {
        self.0.contains_key(key)
    }

    fn map_insert(&mut self, key: String, elem: Value) {
        let _previous = self.0.insert(key, elem);
    }
}

impl YamlDecode for GeneralMap {
    const YAML_KIND: GoKind = GoKind::Map;

    fn yaml_type_name(out: &mut String) {
        out.push_str("map[interface {}]interface {}");
    }

    fn yaml_mapping(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        d.decode_map(n, self, false).map(Some)
    }
}

impl MapTarget for GeneralMap {
    type Elem = Value;
    type Key = Value;

    fn map_contains(&self, key: &Value) -> bool {
        Value::general_map_contains(&self.0, key)
    }

    fn map_insert(&mut self, key: Value, elem: Value) {
        Value::general_map_insert(&mut self.0, key, elem);
    }
}

impl YamlDecode for Value {
    const YAML_KIND: GoKind = GoKind::Interface;

    fn yaml_type_name(out: &mut String) {
        out.push_str("interface {}");
    }

    fn yaml_null(&mut self) -> bool {
        *self = Self::Nil;
        true
    }

    fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
        *self = s.interface();
        Ok(true)
    }

    fn yaml_sequence(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        let mut items = Vec::new();
        for &c in d.content(n) {
            let mut e = Self::Nil;
            if d.unmarshal(c, &mut e)? {
                items.push(e);
            }
        }
        *self = Self::Slice(&Type::Interface, Some(items));
        Ok(Some(true))
    }

    fn yaml_mapping(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        // Go stores the new map in the interface before decoding into it, so a fatal error
        // leaves the entries decoded so far.
        if d.is_string_map(n) {
            let mut map = StringMap::default();
            let result = d.decode_map(n, &mut map, false);
            *self = Self::StringMap(&Type::Interface, Some(map.0));
            result.map(Some)
        } else {
            let mut map = GeneralMap::default();
            let result = d.decode_map(n, &mut map, false);
            *self = Self::Map(&Type::Interface, &Type::Interface, Some(map.0));
            result.map(Some)
        }
    }

    fn yaml_key_interface(&self) -> Option<(String, Value)> {
        Some((self.type_name(), self.clone()))
    }

    fn yaml_key_kind(&self) -> Option<GoKind> {
        self.kind()
    }
}

impl<T: YamlDecode + Default> YamlDecode for Vec<T> {
    const YAML_KIND: GoKind = GoKind::Slice;
    const YAML_OPTION_IS_PTR: bool = false;

    fn yaml_type_name(out: &mut String) {
        out.push_str("[]");
        T::yaml_type_name(out);
    }

    fn yaml_null(&mut self) -> bool {
        self.clear();
        true
    }

    fn yaml_sequence(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        // Go: a new slice of one zero element per item, the items that decode stored from the
        // front, then cut to their number; a fatal error leaves the full-length slice.
        let content = d.content(n);
        self.clear();
        self.resize_with(content.len(), T::default);
        let mut j = 0;
        for &c in content {
            let mut e = T::default();
            if d.unmarshal(c, &mut e)? {
                if let Some(slot) = self.get_mut(j) {
                    *slot = e;
                }
                j += 1;
            }
        }
        self.truncate(j);
        Ok(Some(true))
    }
}

impl<K, V> YamlDecode for BTreeMap<K, V>
where
    K: YamlDecode + Default + Ord,
    V: YamlDecode + Default,
{
    const YAML_KIND: GoKind = GoKind::Map;
    const YAML_OPTION_IS_PTR: bool = false;

    fn yaml_type_name(out: &mut String) {
        out.push_str("map[");
        K::yaml_type_name(out);
        out.push(']');
        V::yaml_type_name(out);
    }

    fn yaml_null(&mut self) -> bool {
        self.clear();
        true
    }

    fn yaml_mapping(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        let was_nil = self.is_empty();
        self.yaml_mapping_nil(d, n, was_nil)
    }

    fn yaml_mapping_nil(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
        was_nil: bool,
    ) -> Result<Option<bool>, UnmarshalError> {
        d.decode_map(n, self, was_nil).map(Some)
    }
}

impl<K, V> MapTarget for BTreeMap<K, V>
where
    K: YamlDecode + Default + Ord,
    V: YamlDecode + Default,
{
    type Elem = V;
    type Key = K;

    fn map_contains(&self, key: &K) -> bool {
        self.contains_key(key)
    }

    fn map_insert(&mut self, key: K, elem: V) {
        let _previous = self.insert(key, elem);
    }
}

impl<K, V, S> YamlDecode for HashMap<K, V, S>
where
    K: YamlDecode + Default + Eq + Hash,
    V: YamlDecode + Default,
    S: BuildHasher + Default,
{
    const YAML_KIND: GoKind = GoKind::Map;
    const YAML_OPTION_IS_PTR: bool = false;

    fn yaml_type_name(out: &mut String) {
        out.push_str("map[");
        K::yaml_type_name(out);
        out.push(']');
        V::yaml_type_name(out);
    }

    fn yaml_null(&mut self) -> bool {
        self.clear();
        true
    }

    fn yaml_mapping(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        let was_nil = self.is_empty();
        self.yaml_mapping_nil(d, n, was_nil)
    }

    fn yaml_mapping_nil(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
        was_nil: bool,
    ) -> Result<Option<bool>, UnmarshalError> {
        d.decode_map(n, self, was_nil).map(Some)
    }
}

impl<K, V, S> MapTarget for HashMap<K, V, S>
where
    K: YamlDecode + Default + Eq + Hash,
    V: YamlDecode + Default,
    S: BuildHasher + Default,
{
    type Elem = V;
    type Key = K;

    fn map_contains(&self, key: &K) -> bool {
        self.contains_key(key)
    }

    fn map_insert(&mut self, key: K, elem: V) {
        let _previous = self.insert(key, elem);
    }
}

impl<T: YamlDecode + Default> YamlDecode for Option<T> {
    const YAML_KIND: GoKind = if T::YAML_OPTION_IS_PTR {
        GoKind::Ptr
    } else {
        T::YAML_KIND
    };

    fn yaml_type_name(out: &mut String) {
        if T::YAML_OPTION_IS_PTR {
            out.push('*');
        }
        T::yaml_type_name(out);
    }

    fn yaml_decode(&mut self, d: &mut Decoder<'_>, n: NodeId) -> Result<bool, UnmarshalError> {
        if T::YAML_OPTION_IS_PTR && !d.is_null(n) {
            // Go: prepare allocates a nil pointer and decodes into its element.
            return self.get_or_insert_with(T::default).yaml_decode(d, n);
        }
        d.dispatch(n, self)
    }

    fn yaml_null(&mut self) -> bool {
        *self = None;
        true
    }

    fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
        if T::YAML_OPTION_IS_PTR {
            return Err(UnmarshalError::Panic(
                "yaml internal error: please report the issue".to_owned(),
            ));
        }
        let mut inner = T::default();
        let good = inner.yaml_scalar(s)?;
        if good {
            *self = Some(inner);
        }
        Ok(good)
    }

    fn yaml_sequence(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        if T::YAML_OPTION_IS_PTR {
            return Ok(None);
        }
        let was_nil = self.is_none();
        let mut inner = self.take().unwrap_or_default();
        let result = inner.yaml_sequence(d, n);
        // A nil slice or map stays nil when the node's kind does not fit it (a type error).
        if !was_nil || !matches!(result, Ok(None)) {
            *self = Some(inner);
        }
        result
    }

    fn yaml_mapping(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        if T::YAML_OPTION_IS_PTR {
            return Ok(None);
        }
        let was_nil = self.is_none();
        let mut inner = self.take().unwrap_or_default();
        let result = inner.yaml_mapping_nil(d, n, was_nil);
        if !was_nil || !matches!(result, Ok(None)) {
            *self = Some(inner);
        }
        result
    }
}

impl<T: YamlDecode + ?Sized> YamlDecode for Box<T> {
    const YAML_KIND: GoKind = T::YAML_KIND;
    const YAML_OPTION_IS_PTR: bool = T::YAML_OPTION_IS_PTR;

    fn yaml_type_name(out: &mut String) {
        T::yaml_type_name(out);
    }

    fn yaml_decode(&mut self, d: &mut Decoder<'_>, n: NodeId) -> Result<bool, UnmarshalError> {
        (**self).yaml_decode(d, n)
    }

    fn yaml_null(&mut self) -> bool {
        (**self).yaml_null()
    }

    fn yaml_scalar(&mut self, s: &Scalar<'_>) -> Result<bool, UnmarshalError> {
        (**self).yaml_scalar(s)
    }

    fn yaml_sequence(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        (**self).yaml_sequence(d, n)
    }

    fn yaml_mapping(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
    ) -> Result<Option<bool>, UnmarshalError> {
        (**self).yaml_mapping(d, n)
    }

    fn yaml_mapping_nil(
        &mut self,
        d: &mut Decoder<'_>,
        n: NodeId,
        was_nil: bool,
    ) -> Result<Option<bool>, UnmarshalError> {
        (**self).yaml_mapping_nil(d, n, was_nil)
    }

    fn yaml_key_interface(&self) -> Option<(String, Value)> {
        (**self).yaml_key_interface()
    }

    fn yaml_key_kind(&self) -> Option<GoKind> {
        (**self).yaml_key_kind()
    }
}
