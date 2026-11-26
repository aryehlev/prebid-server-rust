//! Go `util/ptrutil`: helpers for optional values, which Go models as pointers.
//!
//! A Go `*T` field of a request or config struct is an `Option<T>` in Rust, so `ToPtr(v)` is
//! `Some(v)` and `Clone(p)` is `p.clone()`. The functions below cover the helpers whose Go
//! semantics are not already a one-word Rust method.

/// Go: `ptrutil.ToPtr`: `Some(v)`.
#[must_use]
pub const fn to_ptr<T>(v: T) -> Option<T> {
    Some(v)
}

/// Go: `ptrutil.Clone`: a copy of the pointed-to value, or `None` for nil.
#[must_use]
pub fn clone<T: Clone>(v: Option<&T>) -> Option<T> {
    v.cloned()
}

/// Go: `ptrutil.ValueOrDefault`: the value, or the zero value of `T` for nil.
#[must_use]
pub fn value_or_default<T: Clone + Default>(v: Option<&T>) -> T {
    v.cloned().unwrap_or_default()
}

/// Go: `ptrutil.Equal`: both nil, or both set to equal values.
#[must_use]
pub fn equal<T: PartialEq + ?Sized>(v1: Option<&T>, v2: Option<&T>) -> bool {
    v1 == v2
}

#[cfg(test)]
mod tests {
    use super::{clone, equal, to_ptr, value_or_default};
    use crate::golden::{TestResult, check_eq};

    #[test]
    fn value_or_default_go_cases() -> TestResult {
        // Go: TestValueOrDefault.
        check_eq(&value_or_default::<i64>(None), &0, "int-nil")?;
        check_eq(&value_or_default(to_ptr(0_i64).as_ref()), &0, "int-0")?;
        check_eq(&value_or_default(to_ptr(42_i64).as_ref()), &42, "int-42")?;
        check_eq(
            &value_or_default::<String>(None),
            &String::new(),
            "string-nil",
        )?;
        check_eq(
            &value_or_default(to_ptr(String::new()).as_ref()),
            &String::new(),
            "string-empty",
        )?;
        check_eq(
            &value_or_default(to_ptr("something".to_owned()).as_ref()),
            &"something".to_owned(),
            "string-something",
        )
    }

    #[test]
    fn equal_go_cases() -> TestResult {
        // Go: TestEqualInt and TestEqualString.
        let ints = [
            ("nil", None, None, true),
            ("nil-value1", None, Some(42), false),
            ("nil-value2", Some(42), None, false),
            ("same", Some(42), Some(42), true),
            ("different", Some(42), Some(24), false),
        ];
        for (name, a, b, want) in ints {
            check_eq(&equal(a.as_ref(), b.as_ref()), &want, name)?;
        }
        let strings = [
            ("nil", None, None, true),
            ("nil-value1", None, Some("hello"), false),
            ("nil-value2", Some("hello"), None, false),
            ("same", Some("hello"), Some("hello"), true),
            ("different", Some("hello"), Some("world"), false),
            ("empty", Some(""), Some(""), true),
        ];
        for (name, a, b, want) in strings {
            check_eq(&equal(a, b), &want, name)?;
        }
        Ok(())
    }

    #[test]
    fn clone_copies_or_stays_nil() -> TestResult {
        // Go: ptrutil.Clone (util/ptrutil/ptrutil.go:7): nil stays nil, a value is copied.
        check_eq(&clone::<i64>(None), &None, "nil")?;
        check_eq(
            &clone(Some(&"x".to_owned())),
            &Some("x".to_owned()),
            "value",
        )
    }
}
