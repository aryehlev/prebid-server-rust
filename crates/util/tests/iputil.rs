//! Go's `net` and `util/iputil` against `pbs_util::iputil` on the golden of
//! `testdata/go/golden/ipsweep.go`: `net.ParseIP`, `IP.String`, `iputil.ParseIP`'s version, the
//! default private-network validator and `privacy.scrubIP` over 50,000 seeded addresses with
//! seeded masks and `net.ParseCIDR` prefixes, and over edge lists of addresses and networks.

#[cfg(test)]
mod tests {
    use std::{error::Error, fmt::Write as _, fs, net::IpAddr, path::PathBuf};

    use ipnet::IpNet;
    use pbs_util::iputil::{
        IPV4_BIT_SIZE, IPV4_DEFAULT_MASKING_BIT_SIZE, IPV6_BIT_SIZE, IPV6_DEFAULT_MASKING_BIT_SIZE,
        IpValidator as _, IpVersion, PublicNetworkIpValidator, net, parse_ip,
    };
    use serde::Deserialize;
    use sha2::{Digest as _, Sha256};

    type TestResult = Result<(), Box<dyn Error>>;

    #[derive(Deserialize)]
    struct Golden {
        seed: u64,
        count: usize,
        inputs_sha256: String,
        outputs_sha256: String,
        edges: Vec<Edge>,
        cidr_edges: Vec<CidrEdge>,
    }

    #[derive(Deserialize)]
    struct Edge {
        input: String,
        outputs: Vec<String>,
    }

    #[derive(Deserialize)]
    struct CidrEdge {
        input: String,
        verdict: String,
    }

    fn golden() -> Result<Golden, Box<dyn Error>> {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "testdata",
            "golden",
            "iputil_sweep.json",
        ]
        .iter()
        .collect();
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn check(ok: bool, what: &str) -> TestResult {
        if ok { Ok(()) } else { Err(what.into()) }
    }

    /// The splitmix64 generator of the Go golden program (Go: `splitmix64`).
    struct SplitMix64(u64);

    impl SplitMix64 {
        const fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        }

        /// Go: `intn(n)`, `next() % n`.
        fn intn(&mut self, n: usize) -> usize {
            let n = u64::try_from(n).unwrap_or(u64::MAX).max(1);
            usize::try_from(self.next() % n).unwrap_or_default()
        }

