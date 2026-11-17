//! Go named string types (`type BidderName string`) whose constants are Rust constants.
//!
//! A `String` newtype cannot be built in a `const`, so these types hold a
//! `Cow<'static, str>`: constants borrow their literal, decoded values own their text. They
//! encode and decode as Go `string`s under their own Go type name (pbs-gojson's newtype
//! semantics, implemented here by hand because the derive does not know `Cow`).

/// Declares a Go named string type over `Cow<'static, str>`, with its pbs-gojson impls; the
/// items after the header go into its inherent `impl` block. The invoking module must have
/// `std::borrow::Cow` in scope.
macro_rules! cow_string_type {
    (
        $(#[$meta:meta])*
        pub struct $name:ident = $go_type:literal;
        $($body:tt)*
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub Cow<'static, str>);

        impl $name {
            #[doc = concat!("A `", $go_type, "` of a string literal (a Go constant).")]
            #[must_use]
            pub const fn from_static(s: &'static str) -> Self {
                Self(Cow::Borrowed(s))
            }

            /// The value as a Go `string` (`string(v)`).
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            $($body)*
        }

        const _: () = {
            use std::{borrow::Borrow, fmt, mem};

            use pbs_gojson::{
                Encoder, GoEncode, GoKind, GoMapKey, GoType, JsonError, JsoniterDecode, StdDecode,
                jsoniter::Iter, stdjson::StdDecoder,
            };

            /// Runs `f` on the owned text of `cow`, keeping what `f` leaves in it.
            fn with_string<R, F: FnOnce(&mut String) -> R>(
                cow: &mut Cow<'static, str>,
                f: F,
            ) -> R {
                let mut s = mem::take(cow).into_owned();
                let result = f(&mut s);
                *cow = Cow::Owned(s);
                result
            }

            impl From<&str> for $name {
                fn from(s: &str) -> Self {
                    Self(Cow::Owned(s.to_owned()))
                }
            }

            impl From<String> for $name {
                fn from(s: String) -> Self {
                    Self(Cow::Owned(s))
                }
            }

            impl AsRef<str> for $name {
                fn as_ref(&self) -> &str {
                    &self.0
                }
            }

            impl Borrow<str> for $name {
                fn borrow(&self) -> &str {
                    &self.0
                }
            }

            impl PartialEq<str> for $name {
                fn eq(&self, other: &str) -> bool {
                    *self.0 == *other
                }
            }

            impl PartialEq<&str> for $name {
                fn eq(&self, other: &&str) -> bool {
                    *self.0 == **other
                }
            }

            impl fmt::Display for $name {
                fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str(&self.0)
                }
            }

            impl GoType for $name {
                const KIND: GoKind = GoKind::String;

                fn go_type_name(out: &mut String) {
                    out.push_str($go_type);
                }
            }

            impl GoEncode for $name {
                fn go_encode(&self, enc: &mut Encoder) {
                    GoEncode::go_encode(&*self.0, enc);
                }

                fn go_is_empty(&self) -> bool {
                    self.0.is_empty()
                }

                fn go_encode_quoted(&self, enc: &mut Encoder) {
                    GoEncode::go_encode_quoted(&*self.0, enc);
                }
            }

            impl GoMapKey for $name {
                fn key_text(&self) -> Cow<'_, [u8]> {
                    Cow::Borrowed(self.0.as_bytes())
                }

                fn std_parse_key(key: &[u8]) -> Option<Self> {
                    <String as GoMapKey>::std_parse_key(key).map(Self::from)
                }

                fn jsoniter_decode_key(it: &mut Iter<'_>) -> Self {
                    Self::from(<String as GoMapKey>::jsoniter_decode_key(it))
                }
            }

            impl JsoniterDecode for $name {
                fn jsoniter_decode(&mut self, it: &mut Iter<'_>) {
                    with_string(&mut self.0, |s| s.jsoniter_decode(it));
                }

                fn jsoniter_decode_string_mode(&mut self, it: &mut Iter<'_>) {
                    with_string(&mut self.0, |s| s.jsoniter_decode_string_mode(it));
                }
            }

            impl StdDecode for $name {
                fn std_decode_as<N: GoType + ?Sized>(
                    &mut self,
                    d: &mut StdDecoder<'_>,
                ) -> Result<(), JsonError> {
                    with_string(&mut self.0, |s| s.std_decode_as::<N>(d))
                }

                fn std_literal_as<N: GoType + ?Sized>(
                    &mut self,
                    item: &[u8],
                    from_quoted: bool,
                    d: &mut StdDecoder<'_>,
                ) -> Result<(), JsonError> {
                    with_string(&mut self.0, |s| s.std_literal_as::<N>(item, from_quoted, d))
                }
            }
        };
    };
}

pub(super) use cow_string_type;
