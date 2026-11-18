//! Go's checks of an ECDSA public key when it parses a certificate
//! (`ecdsa.ParseUncompressedPublicKey`, then `crypto/internal/fips140/nistec`'s `SetBytes`): an
//! uncompressed point of the curve's length, coordinates below the field prime, and the point on
//! the curve (y² = x³ − 3x + b).
//!
//! The arithmetic is plain double-and-add over 576-bit numbers: a key is checked once per parse,
//! and nothing here is secret.

use std::cmp::Ordering;

/// A number of up to 576 bits, as little-endian 64-bit limbs.
type Limbs = [u64; LIMBS];

const LIMBS: usize = 9;

/// A curve Go's `x509` parses ECDSA keys on (Go: `namedCurveFromOID`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Curve {
    P224,
    P256,
    P384,
    P521,
}

impl Curve {
    /// The curve named by an OID's DER contents.
    pub(super) const fn from_oid(oid: &[u8]) -> Option<Self> {
        match oid {
            [0x2B, 0x81, 0x04, 0x00, 0x21] => Some(Self::P224),
            [0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07] => Some(Self::P256),
            [0x2B, 0x81, 0x04, 0x00, 0x22] => Some(Self::P384),
            [0x2B, 0x81, 0x04, 0x00, 0x23] => Some(Self::P521),
            _ => None,
        }
    }

    /// The bytes of one coordinate.
    const fn element_len(self) -> usize {
        match self {
            Self::P224 => 28,
            Self::P256 => 32,
            Self::P384 => 48,
            Self::P521 => 66,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::P224 => "P224",
            Self::P256 => "P256",
            Self::P384 => "P384",
            Self::P521 => "P521",
        }
    }

    /// The field prime, big-endian hex of the element length.
    const fn prime(self) -> &'static str {
        match self {
            Self::P224 => "ffffffffffffffffffffffffffffffff000000000000000000000001",
            Self::P256 => "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff",
            Self::P384 => {
                "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff00000000\
                 00000000ffffffff"
            }
            Self::P521 => {
                "01ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff\
                 ffffffffffffffffffffffffffffffffffffffffffffffffff"
            }
        }
    }

    /// The curve's `b`, big-endian hex.
    const fn b(self) -> &'static str {
        match self {
            Self::P224 => "b4050a850c04b3abf54132565044b0b7d7bfd8ba270b39432355ffb4",
            Self::P256 => "5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b",
            Self::P384 => {
                "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a\
                 85c8edd3ec2aef"
            }
            Self::P521 => {
                "0051953eb9618e1c9a1f929a21a0b68540eea2da725b99b315f3b8b489918ef109e156193951ec7e93\
                 7b1652c0bd3bb1bf073573df883d2c34f1ef451fd46b503f00"
            }
        }
    }

    /// Go's error for a coordinate that is not below the prime: P-256 has its own assembly
    /// implementation (amd64 and arm64), the other curves use fiat's `SetBytes`.
    fn element_error(self) -> String {
        match self {
            Self::P256 => "invalid P256 element encoding".to_owned(),
            Self::P224 | Self::P384 | Self::P521 => {
                format!("invalid {}Element encoding", self.name())
            }
        }
    }
}

/// Go: `ecdsa.ParseUncompressedPublicKey(curve, data)`.
pub(super) fn check_public_key(curve: Curve, data: &[u8]) -> Result<(), String> {
    let Some((&4, coordinates)) = data.split_first() else {
        return Err("ecdsa: invalid uncompressed public key".to_owned());
    };
    let len = curve.element_len();
    if coordinates.len() != 2 * len {
        return Err(format!("invalid {} point encoding", curve.name()));
    }
    let (x, y) = coordinates.split_at(len);
    let prime = hex(curve.prime());
    if x >= prime.as_slice() || y >= prime.as_slice() {
        return Err(curve.element_error());
    }
    let p = limbs(&prime);
    let x = limbs(x);
    let y = limbs(y);
    let x3 = mul_mod(&mul_mod(&x, &x, &p), &x, &p);
    let three_x = add_mod(&add_mod(&x, &x, &p), &x, &p);
    let rhs = add_mod(&sub_mod(&x3, &three_x, &p), &limbs(&hex(curve.b())), &p);
    if mul_mod(&y, &y, &p) == rhs {
        Ok(())
    } else {
        Err(format!("{} point not on curve", curve.name()))
    }
}

/// Lower-case hex to bytes.
fn hex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .chunks(2)
        .map(|pair| pair.iter().fold(0_u8, |n, &c| (n << 4) | hex_digit(c)))
        .collect()
}

const fn hex_digit(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => 0,
    }
}

/// A big-endian number of at most 72 bytes as limbs.
fn limbs(be: &[u8]) -> Limbs {
    let mut out = [0; LIMBS];
    for (limb, chunk) in out.iter_mut().zip(be.rchunks(8)) {
        *limb = chunk.iter().fold(0, |n, &b| (n << 8) | u64::from(b));
    }
    out
}

