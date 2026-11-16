//! GPP string parsing. Go: `parse.go`.

use crate::{
    constants::{
        SECTION_GPP, SECTION_USPCA, SECTION_USPCO, SECTION_USPCT, SECTION_USPNAT, SECTION_USPUT,
        SECTION_USPVA, SectionId, section_name_by_id,
    },
    error::GppError,
    sections::{
        uspca::Uspca, uspco::Uspco, uspct::Uspct, uspnat::Uspnat, usput::Usput, uspva::Uspva,
    },
    util::BitStream,
};

/// The first character of every GPP header (6 bits encoding type 3). Go: `SectionGPPByte`.
pub const SECTION_GPP_BYTE: u8 = b'D';
/// Minimum decoded header length in bytes. Go: `MaxHeaderLength` (sic).
pub const MAX_HEADER_LENGTH: u16 = 3;

/// A parsed GPP string. Go: `gpp.GppContainer`.
///
/// On a header error [`parse`] returns whatever had been filled in so far: `version` is set once
/// the version was read (Fibonacci range and section-count errors), and `section_types` and
/// `sections` stay empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct GppContainer {
    /// GPP version from the header. Go: `Version`.
    pub version: i64,
    /// Section IDs in header order. Go: `SectionTypes`.
    pub section_types: Vec<SectionId>,
    /// One section per entry of `section_types`, in the same order. Go: `Sections`.
    pub sections: Vec<Section>,
}

/// A GPP section. Go: the `gpp.Section` interface and its implementations.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    /// US national (7).
    Uspnat(Uspnat),
    /// California (8).
    Uspca(Uspca),
    /// Virginia (9).
    Uspva(Uspva),
    /// Colorado (10).
    Uspco(Uspco),
    /// Utah (11).
    Usput(Usput),
    /// Connecticut (12).
    Uspct(Uspct),
    /// Any other section ID, kept undecoded. Go: `gpp.GenericSection`.
    Generic(GenericSection),
}

impl Section {
    /// Go: `Section.GetID`. A US section whose string failed to decode is Go's zero value, so
    /// its ID is 0.
    #[must_use]
    pub const fn id(&self) -> SectionId {
        match self {
            Self::Uspnat(s) => s.id(),
            Self::Uspca(s) => s.id(),
            Self::Uspva(s) => s.id(),
            Self::Uspco(s) => s.id(),
            Self::Usput(s) => s.id(),
            Self::Uspct(s) => s.id(),
            Self::Generic(s) => s.id(),
        }
    }

    /// The section string ("base64 encoding usually, but plaintext for ccpa").
    /// Go: `Section.GetValue`. A US section whose string failed to decode has an empty value.
    #[must_use]
    pub fn value(&self) -> &str {
        match self {
            Self::Uspnat(s) => s.value(),
            Self::Uspca(s) => s.value(),
            Self::Uspva(s) => s.value(),
            Self::Uspco(s) => s.value(),
            Self::Usput(s) => s.value(),
            Self::Uspct(s) => s.value(),
            Self::Generic(s) => s.value(),
        }
    }

    /// Re-encodes the section. Go: `Section.Encode`.
    #[must_use]
    pub fn encode(&self, gpc_included: bool) -> Vec<u8> {
        match self {
            Self::Uspnat(s) => s.encode(gpc_included),
            Self::Uspca(s) => s.encode(gpc_included),
            Self::Uspva(s) => s.encode(gpc_included),
            Self::Uspco(s) => s.encode(gpc_included),
            Self::Usput(s) => s.encode(gpc_included),
            Self::Uspct(s) => s.encode(gpc_included),
            Self::Generic(s) => s.encode(gpc_included),
        }
    }
}

/// A section go-gpp does not decode. Go: `gpp.GenericSection`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct GenericSection {
    section_id: SectionId,
    value: String,
}

impl GenericSection {
    /// Creates a section with the given ID and raw string. Go builds these only inside `Parse`
    /// (its fields are unexported); this constructor also stands in for the mock sections that
    /// Prebid Server's Go tests implement.
    #[must_use]
    pub const fn new(section_id: SectionId, value: String) -> Self {
        Self { section_id, value }
    }

    /// Go: `GenericSection.GetID`.
    #[must_use]
    pub const fn id(&self) -> SectionId {
        self.section_id
    }

    /// Go: `GenericSection.GetValue`.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the raw value; the argument is ignored. Go: `GenericSection.Encode`.
    #[must_use]
    pub fn encode(&self, _gpc_included: bool) -> Vec<u8> {
        self.value.as_bytes().to_vec()
    }
}

/// Parses a GPP string. Go: `gpp.Parse`.
///
/// Like Go this returns the container together with every error found. Header errors stop
/// parsing (one error, partially filled container). Section errors do not: each failing US
/// section is recorded as `error parsing <name> consent string: <err>` and left as the zero
/// value (ID 0, empty value) in `sections`, and parsing continues.
///
/// Go quirk reproduced: the default (undecoded section) branch checks a stale `err` variable, so
/// every generic section that follows a failed US section (with no successful US section in
/// between) adds `error parsing unsupported (section N) consent string: <that same err>`.
///
/// Divergence: where Go never returns (a Fibonacci range ending at section ID 65535 makes its
/// `uint16` loop wrap forever while appending IDs, and an unterminated Fibonacci integer in a
/// header of at least 8192 bytes spins forever), this returns a Rust-only header error.
#[must_use]
pub fn parse(v: &str) -> (GppContainer, Vec<GppError>) {
    let mut gpp = GppContainer::default();

    let section_strings: Vec<&str> = v.split('~').collect();
    match parse_header(&section_strings, &mut gpp) {
        Ok(sec_ids) => gpp.section_types = sec_ids,
        Err(err) => return (gpp, vec![err]),
    }

    let (sections, errs) = parse_sections(&gpp.section_types, &section_strings);
    gpp.sections = sections;

    (gpp, errs)
}

