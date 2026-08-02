//! FlatBuffers (de)serialization of the portable action-sequence table.
//!
//! `flatc` is not available in this toolchain, so this module encodes
//! directly with the `flatbuffers` crate's builder/reader primitives — the
//! same low-level APIs generated code calls — rather than relying on
//! schema codegen. The wire format is a small two-field table:
//!
//! - field 0 (`names`): a `Vector<ForwardsUOffset<&str>>` where every 2
//!   consecutive entries form one `(from_action, to_action)` pair.
//! - field 1 (`counts`): a `Vector<u32>`, one entry per pair, holding the
//!   transition count as a proper integer instead of a decimal string.
//!
//! `decode_model` crosses a trust boundary: domain packs and other
//! speculative-state payloads can originate outside this process (plugin
//! hosts, imported packs, etc.), so every length and offset read from
//! `bytes` is treated as adversarial. FlatBuffers' own verifier
//! bounds-checks the vectors before this module touches them, but this
//! module additionally caps its own preallocation using the physical
//! buffer size rather than trusting a claimed vector length outright — the
//! verifier is documented as "experimental" upstream and should not be the
//! only line of defense against an oversized allocation.
use super::model::ActionSignature;
use flatbuffers::{Follow, ForwardsUOffset, Table, VOffsetT, Vector, Verifiable, Verifier};

const FIELD_NAMES: VOffsetT = 4;
const FIELD_COUNTS: VOffsetT = 6;

/// Conservative lower bound on the wire bytes required per encoded record:
/// two `ForwardsUOffset` entries in the `names` vector (4 bytes each) plus
/// one `u32` entry in the `counts` vector (4 bytes) — 12 bytes, ignoring the
/// string payloads those offsets point at. Used only to derive an upper
/// bound on preallocation, so a claimed vector length can never force an
/// allocation larger than the input could plausibly justify.
const MIN_BYTES_PER_RECORD: usize = 12;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SpeculativeCodecError {
    #[error("flatbuffer payload is malformed: {reason}")]
    Malformed { reason: String },
    #[error("flatbuffer payload failed verification: {0}")]
    InvalidFlatbuffer(#[from] flatbuffers::InvalidFlatbuffer),
}

/// Bound a preallocation size using the claimed element count together with
/// the physical size of the source buffer, so a crafted claimed length
/// cannot force an oversized allocation before the input has actually been
/// walked. Kept as a standalone function so the bound itself can be
/// exercised directly in tests without needing to hand-craft FlatBuffers
/// bytes for every case.
fn bounded_capacity(claimed_records: usize, buffer_len: usize) -> usize {
    let max_plausible_records = buffer_len / MIN_BYTES_PER_RECORD;
    claimed_records.min(max_plausible_records)
}

/// Raw (non-codegen'd) view over the two-field model table described above.
struct ModelTable<'a> {
    table: Table<'a>,
}

impl<'a> ModelTable<'a> {
    fn names(&self) -> Option<Vector<'a, ForwardsUOffset<&'a str>>> {
        // Safety: the field's type matches exactly what `run_verifier` below
        // checked before `flatbuffers::root` ever hands back a `ModelTable`.
        unsafe {
            self.table
                .get::<ForwardsUOffset<Vector<'a, ForwardsUOffset<&'a str>>>>(FIELD_NAMES, None)
        }
    }

    fn counts(&self) -> Option<Vector<'a, u32>> {
        // Safety: see `names` above.
        unsafe {
            self.table
                .get::<ForwardsUOffset<Vector<'a, u32>>>(FIELD_COUNTS, None)
        }
    }
}

impl<'a> Follow<'a> for ModelTable<'a> {
    type Inner = ModelTable<'a>;
    unsafe fn follow(buf: &'a [u8], loc: usize) -> Self::Inner {
        ModelTable {
            table: Table::new(buf, loc),
        }
    }
}

impl Verifiable for ModelTable<'_> {
    fn run_verifier(v: &mut Verifier, pos: usize) -> Result<(), flatbuffers::InvalidFlatbuffer> {
        v.visit_table(pos)?
            .visit_field::<ForwardsUOffset<Vector<ForwardsUOffset<&str>>>>(
                "names",
                FIELD_NAMES,
                true,
            )?
            .visit_field::<ForwardsUOffset<Vector<u32>>>("counts", FIELD_COUNTS, true)?
            .finish();
        Ok(())
    }
}

