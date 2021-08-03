use config::{FileFormat, Value};
use ipnet::{AddrParseError, Ipv4Net};
use iprange::IpRange;
use std::collections::HashMap;

/// CDN configuration.
pub struct Config {
    /// IPs which may purge the cache (empty = deny all)
    pub acl_fastlypurge: IpRange<Ipv4Net>,
    /// IPs which may make requests (empty = allow all)
    pub acl_allowlist: IpRange<Ipv4Net>,
    /// IPs which may NOT make requests (empty = allow all)
    pub acl_denylist: IpRange<Ipv4Net>,
    /// HTTP Basic Auth credentials
    pub basic_authorization: Option<String>,
    /// Paths to return a 404 for
    pub synthetic_not_found: Vec<String>,
    /// Paths to return a 302 for (and their destination)
    pub synthetic_redirect: HashMap<String, String>,
    /// Mirror configuration
    pub mirrors: HashMap<String, MirrorConfig>,
    /// A/B test configuration
    pub ab_tests: HashMap<String, ABTestConfig>,
}

/// Mirror configuration.
pub struct MirrorConfig {
    /// Path prefix
    pub prefix: Option<String>,
}

/// A/B test configuration.
pub struct ABTestConfig {
    /// Is it active?
    pub active: bool,
    /// Cookie expiration (= bucket re-assignment) time.
    pub expires: i64,
    /// Variant names and weightings
    pub variants: HashMap<String, i64>,
    /// Variant used for the crawler worker (defaults to "A" if the
    /// `crawler_variant` field is not set and there is an `A`
    /// variant).
    pub crawler_variant: String,
}

/// An error when parsing configuration.
pub enum ParseError {
    InvalidKey(String),
    MissingKey(String),
    InvalidYaml,
}

/// Parse a YAML configuration string.
pub fn parse_config(config_str: &str) -> Result<Config, ParseError> {
    let mut settings = config::Config::new();
    settings
        .merge(config::File::from_str(config_str, FileFormat::Yaml))
        .map_err(|_| ParseError::InvalidYaml)?;

    let acl_fastlypurge = parse_acl(&settings, "acl.fastlypurge")?;
    let acl_allowlist = parse_acl(&settings, "acl.allowlist")?;
    let acl_denylist = parse_acl(&settings, "acl.denylist")?;
    let basic_authorization = settings.get_str("basic_authorization").ok();
    let synthetic_not_found = parse_array_of_strings(&settings, "special_paths.not_found")?;
    let synthetic_redirect = parse_map_of_strings(&settings, "special_paths.redirect")?;
    let mirrors = parse_map_of_mirrors(&settings, "mirrors")?;
    let ab_tests = parse_map_of_ab_tests(&settings, "ab_tests")?;

    Ok(Config {
        acl_fastlypurge: acl_fastlypurge,
        acl_allowlist: acl_allowlist,
        acl_denylist: acl_denylist,
        basic_authorization: basic_authorization,
        synthetic_not_found: synthetic_not_found,
        synthetic_redirect: synthetic_redirect,
        mirrors: mirrors,
        ab_tests: ab_tests,
    })
}

/// Get an ACL from the settings.
fn parse_acl(settings: &config::Config, key: &str) -> Result<IpRange<Ipv4Net>, ParseError> {
    let values = parse_array_of_strings(settings, key)?;

    let networks = values
        .iter()
        .map(|s| s.parse())
        .collect::<Result<Vec<Ipv4Net>, AddrParseError>>()
        .map_err(|_| ParseError::InvalidKey(key.to_string()))?;

    Ok(networks.into_iter().collect())
}

/// Get an array of `String`s from the settings.
fn parse_array_of_strings(settings: &config::Config, key: &str) -> Result<Vec<String>, ParseError> {
    let array = parse_array(settings, key)?;
    parse_values_to_strings(array, key)
}

/// Get a map of `String`s from the settings.
fn parse_map_of_strings(
    settings: &config::Config,
    key: &str,
) -> Result<HashMap<String, String>, ParseError> {
    let map = parse_map(settings, key)?;
    let mut new_map = HashMap::new();
    for (mkey, value) in map.iter() {
        let parsed = parse_value_to_string(value, &format!("{}.{}", key, mkey))?;
        new_map.insert(mkey.clone(), parsed);
    }
    Ok(new_map)
}