fn add(a: &Limbs, b: &Limbs) -> (Limbs, bool) {
    let mut out = [0; LIMBS];
    let mut carry = false;
    for (o, (x, y)) in out.iter_mut().zip(a.iter().zip(b)) {
        let (sum, c1) = x.overflowing_add(*y);
        let (sum, c2) = sum.overflowing_add(u64::from(carry));
        *o = sum;
        carry = c1 || c2;
    }
    (out, carry)
}

fn sub(a: &Limbs, b: &Limbs) -> (Limbs, bool) {
    let mut out = [0; LIMBS];
    let mut borrow = false;
    for (o, (x, y)) in out.iter_mut().zip(a.iter().zip(b)) {
        let (diff, b1) = x.overflowing_sub(*y);
        let (diff, b2) = diff.overflowing_sub(u64::from(borrow));
        *o = diff;
        borrow = b1 || b2;
    }
    (out, borrow)
}

fn below(a: &Limbs, b: &Limbs) -> bool {
    a.iter().rev().cmp(b.iter().rev()) == Ordering::Less
}

/// `a + b mod p` for `a, b < p`.
fn add_mod(a: &Limbs, b: &Limbs, p: &Limbs) -> Limbs {
    let (sum, carry) = add(a, b);
    if carry || !below(&sum, p) {
        sub(&sum, p).0
    } else {
        sum
    }
}

/// `a - b mod p` for `a, b < p`.
fn sub_mod(a: &Limbs, b: &Limbs, p: &Limbs) -> Limbs {
    let (diff, borrow) = sub(a, b);
    if borrow { add(&diff, p).0 } else { diff }
}

/// `a * b mod p` for `a, b < p`, by doubling and adding.
fn mul_mod(a: &Limbs, b: &Limbs, p: &Limbs) -> Limbs {
    let mut r = [0; LIMBS];
    for limb in b.iter().rev() {
        for bit in (0..64).rev() {
            r = add_mod(&r, &r, p);
            if (limb >> bit) & 1 == 1 {
                r = add_mod(&r, a, p);
            }
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::ensure_eq;

    /// A curve's base point, uncompressed (Go: `elliptic.Curve.Params().Gx` and `Gy`).
    fn generator(curve: Curve) -> Vec<u8> {
        let (x, y) = match curve {
            Curve::P224 => (
                "b70e0cbd6bb4bf7f321390b94a03c1d356c21122343280d6115c1d21",
                "bd376388b5f723fb4c22dfe6cd4375a05a07476444d5819985007e34",
            ),
            Curve::P256 => (
                "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296",
                "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5",
            ),
            Curve::P384 => (
                "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c\
                 3a545e3872760ab7",
                "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d\
                 7a431d7c90ea0e5f",
            ),
            Curve::P521 => (
                "00c6858e06b70404e9cd9e3ecb662395b4429c648139053fb521f828af606b4d3dbaa14b5e77efe7\
                 5928fe1dc127a2ffa8de3348b3c1856a429bf97e7e31c2e5bd66",
                "011839296a789a3bc0045c8a5fb42c7d1bd998f54449579b446817afbd17273e662c97ee72995ef4\
                 2640c550b9013fad0761353c7086a272c24088be94769fd16650",
            ),
        };
        let mut point = vec![4];
        point.extend(hex(x));
        point.extend(hex(y));
        point
    }

    /// Every curve's prime and `b` are right: its base point is on it, and only just.
    #[test]
    fn base_points() -> Result<(), String> {
        for curve in [Curve::P224, Curve::P256, Curve::P384, Curve::P521] {
            let point = generator(curve);
            ensure_eq(&point.len(), &(1 + 2 * curve.element_len()), curve.name())?;
            ensure_eq(
                &hex(curve.prime()).len(),
                &curve.element_len(),
                curve.name(),
            )?;
            ensure_eq(&hex(curve.b()).len(), &curve.element_len(), curve.name())?;
            ensure_eq(&check_public_key(curve, &point), &Ok(()), curve.name())?;
            let mut off = point;
            if let Some(last) = off.last_mut() {
                *last ^= 1;
            }
            ensure_eq(
                &check_public_key(curve, &off),
                &Err(format!("{} point not on curve", curve.name())),
                curve.name(),
            )?;
        }
        Ok(())
    }

    #[test]
    fn points() -> Result<(), String> {
        let generator = generator(Curve::P256);
        let mut big = vec![4];
        big.extend(hex(Curve::P384.prime()));
        big.extend(hex(Curve::P384.prime()));
        ensure_eq(
            &check_public_key(Curve::P384, &big),
            &Err("invalid P384Element encoding".to_owned()),
            "unreduced",
        )?;
        ensure_eq(
            &check_public_key(Curve::P521, &generator),
            &Err("invalid P521 point encoding".to_owned()),
            "length",
        )?;
        ensure_eq(
            &check_public_key(Curve::P224, &[2, 1]),
            &Err("ecdsa: invalid uncompressed public key".to_owned()),
            "compressed",
        )
    }
}