        /// Go: `pick(xs)`.
        fn pick<T: Copy + Default>(&mut self, xs: &[T]) -> T {
            let at = self.intn(xs.len());
            xs.get(at).copied().unwrap_or_default()
        }
    }

    /// Go: `manifest`, SHA-256 over `len:input\n` and over `output\n`.
    #[derive(Default)]
    struct Manifest {
        inputs: Sha256,
        outputs: Sha256,
    }

    impl Manifest {
        fn add(&mut self, input: &str, output: &str) {
            self.inputs
                .update(format!("{}:{input}\n", input.len()).as_bytes());
            self.outputs.update(format!("{output}\n").as_bytes());
        }

        fn sums(self) -> (String, String) {
            (hex(&self.inputs.finalize()), hex(&self.outputs.finalize()))
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().fold(String::new(), |mut s, b| {
            let _ok = write!(s, "{b:02x}");
            s
        })
    }

    /// Go: `ipMutationAlphabet`.
    const MUTATIONS: &[u8; 31] = b"0123456789abcdefABCDEF.:%[]/ \tx";

    /// Go: `ipZones`.
    const ZONES: [&str; 5] = ["eth0", "1", "en0", "", "25"];

    /// Go: `ipMaskBits`.
    const MASK_BITS: [i64; 4] = [32, 128, 64, 0];

    /// Go: `sweepOctet`.
    fn octet(r: &mut SplitMix64) -> String {
        match r.intn(6) {
            0 => "0".to_owned(),
            1 => "255".to_owned(),
            _ => r.intn(256).to_string(),
        }
    }

    /// Go: `sweepGroup`.
    fn group(r: &mut SplitMix64) -> String {
        let drawn = r.intn(0x1_0000);
        let v = if r.intn(4) == 0 { 0 } else { drawn };
        let s = format!("{v:x}");
        match r.intn(4) {
            0 => s.to_uppercase(),
            1 => format!("{s:0>4}"),
            _ => s,
        }
    }

    /// Go: `sweepDotted`.
    fn dotted(r: &mut SplitMix64) -> String {
        let o: Vec<String> = (0..4).map(|_| octet(r)).collect();
        o.join(".")
    }

    /// Go: `sweepGroups`.
    fn groups(r: &mut SplitMix64, n: usize) -> String {
        let g: Vec<String> = (0..n).map(|_| group(r)).collect();
        g.join(":")
    }

    /// Go: `seededAddressBase`.
    fn address_base(r: &mut SplitMix64) -> String {
        match r.intn(11) {
            0 | 1 => dotted(r),
            2 => {
                let mut o: Vec<String> = (0..4).map(|_| octet(r)).collect();
                if let Some(field) = o.get_mut(r.intn(4)) {
                    field.insert(0, '0');
                }
                o.join(".")
            }
            3 => format!("::ffff:{}", dotted(r)),
            4 => format!("::FFFF:{}", dotted(r)),
            5 => format!("0:0:0:0:0:ffff:{}", dotted(r)),
            6 => format!("::{}", dotted(r)),
            7 => groups(r, 8),
            8 => {
                let h = r.intn(8);
                let t = r.intn(8 - h);
                let head = groups(r, h);
                format!("{head}::{}", groups(r, t))
            }
            9 => {
                let g = group(r);
                format!("fe80::{g}%{}", r.pick(&ZONES))
            }
            _ => {
                let g = groups(r, 6);
                format!("{g}:{}", dotted(r))
            }
        }
    }

    /// Go: `seededAddress`: a base form, or one with a byte inserted, deleted or replaced, or
    /// added before or after. The forms are ASCII, so a byte is a character.
    fn address(r: &mut SplitMix64) -> String {
        let s = address_base(r);
        if r.intn(4) != 0 {
            return s;
        }
        let b = char::from(r.pick(MUTATIONS));
        let i = r.intn(s.len());
        let head = s.get(..i).unwrap_or_default();
        match r.intn(5) {
            0 => format!("{head}{b}{}", s.get(i..).unwrap_or_default()),
            1 => format!("{head}{}", s.get(i + 1..).unwrap_or_default()),
            2 => format!("{head}{b}{}", s.get(i + 1..).unwrap_or_default()),
            3 => format!("{b}{s}"),
            _ => format!("{s}{b}"),
        }
    }

    /// Go: `defaultValidator`, the private networks of the default request validation.
    fn default_validator() -> Result<PublicNetworkIpValidator, Box<dyn Error>> {
        let nets = |list: &[&str]| -> Result<Vec<IpNet>, Box<dyn Error>> {
            list.iter()
                .map(|s| Ok(net::parse_cidr(s)?.1))
                .collect::<Result<Vec<_>, Box<dyn Error>>>()
        };
        Ok(PublicNetworkIpValidator {
            ipv4_private_networks: nets(&[
                "10.0.0.0/8",
                "172.16.0.0/12",
                "192.168.0.0/16",
                "169.254.0.0/16",
                "127.0.0.0/8",
            ])?,
            ipv6_private_networks: nets(&[
                "::1/128",
                "fc00::/7",
                "fe80::/10",
                "ff00::/8",
                "2001:db8::/32",
            ])?,
        })
    }

    /// Go's 16 bytes of a parsed address (Go: `ipHex`), an IPv4 address in its mapped form.
    fn go_hex(ip: Option<IpAddr>) -> String {
        ip.map_or_else(
            || "nil".to_owned(),
            |ip| match ip {
                IpAddr::V4(a) => hex(&a.to_ipv6_mapped().octets()),
                IpAddr::V6(a) => hex(&a.octets()),
            },
        )
    }

    /// Go: `scrubIP` (privacy/scrubber.go:149-156).
    fn scrub(s: &str, ones: i64, bits: i64) -> String {
        if s.is_empty() {
            return String::new();
        }
        net::ip_string(net::parse_ip(s).and_then(|ip| net::mask(ip, ones, bits)))
    }

    /// Go: `cidrVerdict`: `err`, or the network's bytes, prefix length and byte length.
    fn cidr_verdict(s: &str) -> String {
        match net::parse_cidr(s) {
            Err(_e) => "err".to_owned(),
            Ok((_ip, IpNet::V4(n))) => format!("{} {} 4", hex(&n.addr().octets()), n.prefix_len()),
            Ok((_ip, IpNet::V6(n))) => {
                format!("{} {} 16", hex(&n.addr().octets()), n.prefix_len())
            }
        }
    }

    /// Go: `ipOutputs`.
    fn ip_outputs(s: &str, validator: &PublicNetworkIpValidator) -> Vec<String> {
        let ip = net::parse_ip(s);
        let ver = parse_ip(s).map_or(IpVersion::Unknown, |(_ip, ver)| ver);
        let valid = ip.map_or_else(
            || "-".to_owned(),
            |ip| validator.is_valid(&ip, ver).to_string(),
        );
        vec![
            go_hex(ip),
            net::ip_string(ip),
            ver.as_int().to_string(),
            valid,
            scrub(s, IPV4_DEFAULT_MASKING_BIT_SIZE, IPV4_BIT_SIZE),
            scrub(s, IPV6_DEFAULT_MASKING_BIT_SIZE, IPV6_BIT_SIZE),
        ]
    }

    #[test]
    fn edge_list() -> TestResult {
        // Go: net.ParseIP, IP.String, iputil.ParseIP, the default validator and scrubIP on
        // the edge addresses: leading zeros, IPv4-mapped forms in each spelling, the
        // IPv4-compatible form, zones, white space, ports and brackets, and the limits of
        // every field. std reads every one as Go does once a mapped address is the IPv4
        // address it maps.
        let g = golden()?;
        let validator = default_validator()?;
        check(g.edges.len() == 79, "the edge addresses")?;
        for edge in &g.edges {
            let got = ip_outputs(&edge.input, &validator);
            check(
                got == edge.outputs,
                &format!("{:?}: got {got:?}, want {:?}", edge.input, edge.outputs),
            )?;
        }
        Ok(())
    }

    #[test]
    fn cidr_edges() -> TestResult {
        // Go: net.ParseCIDR as config.parseNetworks reads request_validation's private
        // networks: prefix spellings dtoi accepts and refuses, leading zeros, mapped networks of
        // both lengths, zones and the limits of the prefix.
        let g = golden()?;
        check(g.cidr_edges.len() == 42, "the edge networks")?;
        for edge in &g.cidr_edges {
            let got = cidr_verdict(&edge.input);
            check(
                got == edge.verdict,
                &format!("{:?}: got {got:?}, want {:?}", edge.input, edge.verdict),
            )?;
        }
        Ok(())
    }

    #[test]
    fn seeded_addresses() -> TestResult {
        // Go: the edge list's functions, scrubIP with a seeded mask and ParseCIDR with a
        // seeded prefix over 50,000 seeded addresses of eleven forms, a quarter of them with a
        // byte inserted, deleted or replaced.
        let g = golden()?;
        check(g.count == 50_000, "50,000 seeded addresses")?;
        let validator = default_validator()?;
        let mut r = SplitMix64(g.seed);
        let mut manifest = Manifest::default();
        for _ in 0..g.count {
            let s = address(&mut r);
            let ones = i64::try_from(r.intn(134))? - 2;
            let bits = r.pick(&MASK_BITS);
            let mut prefix = (i64::try_from(r.intn(134))? - 2).to_string();
            if r.intn(5) == 0 {
                prefix.insert(0, '0');
            }
            let mut fields = ip_outputs(&s, &validator);
            fields.push(ones.to_string());
            fields.push(bits.to_string());
            fields.push(scrub(&s, ones, bits));
            fields.push(cidr_verdict(&format!("{s}/{prefix}")));
            manifest.add(&s, &fields.join("\t"));
        }
        let (inputs, outputs) = manifest.sums();
        check(inputs == g.inputs_sha256, "the seeded addresses")?;
        check(outputs == g.outputs_sha256, "their outputs")
    }
}
