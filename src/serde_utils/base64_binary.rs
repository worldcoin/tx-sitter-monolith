use base64::engine::general_purpose;
use base64::Engine as _;
use serde::Deserialize;

pub fn serialize<S>(
    binaries: &Option<Vec<Vec<u8>>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match binaries {
        Some(binaries) => {
            let base64_vec: Vec<String> = binaries
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
            let decoded_vec: Vec<Vec<u8>> = base64_vec
                .into_iter()
                .map(|base64_str| {
                    general_purpose::STANDARD.decode(base64_str).unwrap()
                })
                .collect();

            Ok(Some(decoded_vec))
        }
        None => Ok(None),
    }
}
