//! FlatBuffers (de)serialization of the portable action-sequence table.
//!
//! `flatc` is not available in this toolchain, so this module encodes
//! directly with the `flatbuffers` crate's builder/reader primitives — the
//! same low-level APIs generated code calls — rather than relying on
//! schema codegen. The wire format is intentionally simple: a flat vector of
//! strings where every 3 consecutive entries form one
//! `(from_action, to_action, count)` record, with `count` written as a
//! decimal string.
use super::model::ActionSignature;
use flatbuffers::{ForwardsUOffset, Vector};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SpeculativeCodecError {
    #[error("flatbuffer payload is malformed: {reason}")]
    Malformed { reason: String },
}

/// Encode `(from_action, to_action, count)` records into a FlatBuffers byte
/// buffer suitable for export/import as a portable domain pack.
pub fn encode_model(records: &[(ActionSignature, ActionSignature, u32)]) -> Vec<u8> {
    let mut builder = flatbuffers::FlatBufferBuilder::new();
    let mut offsets = Vec::with_capacity(records.len() * 3);
    for (from, to, count) in records {
        offsets.push(builder.create_string(from.as_str()));
        offsets.push(builder.create_string(to.as_str()));
        offsets.push(builder.create_string(&count.to_string()));
    }
    let vector = builder.create_vector(&offsets);
    builder.finish_minimal(vector);
    builder.finished_data().to_vec()
}

/// Decode a FlatBuffers byte buffer produced by [`encode_model`] back into
/// `(from_action, to_action, count)` records.
pub fn decode_model(
    bytes: &[u8],
) -> Result<Vec<(ActionSignature, ActionSignature, u32)>, SpeculativeCodecError> {
    let vector = flatbuffers::root::<Vector<ForwardsUOffset<&str>>>(bytes).map_err(|err| {
        SpeculativeCodecError::Malformed {
            reason: err.to_string(),
        }
    })?;

    if vector.len() % 3 != 0 {
        return Err(SpeculativeCodecError::Malformed {
            reason: format!(
                "expected a multiple of 3 string entries, found {}",
                vector.len()
            ),
        });
    }

    let mut records = Vec::with_capacity(vector.len() / 3);
    let mut entries = vector.iter();
    while let (Some(from), Some(to), Some(count_str)) =
        (entries.next(), entries.next(), entries.next())
    {
        let count: u32 = count_str
            .parse()
            .map_err(|_| SpeculativeCodecError::Malformed {
                reason: format!("invalid count value: {count_str:?}"),
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
    fn rejects_garbage_bytes_as_malformed() {
        let err = decode_model(&[0xFF, 0x00, 0x12, 0x34]).unwrap_err();
        assert!(matches!(err, SpeculativeCodecError::Malformed { .. }));
    }

    #[test]
    fn rejects_truncated_record_groups_as_malformed() {
        // A valid vector of 2 strings — not a multiple of 3.
        let mut builder = flatbuffers::FlatBufferBuilder::new();
        let a = builder.create_string("only");
        let b = builder.create_string("two");
        let vector = builder.create_vector(&[a, b]);
        builder.finish_minimal(vector);
        let bytes = builder.finished_data().to_vec();

        let err = decode_model(&bytes).unwrap_err();
        assert_eq!(
            err,
            SpeculativeCodecError::Malformed {
                reason: "expected a multiple of 3 string entries, found 2".to_string()
            }
        );
    }

    #[test]
    fn rejects_non_numeric_count_as_malformed() {
        let mut builder = flatbuffers::FlatBufferBuilder::new();
        let from = builder.create_string("click:a");
        let to = builder.create_string("click:b");
        let count = builder.create_string("not-a-number");
        let vector = builder.create_vector(&[from, to, count]);
        builder.finish_minimal(vector);
        let bytes = builder.finished_data().to_vec();

        let err = decode_model(&bytes).unwrap_err();
        assert_eq!(
            err,
            SpeculativeCodecError::Malformed {
                reason: "invalid count value: \"not-a-number\"".to_string()
            }
        );
    }
}