/// Decodes the header (the first `~`-separated string) and returns the section IDs. Sets
/// `gpp.version` as soon as it is known, which is what Go leaves in the returned container on a
/// later header error.
fn parse_header(
    section_strings: &[&str],
    gpp: &mut GppContainer,
) -> Result<Vec<SectionId>, GppError> {
    let header = section_strings.first().copied().unwrap_or_default();

    let mut bs = BitStream::from_base64(header).map_err(|err| {
        GppError::new(format!("error parsing GPP header, base64 decoding: {err}"))
    })?;
    if bs.len() < MAX_HEADER_LENGTH {
        return Err(GppError::new(format!(
            "error parsing GPP header, should be at least {MAX_HEADER_LENGTH} bytes long"
        )));
    }

    // The first 6 bits of the header must always evaluate to the integer '3' as the GPP header
    // type; Go short-cuts this with a byte comparison on the encoded string.
    if header.as_bytes().first() != Some(&SECTION_GPP_BYTE) {
        return Err(GppError::new(format!(
            "error parsing GPP header, header must have type={SECTION_GPP}"
        )));
    }
    bs.set_position(6);

    let ver = bs.read_byte6().map_err(|err| {
        GppError::new(format!(
            "error parsing GPP header, unable to parse GPP version: {err}"
        ))
    })?;
    gpp.version = i64::from(ver);

    let int_range = bs.read_fibonacci_range().map_err(|err| {
        GppError::new(format!(
            "error parsing GPP header, section identifiers: {err}"
        ))
    })?;

    // We do not count the GPP header as a section.
    let sec_count = section_strings.len() - 1;

    // Go expands every range with `for i := sec.StartID; i <= sec.EndID; i++` on uint16. Count
    // first so that huge ranges are never materialised unless they match the section count.
    let mut id_count = 0u64;
    for r in &int_range.range {
        if r.end_id == u16::MAX {
            // Rust-only: Go's loop never terminates here (it appends until out of memory).
            return Err(GppError::new(format!(
                "error parsing GPP header, section identifiers: range {}-{} never ends",
                r.start_id, r.end_id
            )));
        }
        if r.start_id <= r.end_id {
            id_count += u64::from(r.end_id - r.start_id) + 1;
        }
    }
    if u64::try_from(sec_count).ok() != Some(id_count) {
        return Err(GppError::new(format!(
            "error parsing GPP header, section IDs do not match the number of sections: found \
             {id_count} IDs, have {sec_count} sections"
        )));
    }

    let mut sec_ids = Vec::with_capacity(sec_count);
    for r in &int_range.range {
        sec_ids.extend((r.start_id..=r.end_id).map(|i| SectionId(i64::from(i))));
    }
    Ok(sec_ids)
}

/// Decodes each section string (`section_strings[i + 1]` for `sec_ids[i]`).
fn parse_sections(
    sec_ids: &[SectionId],
    section_strings: &[&str],
) -> (Vec<Section>, Vec<GppError>) {
    let mut sections = Vec::with_capacity(sec_ids.len());
    let mut errs = Vec::new();
    // Go reuses one `err` variable across iterations; the default branch reads it.
    let mut err: Option<GppError> = None;
    for (i, &id) in sec_ids.iter().enumerate() {
        let section_string = section_strings.get(i + 1).copied().unwrap_or_default();
        if let Some((section, section_err)) = parse_us_section(id, section_string) {
            if let Some(section_err) = &section_err {
                errs.push(GppError::new(format!(
                    "error parsing {} consent string: {section_err}",
                    section_name_by_id(id.0)
                )));
            }
            sections.push(section);
            err = section_err;
        } else {
            sections.push(Section::Generic(GenericSection::new(
                id,
                section_string.to_owned(),
            )));
            if let Some(stale) = &err {
                errs.push(GppError::new(format!(
                    "error parsing unsupported (section {id}) consent string: {stale}"
                )));
            }
        }
    }
    (sections, errs)
}

/// Pairs a decoded section with no error, or Go's zero value of that section type with the error
/// (`NewUSPNAT` and friends return `USPNAT{}` alongside the error).
fn decoded_or_zero<T: Default>(
    result: Result<T, GppError>,
    wrap: fn(T) -> Section,
) -> (Section, Option<GppError>) {
    match result {
        Ok(section) => (wrap(section), None),
        Err(err) => (wrap(T::default()), Some(err)),
    }
}

/// Decodes a US state section, returning the section and its error. Returns `None` for IDs
/// go-gpp does not decode.
fn parse_us_section(id: SectionId, encoded: &str) -> Option<(Section, Option<GppError>)> {
    Some(match id {
        SECTION_USPNAT => decoded_or_zero(Uspnat::new(encoded), Section::Uspnat),
        SECTION_USPCA => decoded_or_zero(Uspca::new(encoded), Section::Uspca),
        SECTION_USPVA => decoded_or_zero(Uspva::new(encoded), Section::Uspva),
        SECTION_USPCO => decoded_or_zero(Uspco::new(encoded), Section::Uspco),
        SECTION_USPUT => decoded_or_zero(Usput::new(encoded), Section::Usput),
        SECTION_USPCT => decoded_or_zero(Uspct::new(encoded), Section::Uspct),
        _ => return None,
    })
}
