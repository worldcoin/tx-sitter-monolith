use std::env::VarError;

/// Replaces every `${NAME}` in the string values of `value`, including those
/// nested in tables and arrays, with the value `env` returns for `NAME`.
///
/// Substituted values are not scanned again, and table keys are left as they
/// are. `$` without a following `{` stays a literal `$`.
///
/// Fails on the first reference to a variable that `env` doesn't know, naming
/// the variable and the key that references it.
pub fn substitute_env_vars(
    value: &mut toml::Value,
    env: &impl Fn(&str) -> Result<String, VarError>,
) -> eyre::Result<()> {
    substitute_at(value, "", env)
}

fn substitute_at(
    value: &mut toml::Value,
    key: &str,
    env: &impl Fn(&str) -> Result<String, VarError>,
) -> eyre::Result<()> {
    match value {
        toml::Value::String(s) => {
            *s = expand(s, env).map_err(|err| eyre::eyre!("{key}: {err}"))?;
        }
        toml::Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                substitute_at(item, &format!("{key}[{index}]"), env)?;
            }
        }
        toml::Value::Table(table) => {
            for (name, item) in table.iter_mut() {
                let key = if key.is_empty() {
                    name.clone()
                } else {
                    format!("{key}.{name}")
                };
                substitute_at(item, &key, env)?;
            }
        }
        toml::Value::Integer(_)
        | toml::Value::Float(_)
        | toml::Value::Boolean(_)
        | toml::Value::Datetime(_) => {}
    }

    Ok(())
}

fn expand(
    s: &str,
    env: &impl Fn(&str) -> Result<String, VarError>,
) -> eyre::Result<String> {
    let mut expanded = String::with_capacity(s.len());
    let mut rest = s;

    while let Some(start) = rest.find("${") {
        expanded.push_str(&rest[..start]);

        let reference = &rest[start + 2..];
        let end = reference
            .find('}')
            .ok_or_else(|| eyre::eyre!("`${{` without a closing `}}`"))?;
        let name = &reference[..end];

        if !is_variable_name(name) {
            eyre::bail!(
                "`${{{name}}}` is not a valid environment variable reference"
            );
        }

        let value = env(name).map_err(|err| match err {
            VarError::NotPresent => {
                eyre::eyre!("environment variable {name} is not set")
            }
            VarError::NotUnicode(_) => {
                eyre::eyre!("environment variable {name} is not valid unicode")
            }
        })?;
        expanded.push_str(&value);

        rest = &reference[end + 1..];
    }
    expanded.push_str(rest);

    Ok(expanded)
}

fn is_variable_name(name: &str) -> bool {
    let mut chars = name.chars();

    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use indoc::indoc;
    use test_case::test_case;

    use super::*;

    fn env(name: &str) -> Result<String, VarError> {
        HashMap::from([
            ("RPC_URL", "https://rpc.example.com/v2/secret"),
            ("KEY", "secret"),
            ("EMPTY", ""),
            ("NESTED", "${KEY}"),
        ])
        .get(name)
        .map(|value| value.to_string())
        .ok_or(VarError::NotPresent)
    }

    fn substitute(toml: &str) -> eyre::Result<toml::Value> {
        let mut value = toml.parse()?;
        substitute_env_vars(&mut value, &env)?;

        Ok(value)
    }

    #[test]
    fn substitutes_nested_string_values() -> eyre::Result<()> {
        let value = substitute(indoc! {r#"
            top = "${KEY}"
            number = 1

            [[networks]]
            http_rpcs = ["${RPC_URL}", "https://fallback.example.com/${KEY}"]

            [relayers.key]
            kms = "arn:${KEY}"
        "#})?;

        let expected: toml::Value = indoc! {r#"
            top = "secret"
            number = 1

            [[networks]]
            http_rpcs = ["https://rpc.example.com/v2/secret", "https://fallback.example.com/secret"]

            [relayers.key]
            kms = "arn:secret"
        "#}
        .parse()?;
        assert_eq!(value, expected);

        Ok(())
    }

    #[test_case("plain", "plain" ; "no reference")]
    #[test_case("${KEY}", "secret" ; "whole value")]
    #[test_case("a${KEY}b${KEY}c", "asecretbsecretc" ; "several references")]
    #[test_case("${EMPTY}", "" ; "empty variable")]
    #[test_case("${NESTED}", "${KEY}" ; "substituted values are not expanded")]
    #[test_case("$KEY and $", "$KEY and $" ; "dollar without a brace")]
    fn expands(input: &str, expected: &str) {
        assert_eq!(expand(input, &env).unwrap(), expected);
    }

    #[test_case(
        r#"url = "${MISSING_RPC_URL}""#,
        "url: environment variable MISSING_RPC_URL is not set" ;
        "unset variable"
    )]
    #[test_case(
        r#"networks = [{ http_rpcs = ["${RPC_URL}", "${MISSING}"] }]"#,
        "networks[0].http_rpcs[1]: environment variable MISSING is not set" ;
        "unset variable in a nested array"
    )]
    #[test_case(
        r#"url = "${KEY""#,
        "url: `${` without a closing `}`" ;
        "unterminated reference"
    )]
    #[test_case(
        r#"url = "${}""#,
        "url: `${}` is not a valid environment variable reference" ;
        "empty name"
    )]
    #[test_case(
        r#"url = "${1KEY}""#,
        "url: `${1KEY}` is not a valid environment variable reference" ;
        "name starting with a digit"
    )]
    fn rejects(toml: &str, expected_error: &str) {
        let err = substitute(toml).unwrap_err();

        assert_eq!(err.to_string(), expected_error);
    }
}
