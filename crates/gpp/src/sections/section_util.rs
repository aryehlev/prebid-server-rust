//! Segments shared by the US state sections. Go: `sections/section_util.go`.

use crate::{error::GppError, util::BitStream};

/// The core segment layout shared by several US state strings. A one-element vector is valid
/// when a state has a single subfield. Go: `sections.CommonUSCoreSegment`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct CommonUsCoreSegment {
    /// Go: `Version`.
    pub version: u8,
    /// Go: `SharingNotice`.
    pub sharing_notice: u8,
    /// Go: `SaleOptOutNotice`.
    pub sale_opt_out_notice: u8,
    /// Go: `TargetedAdvertisingOptOutNotice`.
    pub targeted_advertising_opt_out_notice: u8,
    /// Go: `SaleOptOut`.
    pub sale_opt_out: u8,
    /// Go: `TargetedAdvertisingOptOut`.
    pub targeted_advertising_opt_out: u8,
    /// Go: `SensitiveDataProcessing`.
    pub sensitive_data_processing: Vec<u8>,
    /// Go: `KnownChildSensitiveDataConsents`.
    pub known_child_sensitive_data_consents: Vec<u8>,
    /// Go: `MspaCoveredTransaction`.
    pub mspa_covered_transaction: u8,
    /// Go: `MspaOptOutOptionMode`.
    pub mspa_opt_out_option_mode: u8,
    /// Go: `MspaServiceProviderMode`.
    pub mspa_service_provider_mode: u8,
}

/// The Global Privacy Control segment. Go: `sections.CommonUSGPCSegment`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CommonUsGpcSegment {
    /// Go: `SubsectionType`.
    pub subsection_type: u8,
    /// Go: `Gpc`.
    pub gpc: bool,
}

/// Go: `sections.ErrorHelper`.
pub(super) fn error_helper(name: &str, err: &GppError) -> GppError {
    GppError::new(format!(
        "unable to set field {name} due to parse error: {err}"
    ))
}

/// Go: `sections.NewCommonUSCoreSegment`.
pub(super) fn new_common_us_core_segment(
    sensitive_data_fields: i64,
    known_child_data_fields: i64,
    bs: &mut BitStream,
) -> Result<CommonUsCoreSegment, GppError> {
    let field = |name: &'static str| move |err: GppError| error_helper(name, &err);
    Ok(CommonUsCoreSegment {
        version: bs.read_byte6().map_err(field("CoreSegment.Version"))?,
        sharing_notice: bs
            .read_byte2()
            .map_err(field("CoreSegment.SharingNotice"))?,
        sale_opt_out_notice: bs
            .read_byte2()
            .map_err(field("CoreSegment.SaleOptOutNotice"))?,
        targeted_advertising_opt_out_notice: bs
            .read_byte2()
            .map_err(field("CoreSegment.TargetedAdvertisingOptOutNotice"))?,
        sale_opt_out: bs.read_byte2().map_err(field("CoreSegment.SaleOptOut"))?,
        targeted_advertising_opt_out: bs
            .read_byte2()
            .map_err(field("CoreSegment.TargetedAdvertisingOptOut"))?,
        sensitive_data_processing: bs
            .read_two_bit_field(sensitive_data_fields)
            .map_err(field("CoreSegment.SensitiveDataProcessing"))?,
        // Go labels this field "...ConsentsArr" here, unlike the state-specific cores.
        known_child_sensitive_data_consents: bs
            .read_two_bit_field(known_child_data_fields)
            .map_err(field("CoreSegment.KnownChildSensitiveDataConsentsArr"))?,
        mspa_covered_transaction: bs
            .read_byte2()
            .map_err(field("CoreSegment.MspaCoveredTransaction"))?,
        mspa_opt_out_option_mode: bs
            .read_byte2()
            .map_err(field("CoreSegment.MspaOptOutOptionMode"))?,
        mspa_service_provider_mode: bs
            .read_byte2()
            .map_err(field("CoreSegment.MspaServiceProviderMode"))?,
    })
}

