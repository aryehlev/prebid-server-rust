package main

import (
	"crypto"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/md5"
	"crypto/rsa"
	"crypto/sha1"
	"crypto/sha256"
	"crypto/sha512"
	"crypto/x509"
	"encoding/binary"
	"fmt"
	"io"
	"math/big"
)

// drbg is a deterministic byte stream: SHA-256 in counter mode over a seed.
type drbg struct {
	seed    []byte
	counter uint64
	buf     []byte
}

func (d *drbg) Read(p []byte) (int, error) {
	for len(d.buf) < len(p) {
		var c [8]byte
		binary.BigEndian.PutUint64(c[:], d.counter)
		d.counter++
		h := sha256.Sum256(append(append([]byte{}, d.seed...), c[:]...))
		d.buf = append(d.buf, h[:]...)
	}
	n := copy(p, d.buf)
	d.buf = d.buf[n:]
	return n, nil
}

type keyPair struct {
	kind string // ecdsa256, ecdsa384, ecdsa521, rsa<bits>, ed25519
	priv crypto.Signer
	spki []byte
}

func detPrime(r io.Reader, bits int) *big.Int {
	b := make([]byte, (bits+7)/8)
	for {
		if _, err := io.ReadFull(r, b); err != nil {
			panic(err)
		}
		b[0] |= 0xC0
		b[len(b)-1] |= 1
		p := new(big.Int).SetBytes(b)
		if p.ProbablyPrime(20) {
			return p
		}
	}
}

func rsaKey(seed string, bits int) *rsa.PrivateKey {
	r := &drbg{seed: []byte("advgen rsa " + seed)}
	e := big.NewInt(65537)
	one := big.NewInt(1)
	for {
		p := detPrime(r, bits/2)
		q := detPrime(r, bits/2)
		if p.Cmp(q) == 0 {
			continue
		}
		pm1 := new(big.Int).Sub(p, one)
		qm1 := new(big.Int).Sub(q, one)
		if new(big.Int).GCD(nil, nil, e, pm1).Cmp(one) != 0 || new(big.Int).GCD(nil, nil, e, qm1).Cmp(one) != 0 {
			continue
		}
		n := new(big.Int).Mul(p, q)
		if n.BitLen() != bits {
			continue
		}
		phi := new(big.Int).Mul(pm1, qm1)
		d := new(big.Int).ModInverse(e, phi)
		k := &rsa.PrivateKey{PublicKey: rsa.PublicKey{N: n, E: 65537}, D: d, Primes: []*big.Int{p, q}}
		k.Precompute()
		if err := k.Validate(); err != nil {
			panic(err)
		}
		return k
	}
}

var keyCache = map[string]*keyPair{}

func newKey(seed, kind string) *keyPair {
	id := kind + "/" + seed
	if k, ok := keyCache[id]; ok {
		return k
	}
	var signer crypto.Signer
	switch kind {
	case "ecdsa256", "ecdsa384", "ecdsa521":
		var curve elliptic.Curve
		var size int
		switch kind {
		case "ecdsa256":
			curve, size = elliptic.P256(), 32
		case "ecdsa384":
			curve, size = elliptic.P384(), 48
		default:
			curve, size = elliptic.P521(), 66
		}
		h := sha512.Sum512([]byte("advgen ec " + seed))
		h2 := sha512.Sum512(h[:])
		scalar := append(h[:], h2[:]...)[:size]
		if kind == "ecdsa521" {
			scalar[0] &= 0x01
		}
		k, err := ecdsa.ParseRawPrivateKey(curve, scalar)
		if err != nil {
			panic(err)
		}
		signer = k
	case "ed25519":
		s := sha256.Sum256([]byte("advgen ed " + seed))
		signer = ed25519.NewKeyFromSeed(s[:])
	default:
		var bits int
		if _, err := fmt.Sscanf(kind, "rsa%d", &bits); err != nil {
			panic(kind)
		}
		signer = rsaKey(seed, bits)
	}
	spki, err := x509.MarshalPKIXPublicKey(signer.Public())
	if err != nil {
		panic(err)
	}
	k := &keyPair{kind: kind, priv: signer, spki: spki}
	keyCache[id] = k
	return k
}

// sigAlg is a signature algorithm: its AlgorithmIdentifier and how to sign.
type sigAlg struct {
	ai   []byte
	hash crypto.Hash // 0: Ed25519
	pss  bool
}

