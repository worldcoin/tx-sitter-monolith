use base64::engine::general_purpose;
use base64::Engine as _;
use serde::Deserialize;

pub fn serialize<S>(
    blobs: &Option<Vec<Vec<u8>>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match blobs {
        Some(blobs) => {
            let base64_vec: Vec<String> = blobs
                .iter()
                .map(|binary| general_purpose::STANDARD.encode(binary))
                .collect();

            serializer.serialize_some(&base64_vec)
        }
        None => serializer.serialize_none(),
    }
}

pub fn deserialize<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<Vec<u8>>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let base64_strings: Option<Vec<String>> =
        Option::deserialize(deserializer)?;

    match base64_strings {
        Some(base64_vec) => {
            let decoded_vec: Result<Vec<Vec<u8>>, _> = base64_vec
                .into_iter()
                .map(|base64_str| {
                    general_purpose::STANDARD
                        .decode(base64_str)
                        .map_err(serde::de::Error::custom)
                })
                .collect();

            Ok(Some(decoded_vec?))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use serde_json;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct Test {
        #[serde(with = "super")]
        blobs: Option<Vec<Vec<u8>>>,
    }

    #[test]
    fn test_deserialize_with_valid_input() {
        let blobs =
            Some(["Hello", "world!"].map(|b| b.as_bytes().to_vec()).to_vec());
        let test = Test {
            blobs: blobs.clone(),
        };

        let s = serde_json::to_string(&test).unwrap();

        let test: Test = serde_json::from_str(&s).unwrap();
        assert_eq!(test.blobs, blobs);
    }

    #[test]
    fn test_deserialize_with_null_input() {
        let test = Test { blobs: None };

        let s = serde_json::to_string(&test).unwrap();

        let test: Test = serde_json::from_str(&s).unwrap();
        assert_eq!(test.blobs, None);
    }
}
