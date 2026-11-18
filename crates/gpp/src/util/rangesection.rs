//! Integer ranges. Go: `util/rangesection.go`.

use super::bitstream::BitStream;
use crate::error::GppError;

/// A list of ID ranges. Go: `util.IntRange`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IntRange {
    /// Number of ranges as encoded. Go: `Size`.
    pub size: u16,
    /// The ranges. Go: `Range`.
    pub range: Vec<IRange>,
    /// The largest ID seen while decoding. Go: `Max`.
    pub max: u16,
}

/// One inclusive ID range. Go: `util.IRange`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct IRange {
    /// First ID. Go: `StartID`.
    pub start_id: u16,
    /// Last ID. Go: `EndID`.
    pub end_id: u16,
}

impl IntRange {
    /// Checks whether `id` is contained in any range. Go: `IntRange.IsSet`.
    #[must_use]
    pub fn is_set(&self, id: u16) -> bool {
        if id > self.max {
            return false;
        }
        self.range.iter().any(|r| r.contains(id))
    }
}

impl IRange {
    /// Checks whether `id` is within this range. Go: `IRange.Contains`.
    #[must_use]
    pub const fn contains(self, id: u16) -> bool {
        self.start_id <= id && self.end_id >= id
    }
}

/// Go: `BitStream.ReadIntRange`.
pub(super) fn read_int_range(bs: &mut BitStream) -> Result<IntRange, GppError> {
    let num_entries = bs
        .read_uint12()
        .map_err(|err| GppError::new(format!("error reading size of Range(Int): {err}")))?;
    let mut max_value = 0u16;

    let mut ranges = Vec::with_capacity(usize::from(num_entries));
    for _ in 0..num_entries {
        let bit = bs.read_byte1().map_err(|err| {
            GppError::new(format!(
                "error reading the boolean bit of a Range(Int) entry: {err}"
            ))
        })?;
        if bit == 0 {
            let entry = bs.read_uint16().map_err(|err| {
                GppError::new(format!(
                    "error reading an int value in a Range(Int) entry: {err}"
                ))
            })?;
            ranges.push(IRange {
                start_id: entry,
                end_id: entry,
            });
            if entry > max_value {
                max_value = entry;
            }
        } else {
            let start_id = bs.read_uint16().map_err(|err| {
                GppError::new(format!(
                    "error reading first int value in a Range(Int) entry: {err}"
                ))
            })?;
            let end_id = bs.read_uint16().map_err(|err| {
                GppError::new(format!(
                    "error reading second int value in a Range(Int) entry: {err}"
                ))
            })?;
            ranges.push(IRange { start_id, end_id });
            if end_id > max_value {
                max_value = end_id;
            }
        }
    }

    Ok(IntRange {
        size: num_entries,
        range: ranges,
        max: max_value,
    })
}

#[cfg(test)]
mod tests {
    //! Go: `util/rangesection_test.go`.

    use super::{IRange, IntRange};
    use crate::util::BitStream;

    #[test]
    fn read_int_range() {
        let expected = IntRange {
            size: 5,
            max: 57257,
            range: vec![
                IRange {
                    start_id: 7,
                    end_id: 7,
                },
                IRange {
                    start_id: 16,
                    end_id: 22,
                },
                IRange {
                    start_id: 24,
                    end_id: 25,
                },
                IRange {
                    start_id: 82,
                    end_id: 82,
                },
                IRange {
                    start_id: 57234,
                    end_id: 57257,
                },
            ],
        };
        let got = BitStream::from_base64("AFAAPABAAFoAMAAyAFLvyW_UgA")
            .and_then(|mut bs| bs.read_int_range());
        let is_set = |id| got.as_ref().is_ok_and(|r| r.is_set(id));
        assert!(is_set(22), "22 is set");
        assert!(is_set(57240), "57240 is set");
        assert!(!is_set(23), "23 is not set");
        assert!(!is_set(60000), "above max");
        assert_eq!(got, Ok(expected), "range");
    }
}