var algs = map[string]sigAlg{
	"ecdsa-sha256":      {seq(oid("1.2.840.10045.4.3.2")), crypto.SHA256, false},
	"ecdsa-sha384":      {seq(oid("1.2.840.10045.4.3.3")), crypto.SHA384, false},
	"ecdsa-sha512":      {seq(oid("1.2.840.10045.4.3.4")), crypto.SHA512, false},
	"ecdsa-sha1":        {seq(oid("1.2.840.10045.4.1")), crypto.SHA1, false},
	"rsa-sha256":        {seq(oid("1.2.840.113549.1.1.11"), null()), crypto.SHA256, false},
	"rsa-sha384":        {seq(oid("1.2.840.113549.1.1.12"), null()), crypto.SHA384, false},
	"rsa-sha512":        {seq(oid("1.2.840.113549.1.1.13"), null()), crypto.SHA512, false},
	"rsa-sha1":          {seq(oid("1.2.840.113549.1.1.5"), null()), crypto.SHA1, false},
	"rsa-md5":           {seq(oid("1.2.840.113549.1.1.4"), null()), crypto.MD5, false},
	"rsa-sha256-noparm": {seq(oid("1.2.840.113549.1.1.11")), crypto.SHA256, false},
	"ed25519":           {seq(oid("1.3.101.112")), 0, false},
	"rsa-pss-sha256":    {pssAI(pssHash(oidSHA256), pssMGF(oidSHA256), pssSalt(32)), crypto.SHA256, true},
	"rsa-pss-sha384":    {pssAI(pssHash(oidSHA384), pssMGF(oidSHA384), pssSalt(48)), crypto.SHA384, true},
	"rsa-pss-sha512":    {pssAI(pssHash(oidSHA512), pssMGF(oidSHA512), pssSalt(64)), crypto.SHA512, true},
}

const (
	oidSHA256 = "2.16.840.1.101.3.4.2.1"
	oidSHA384 = "2.16.840.1.101.3.4.2.2"
	oidSHA512 = "2.16.840.1.101.3.4.2.3"
)

// pssAI is an RSASSA-PSS AlgorithmIdentifier whose parameters SEQUENCE holds fields as given.
func pssAI(fields ...[]byte) []byte { return seq(oid("1.2.840.113549.1.1.10"), seq(fields...)) }

// pssHash is the [0] hashAlgorithm field.
func pssHash(hashOID string) []byte { return tlv(0xA0, seq(oid(hashOID), null())) }

// pssMGF is the [1] maskGenAlgorithm field: MGF1 with hashOID.
func pssMGF(hashOID string) []byte {
	return tlv(0xA1, seq(oid("1.2.840.113549.1.1.8"), seq(oid(hashOID), null())))
}

// pssSalt is the [2] saltLength field.
func pssSalt(n int64) []byte { return tlv(0xA2, integer(n)) }

// signPSS signs digest with RSASSA-PSS (RFC 8017, section 9.1.1): MGF1 with the message hash and
// a salt as long as the hash, derived from the digest so that the golden is reproducible
// (rsa.SignPSS draws the salt from a random source).
func signPSS(priv *rsa.PrivateKey, hash crypto.Hash, digest []byte) []byte {
	hLen := hash.Size()
	sh := hash.New()
	sh.Write([]byte("advgen pss salt"))
	sh.Write(digest)
	salt := sh.Sum(nil)[:hLen]
	emBits := priv.N.BitLen() - 1
	emLen := (emBits + 7) / 8
	mh := hash.New()
	mh.Write(make([]byte, 8))
	mh.Write(digest)
	mh.Write(salt)
	h := mh.Sum(nil)
	db := make([]byte, emLen-hLen-1)
	db[len(db)-hLen-1] = 0x01
	copy(db[len(db)-hLen:], salt)
	mask := mgf1(hash, h, len(db))
	for i := range db {
		db[i] ^= mask[i]
	}
	db[0] &= 0xFF >> (8*emLen - emBits)
	em := append(append(db, h...), 0xBC)
	sig := new(big.Int).Exp(new(big.Int).SetBytes(em), priv.D, priv.N)
	out := sig.FillBytes(make([]byte, priv.Size()))
	if err := rsa.VerifyPSS(&priv.PublicKey, hash, digest, out, &rsa.PSSOptions{SaltLength: rsa.PSSSaltLengthEqualsHash}); err != nil {
		panic(err)
	}
	return out
}

// mgf1 is MGF1 (RFC 8017, appendix B.2.1) with hash.
func mgf1(hash crypto.Hash, seed []byte, n int) []byte {
	var out []byte
	for counter := uint32(0); len(out) < n; counter++ {
		h := hash.New()
		h.Write(seed)
		var c [4]byte
		binary.BigEndian.PutUint32(c[:], counter)
		h.Write(c[:])
		out = h.Sum(out)
	}
	return out[:n]
}

func sign(k *keyPair, alg sigAlg, tbs []byte) []byte {
	var digest []byte
	switch alg.hash {
	case 0:
	case crypto.MD5:
		h := md5.Sum(tbs)
		digest = h[:]
	case crypto.SHA1:
		h := sha1.Sum(tbs)
		digest = h[:]
	case crypto.SHA256:
		h := sha256.Sum256(tbs)
		digest = h[:]
	case crypto.SHA384:
		h := sha512.Sum384(tbs)
		digest = h[:]
	case crypto.SHA512:
		h := sha512.Sum512(tbs)
		digest = h[:]
	}
	switch priv := k.priv.(type) {
	case *ecdsa.PrivateKey:
		sig, err := priv.Sign(nil, digest, alg.hash)
		if err != nil {
			panic(err)
		}
		return sig
	case *rsa.PrivateKey:
		if alg.pss {
			return signPSS(priv, alg.hash, digest)
		}
		sig, err := rsa.SignPKCS1v15(nil, priv, alg.hash, digest)
		if err != nil {
			panic(err)
		}
		return sig
	case ed25519.PrivateKey:
		return ed25519.Sign(priv, tbs)
	}
	panic("unknown key")
}
