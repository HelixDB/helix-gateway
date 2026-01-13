//! Serialization format abstraction.
//!
//! Provides a unified interface for serializing and deserializing request/response
//! bodies. Currently supports JSON via sonic-rs, but the abstraction allows adding
//! other formats (e.g., MessagePack, CBOR) without changing handler code.

use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufWriter};

use crate::{Error, utils::MaybeOwned};

/// Supported serialization formats.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub enum Format {
    #[default]
    Json,
}

impl std::str::FromStr for Format {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "application/json" => Ok(Format::Json),
            _ => Err(()),
        }
    }
}

#[allow(unused)]
impl Format {
    /// Returns the MIME content type for this format.
    pub(crate) fn content_type(self) -> &'static str {
        match self {
            Format::Json => "application/json",
        }
    }

    /// Serialize the value to bytes.
    /// If using a zero-copy format it will return a Cow::Borrowed, with a lifetime corresponding to the value.
    /// Otherwise, it returns a Cow::Owned.
    pub(crate) fn serialize<'a, T: Serialize>(self, val: &T) -> Result<Cow<'a, [u8]>, Error> {
        match self {
            Format::Json => Ok(Cow::from(sonic_rs::to_vec(val)?)),
        }
    }

    /// Serialize the value to the supplied async writer.
    /// This will use an underlying async implementation if possible, otherwise it will buffer it
    pub(crate) async fn serialize_to_async<T: Serialize>(
        self,
        val: &T,
        writer: &mut BufWriter<impl AsyncWrite + Unpin>,
    ) -> Result<(), Error> {
        match self {
            Format::Json => {
                let encoded = sonic_rs::to_vec(val)?;
                writer
                    .write_all(&encoded)
                    .await
                    .map_err(|e| Error::InternalError(eyre::Error::from(e)));
            }
        }
        Ok(())
    }

    /// Deserialize the provided value
    /// Returns a MaybeOwned::Borrowed if using a zero-copy format
    /// or a MaybeOwned::Owned otherwise
    pub(crate) fn deserialize<'a, T: Deserialize<'a>>(
        self,
        val: &'a [u8],
    ) -> Result<MaybeOwned<'a, T>, Error> {
        match self {
            Format::Json => Ok(MaybeOwned::Owned(sonic_rs::from_slice::<T>(val)?)),
        }
    }

    /// Deserialize the provided value
    pub(crate) fn deserialize_owned<'a, T: Deserialize<'a>>(
        self,
        val: &'a [u8],
    ) -> Result<T, Error> {
        match self {
            Format::Json => Ok(sonic_rs::from_slice::<T>(val)?),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct TestStruct {
        name: String,
        value: i32,
    }

    #[test]
    fn test_format_default() {
        let format = Format::default();
        assert_eq!(format, Format::Json);
    }

    #[test]
    fn test_format_from_str_json() {
        let format = Format::from_str("application/json").unwrap();
        assert_eq!(format, Format::Json);
    }

    #[test]
    fn test_format_from_str_invalid() {
        assert!(Format::from_str("text/plain").is_err());
        assert!(Format::from_str("application/xml").is_err());
        assert!(Format::from_str("").is_err());
        assert!(Format::from_str("json").is_err());
    }

    #[test]
    fn test_serialize_struct() {
        let format = Format::Json;
        let test_data = TestStruct {
            name: "test".to_string(),
            value: 42,
        };

        let serialized = format.serialize(&test_data).unwrap();
        let expected = br#"{"name":"test","value":42}"#;
        assert_eq!(serialized.as_ref(), expected);
    }

    #[test]
    fn test_serialize_primitive() {
        let format = Format::Json;

        let num_serialized = format.serialize(&123).unwrap();
        assert_eq!(num_serialized.as_ref(), b"123");

        let str_serialized = format.serialize(&"hello").unwrap();
        assert_eq!(str_serialized.as_ref(), b"\"hello\"");

        let bool_serialized = format.serialize(&true).unwrap();
        assert_eq!(bool_serialized.as_ref(), b"true");
    }

    #[test]
    fn test_serialize_vec() {
        let format = Format::Json;
        let data = vec![1, 2, 3];

        let serialized = format.serialize(&data).unwrap();
        assert_eq!(serialized.as_ref(), b"[1,2,3]");
    }

    #[tokio::test]
    async fn test_serialize_to_async() {
        let format = Format::Json;
        let test_data = TestStruct {
            name: "async_test".to_string(),
            value: 99,
        };

        let mut buffer = Vec::new();
        let mut writer = BufWriter::new(&mut buffer);

        format
            .serialize_to_async(&test_data, &mut writer)
            .await
            .unwrap();
        writer.flush().await.unwrap();

        let expected = br#"{"name":"async_test","value":99}"#;
        assert_eq!(buffer, expected);
    }

    #[test]
    fn test_deserialize_struct() {
        let format = Format::Json;
        let json_bytes = br#"{"name":"deserialized","value":100}"#;

        let result: MaybeOwned<TestStruct> = format.deserialize(json_bytes).unwrap();
        assert_eq!(result.name, "deserialized");
        assert_eq!(result.value, 100);
    }

    #[test]
    fn test_deserialize_primitive() {
        let format = Format::Json;

        let num: MaybeOwned<i32> = format.deserialize(b"42").unwrap();
        assert_eq!(*num, 42);

        let string: MaybeOwned<String> = format.deserialize(b"\"hello\"").unwrap();
        assert_eq!(*string, "hello");

        let boolean: MaybeOwned<bool> = format.deserialize(b"true").unwrap();
        assert_eq!(*boolean, true);
    }

    #[test]
    fn test_deserialize_vec() {
        let format = Format::Json;
        let json_bytes = b"[1,2,3,4,5]";

        let result: MaybeOwned<Vec<i32>> = format.deserialize(json_bytes).unwrap();
        assert_eq!(*result, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_deserialize_invalid_json() {
        let format = Format::Json;
        let invalid_json = b"not valid json";

        let result: Result<MaybeOwned<TestStruct>, Error> = format.deserialize(invalid_json);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_owned_struct() {
        let format = Format::Json;
        let json_bytes = br#"{"name":"owned","value":200}"#;

        let result: TestStruct = format.deserialize_owned(json_bytes).unwrap();
        assert_eq!(result.name, "owned");
        assert_eq!(result.value, 200);
    }

    #[test]
    fn test_deserialize_owned_primitive() {
        let format = Format::Json;

        let num: i32 = format.deserialize_owned(b"123").unwrap();
        assert_eq!(num, 123);

        let string: String = format.deserialize_owned(b"\"world\"").unwrap();
        assert_eq!(string, "world");
    }

    #[test]
    fn test_deserialize_owned_invalid_json() {
        let format = Format::Json;
        let invalid_json = b"{invalid}";

        let result: Result<TestStruct, Error> = format.deserialize_owned(invalid_json);
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip_struct() {
        let format = Format::Json;
        let original = TestStruct {
            name: "roundtrip".to_string(),
            value: 12345,
        };

        let serialized = format.serialize(&original).unwrap();
        let deserialized: TestStruct = format.deserialize_owned(&serialized).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_roundtrip_complex() {
        let format = Format::Json;
        let original = vec![
            TestStruct {
                name: "first".to_string(),
                value: 1,
            },
            TestStruct {
                name: "second".to_string(),
                value: 2,
            },
        ];

        let serialized = format.serialize(&original).unwrap();
        let deserialized: Vec<TestStruct> = format.deserialize_owned(&serialized).unwrap();

        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_format_clone_and_copy() {
        let format = Format::Json;
        let cloned = format.clone();
        let copied = format;

        assert_eq!(format, cloned);
        assert_eq!(format, copied);
    }
}