impl CommonUsCoreSegment {
    /// Writes the segment. Go: `CommonUSCoreSegment.Encode`.
    pub(crate) fn encode(&self, bs: &mut BitStream) {
        bs.write_byte6(self.version);
        bs.write_byte2(self.sharing_notice);
        bs.write_byte2(self.sale_opt_out_notice);
        bs.write_byte2(self.targeted_advertising_opt_out_notice);
        bs.write_byte2(self.sale_opt_out);
        bs.write_byte2(self.targeted_advertising_opt_out);
        bs.write_two_bit_field(&self.sensitive_data_processing);
        bs.write_two_bit_field(&self.known_child_sensitive_data_consents);
        bs.write_byte2(self.mspa_covered_transaction);
        bs.write_byte2(self.mspa_opt_out_option_mode);
        bs.write_byte2(self.mspa_service_provider_mode);
    }
}

/// Go: `sections.NewCommonUSGPCSegment`.
pub(super) fn new_common_us_gpc_segment(
    bs: &mut BitStream,
) -> Result<CommonUsGpcSegment, GppError> {
    let subsection_type = bs
        .read_byte2()
        .map_err(|err| error_helper("GPCSegment.SubsectionType", &err))?;

    if subsection_type != 1 {
        return Err(GppError::new(format!(
            "invalid subsection type {subsection_type} for GPC segment"
        )));
    }

    let gpc = bs
        .read_byte1()
        .map_err(|err| error_helper("GPCSegment.Gpc", &err))?;
    Ok(CommonUsGpcSegment {
        subsection_type,
        gpc: gpc == 1,
    })
}

impl CommonUsGpcSegment {
    /// The segment a section gets when its string has no GPC part: `{SubsectionType: 1, Gpc:
    /// false}`.
    pub(crate) const DEFAULT_WHEN_ABSENT: Self = Self {
        subsection_type: 1,
        gpc: false,
    };

    /// Writes the segment. Go: `CommonUSGPCSegment.Encode`.
    pub(crate) fn encode(self, bs: &mut BitStream) {
        bs.write_byte2(self.subsection_type);
        bs.write_byte1(u8::from(self.gpc));
    }
}

/// Splits a section string on `.` and decodes the core segment and, when `gpc_check` is set and a
/// second segment exists, the GPC segment. Go: `sections.CreateBitStreams`.
///
/// Go's "no segments found in encoded string" branch is unreachable (`strings.Split` never
/// returns an empty slice) and is not ported.
pub(super) fn create_bit_streams(
    encoded: &str,
    gpc_check: bool,
) -> Result<(BitStream, Option<BitStream>), GppError> {
    let mut segments = encoded.split('.');
    let core_bit_stream = BitStream::from_base64(segments.next().unwrap_or_default())?;

    if gpc_check && let Some(gpc) = segments.next() {
        let gpc_bit_stream = BitStream::from_base64(gpc)?;
        return Ok((core_bit_stream, Some(gpc_bit_stream)));
    }

    Ok((core_bit_stream, None))
}

/// Decodes the optional GPC segment the way every GPC-bearing section does: absent means
/// [`CommonUsGpcSegment::DEFAULT_WHEN_ABSENT`].
pub(super) fn gpc_segment_or_default(
    gpc_bit_stream: Option<BitStream>,
) -> Result<CommonUsGpcSegment, GppError> {
    gpc_bit_stream.map_or(Ok(CommonUsGpcSegment::DEFAULT_WHEN_ABSENT), |mut bs| {
        new_common_us_gpc_segment(&mut bs)
    })
}

/// Encodes a core segment and, if `gpc_included`, a `.`-separated GPC segment. This is the body
/// shared by the `Encode` methods of the GPC-bearing sections.
pub(super) fn encode_with_gpc<F: FnOnce(&mut BitStream)>(
    core: F,
    gpc: CommonUsGpcSegment,
    gpc_included: bool,
) -> Vec<u8> {
    let mut bs = BitStream::new_for_write();
    core(&mut bs);
    let mut res = bs.base64_encode();
    if !gpc_included {
        return res;
    }
    bs.reset();
    res.push(b'.');
    gpc.encode(&mut bs);
    res.extend_from_slice(&bs.base64_encode());
    res
}

/// Encodes a core segment alone (sections without GPC).
pub(super) fn encode_core<F: FnOnce(&mut BitStream)>(core: F) -> Vec<u8> {
    let mut bs = BitStream::new_for_write();
    core(&mut bs);
    bs.base64_encode()
}