/// Encode `(from_action, to_action, count)` records into a FlatBuffers byte
/// buffer suitable for export/import as a portable domain pack.
pub fn encode_model(records: &[(ActionSignature, ActionSignature, u32)]) -> Vec<u8> {
    let mut builder = flatbuffers::FlatBufferBuilder::new();
    let mut name_offsets = Vec::with_capacity(records.len() * 2);
    let mut counts = Vec::with_capacity(records.len());
    for (from, to, count) in records {
        name_offsets.push(builder.create_string(from.as_str()));
        name_offsets.push(builder.create_string(to.as_str()));
        counts.push(*count);
    }
    let names_vector = builder.create_vector(&name_offsets);
    let counts_vector = builder.create_vector(&counts);

    let table_start = builder.start_table();
    builder.push_slot_always(FIELD_NAMES, names_vector);
    builder.push_slot_always(FIELD_COUNTS, counts_vector);
    let table_end = builder.end_table(table_start);
    builder.finish_minimal(table_end);
    builder.finished_data().to_vec()
}

/// Decode a FlatBuffers byte buffer produced by [`encode_model`] back into
/// `(from_action, to_action, count)` records.
///
/// `bytes` crosses a trust boundary and is treated as adversarial input:
/// every length is verified against the actual buffer before use, and
/// preallocation is capped by the physical buffer size rather than by the
/// untrusted claimed vector lengths alone.
pub fn decode_model(
    bytes: &[u8],
) -> Result<Vec<(ActionSignature, ActionSignature, u32)>, SpeculativeCodecError> {
    let model = flatbuffers::root::<ModelTable>(bytes)?;

    // `run_verifier` marks both fields required, so a successfully verified
    // root guarantees both are present; the `ok_or_else` below is a
    // defensive fallback, not an expected path.
    let names = model
        .names()
        .ok_or_else(|| SpeculativeCodecError::Malformed {
            reason: "missing names vector".to_string(),
        })?;
    let counts = model
        .counts()
        .ok_or_else(|| SpeculativeCodecError::Malformed {
            reason: "missing counts vector".to_string(),
        })?;

    if names.len() % 2 != 0 {
        return Err(SpeculativeCodecError::Malformed {
            reason: format!(
                "expected an even number of name entries, found {}",
                names.len()
            ),
        });
    }
    if names.len() / 2 != counts.len() {
        return Err(SpeculativeCodecError::Malformed {
            reason: format!(
                "names vector implies {} records but counts vector has {}",
                names.len() / 2,
                counts.len()
            ),
        });
    }

    let claimed_records = counts.len();
    let mut records = Vec::with_capacity(bounded_capacity(claimed_records, bytes.len()));

    let mut names_iter = names.iter();
    let mut counts_iter = counts.iter();
    for _ in 0..claimed_records {
        let from = names_iter
            .next()
            .ok_or_else(|| SpeculativeCodecError::Malformed {
                reason: "names vector exhausted before expected record count".to_string(),
            })?;
        let to = names_iter
            .next()
            .ok_or_else(|| SpeculativeCodecError::Malformed {
                reason: "names vector exhausted before expected record count".to_string(),
            })?;
        let count = counts_iter
            .next()
            .ok_or_else(|| SpeculativeCodecError::Malformed {
                reason: "counts vector exhausted before expected record count".to_string(),
            })?;
        records.push((
            ActionSignature::from(from),
            ActionSignature::from(to),
            count,
        ));
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(name: &str) -> ActionSignature {
        ActionSignature::from(name)
    }

    #[test]
    fn round_trips_empty_records() {
        let bytes = encode_model(&[]);
        assert_eq!(decode_model(&bytes).unwrap(), vec![]);
    }

    #[test]
    fn round_trips_a_single_record() {
        let records = vec![(
            action("click:search_button"),
            action("type:search_input"),
            7,
        )];
        let bytes = encode_model(&records);
        assert_eq!(decode_model(&bytes).unwrap(), records);
    }

    #[test]
    fn round_trips_multiple_records() {
        let records = vec![
            (
                action("click:search_button"),
                action("type:search_input"),
                7,
            ),
            (action("type:search_input"), action("click:submit"), 5),
            (action("click:submit"), action("click:result_link"), 1),
        ];
        let bytes = encode_model(&records);
        assert_eq!(decode_model(&bytes).unwrap(), records);
    }

    #[test]
    fn round_trips_unicode_action_names() {
        let records = vec![(action("クリック:検索ボタン"), action("入力:検索欄"), 3)];
        let bytes = encode_model(&records);
        assert_eq!(decode_model(&bytes).unwrap(), records);
    }

    #[test]
    fn round_trips_a_large_count_value() {
        // Regression check for the old decimal-string encoding: values near
        // u32::MAX must round-trip exactly now that `count` is encoded as a
        // real integer.
        let records = vec![(action("click:a"), action("click:b"), u32::MAX)];
        let bytes = encode_model(&records);
        assert_eq!(decode_model(&bytes).unwrap(), records);
    }

    #[test]
    fn rejects_garbage_bytes_as_invalid_flatbuffer() {
        let err = decode_model(&[0xFF, 0x00, 0x12, 0x34]).unwrap_err();
        assert!(matches!(err, SpeculativeCodecError::InvalidFlatbuffer(_)));
    }

    #[test]
    fn rejects_odd_length_names_vector_as_malformed() {
        let mut builder = flatbuffers::FlatBufferBuilder::new();
        let a = builder.create_string("only");
        let b = builder.create_string("two");
        let c = builder.create_string("three");
        let names = builder.create_vector(&[a, b, c]);
        let counts = builder.create_vector(&[1u32]);

        let table_start = builder.start_table();
        builder.push_slot_always(FIELD_NAMES, names);
        builder.push_slot_always(FIELD_COUNTS, counts);
        let table_end = builder.end_table(table_start);
        builder.finish_minimal(table_end);
        let bytes = builder.finished_data().to_vec();

        let err = decode_model(&bytes).unwrap_err();
        assert_eq!(
            err,
            SpeculativeCodecError::Malformed {
                reason: "expected an even number of name entries, found 3".to_string()
            }
        );
    }

    #[test]
    fn rejects_mismatched_names_and_counts_lengths_as_malformed() {
        let mut builder = flatbuffers::FlatBufferBuilder::new();
        let a = builder.create_string("from");
        let b = builder.create_string("to");
        let names = builder.create_vector(&[a, b]);
        // Names imply 1 record, but counts claims 2.
        let counts = builder.create_vector(&[1u32, 2u32]);

        let table_start = builder.start_table();
        builder.push_slot_always(FIELD_NAMES, names);
        builder.push_slot_always(FIELD_COUNTS, counts);
        let table_end = builder.end_table(table_start);
        builder.finish_minimal(table_end);
        let bytes = builder.finished_data().to_vec();

        let err = decode_model(&bytes).unwrap_err();
        assert_eq!(
            err,
            SpeculativeCodecError::Malformed {
                reason: "names vector implies 1 records but counts vector has 2".to_string()
            }
        );
    }

    /// A crafted payload whose vector header claims a huge element count
    /// that the actual buffer bytes cannot possibly back. `decode_model`
    /// must reject the payload cleanly instead of attempting an oversized
    /// allocation driven directly by the untrusted claimed length.
    #[test]
    fn rejects_crafted_payload_with_oversized_claimed_length_without_huge_alloc() {
        let mut bytes = vec![0u8; 8];
        // Root uoffset: points to offset 4 (relative to position 0).
        bytes[0..4].copy_from_slice(&4u32.to_le_bytes());
        // Claimed vector length: absurdly large, far beyond what an 8-byte
        // buffer could ever physically contain.
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());

        let result = decode_model(&bytes);
        assert!(
            result.is_err(),
            "decoding a payload whose claimed length outstrips the buffer must fail \
             cleanly instead of attempting an oversized allocation"
        );
    }

    #[test]
    fn bounds_preallocation_by_buffer_size_not_claimed_length() {
        // A claimed length of 10 million records backed by only 24 bytes of
        // buffer must not translate into a 10-million-element
        // `Vec::with_capacity` — capacity should be capped by what the
        // buffer could plausibly contain.
        let claimed = 10_000_000;
        let buffer_len = 24;
        let capped = bounded_capacity(claimed, buffer_len);
        assert!(capped <= buffer_len / MIN_BYTES_PER_RECORD);
        assert!(capped < claimed);
    }

    #[test]
    fn does_not_shrink_capacity_when_claimed_length_is_plausible() {
        let claimed = 3;
        let buffer_len = 1024;
        assert_eq!(bounded_capacity(claimed, buffer_len), claimed);
    }
}