/// Get a map of `MirrorConfig`s from the settings.
fn parse_map_of_mirrors(
    settings: &config::Config,
    key: &str,
) -> Result<HashMap<String, MirrorConfig>, ParseError> {
    let map = parse_map(settings, key)?;
    let mut new_map = HashMap::new();
    for (mkey, value) in map.iter() {
        let parsed = parse_value_to_mirror(value, &format!("{}.{}", key, mkey))?;
        new_map.insert(mkey.clone(), parsed);
    }
    Ok(new_map)
}

/// Get a map of `ABTestConfig`s from the settings.
fn parse_map_of_ab_tests(
    settings: &config::Config,
    key: &str,
) -> Result<HashMap<String, ABTestConfig>, ParseError> {
    let map = parse_map(settings, key)?;
    let mut new_map = HashMap::new();
    for (mkey, value) in map.iter() {
        let parsed = parse_value_to_ab_test(value, &format!("{}.{}", key, mkey))?;
        new_map.insert(mkey.clone(), parsed);
    }
    Ok(new_map)
}

/// Turn an array of `Value`s into an array of `String`s
fn parse_values_to_strings(values: Vec<Value>, key: &str) -> Result<Vec<String>, ParseError> {
    values
        .into_iter()
        .map(|s| parse_value_to_string(&s, key))
        .collect()
}

/// Turn a `Value` into a `MirrorConfig`.
fn parse_value_to_mirror(value: &Value, key: &str) -> Result<MirrorConfig, ParseError> {
    let table = value
        .clone()
        .into_table()
        .map_err(|_| ParseError::InvalidKey(key.to_string()))?;

    let mut prefix = None;
    if let Some(value) = table.get("prefix") {
        let prefix_string = parse_value_to_string(value, &format!("{}.prefix", key))?;
        prefix = Some(prefix_string);
    }

    Ok(MirrorConfig { prefix: prefix })
}

/// Turn a `Value` into an `ABTestConfig`.
fn parse_value_to_ab_test(value: &Value, key: &str) -> Result<ABTestConfig, ParseError> {
    let table = value
        .clone()
        .into_table()
        .map_err(|_| ParseError::InvalidKey(key.to_string()))?;

    let active = match table.get("active") {
        Some(value) => parse_value_to_bool(&value, &format!("{}.active", key)),
        None => Ok(false),
    }?;

    let expires = match table.get("expires") {
        Some(value) => parse_value_to_int(&value, &format!("{}.expires", key)),
        None => Err(ParseError::MissingKey(format!("{}.expires", key))),
    }?;

    let variants = match table.get("variants") {
        Some(value) => {
            let map = value
                .clone()
                .into_table()
                .map_err(|_| ParseError::InvalidKey(format!("{}.variants", key)))?;
            let mut new_map = HashMap::new();
            for (mkey, value) in map.iter() {
                let parsed = parse_value_to_int(value, &format!("{}.variants.{}", key, mkey))?;
                new_map.insert(mkey.clone(), parsed);
            }
            Ok(new_map)
        }
        None => Err(ParseError::MissingKey(format!("{}.variants", key))),
    }?;

    let crawler_variant = match table.get("crawler_variant") {
        Some(value) => parse_value_to_string(&value, &format!("{}.crawler_variant", key)),
        None if variants.contains_key("A") => Ok("A".to_string()),
        _ => Err(ParseError::MissingKey(format!("{}.crawler_variant", key))),
    }?;

    Ok(ABTestConfig {
        active: active,
        expires: expires,
        variants: variants,
        crawler_variant: crawler_variant,
    })
}

/// Turn a `Value` into a `String`.
fn parse_value_to_string(value: &Value, key: &str) -> Result<String, ParseError> {
    value
        .clone()
        .into_str()
        .map_err(|_| ParseError::InvalidKey(key.to_string()))
}

/// Turn a `Value` into a `bool`.
fn parse_value_to_bool(value: &Value, key: &str) -> Result<bool, ParseError> {
    value
        .clone()
        .into_bool()
        .map_err(|_| ParseError::InvalidKey(key.to_string()))
}

/// Turn a `Value` into a `i64`.
fn parse_value_to_int(value: &Value, key: &str) -> Result<i64, ParseError> {
    value
        .clone()
        .into_int()
        .map_err(|_| ParseError::InvalidKey(key.to_string()))
}

/// Get an array from the settings.
fn parse_array(settings: &config::Config, key: &str) -> Result<Vec<Value>, ParseError> {
    settings
        .get_array(key)
        .map_err(|_| ParseError::MissingKey(key.to_string()))
}

/// Get a map from the settings.
fn parse_map(settings: &config::Config, key: &str) -> Result<HashMap<String, Value>, ParseError> {
    settings
        .get_table(key)
        .map_err(|_| ParseError::MissingKey(key.to_string()))
}
