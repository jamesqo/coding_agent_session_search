use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Deserializer, Serialize};

use crate::AppError;

const MAX_CONFIG_BYTES: usize = 1024 * 1024;
const DEFAULT_SINCE_DAYS: u32 = 90;
const MAX_SINCE_DAYS: u32 = 36_500;
const MAX_DEFAULT_REMOTE_NODES: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedConfig {
    pub(crate) path: PathBuf,
    pub(crate) loaded: bool,
    pub(crate) local: Option<ResolvedNode>,
    pub(crate) nodes: Vec<ResolvedNode>,
    pub(crate) providers: ResolvedProviders,
    pub(crate) since_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedNode {
    pub(crate) name: String,
    pub(crate) ssh: String,
    pub(crate) search: bool,
    pub(crate) providers: ResolvedProviders,
    pub(crate) since_days: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResolvedProviders {
    pub(crate) claude_code: Option<Vec<PathBuf>>,
    pub(crate) codex: Option<Vec<PathBuf>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ConfigurationStatus {
    path: PathBuf,
    loaded: bool,
    local_node: Option<String>,
    providers: ProviderStatusMap,
    index: IndexStatus,
}

#[derive(Debug, Serialize)]
struct ProviderStatusMap {
    #[serde(rename = "claude-code", skip_serializing_if = "Option::is_none")]
    claude_code: Option<ProviderStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    codex: Option<ProviderStatus>,
}

#[derive(Debug, Serialize)]
struct ProviderStatus {
    roots: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
struct IndexStatus {
    since_days: Option<u32>,
}

impl ResolvedConfig {
    pub(crate) fn status(&self) -> ConfigurationStatus {
        ConfigurationStatus {
            path: self.path.clone(),
            loaded: self.loaded,
            local_node: self.local.as_ref().map(|node| node.name.clone()),
            providers: ProviderStatusMap {
                claude_code: self
                    .providers
                    .claude_code
                    .clone()
                    .map(|roots| ProviderStatus { roots }),
                codex: self
                    .providers
                    .codex
                    .clone()
                    .map(|roots| ProviderStatus { roots }),
            },
            index: IndexStatus {
                since_days: self.since_days,
            },
        }
    }
}

#[derive(Debug)]
struct ParsedDocument {
    local: ResolvedNode,
    nodes: Vec<ResolvedNode>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDocument {
    version: u32,
    local_node: String,
    nodes: Vec<RawNode>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNode {
    name: String,
    ssh: String,
    search: bool,
    providers: RawProviders,
    #[serde(default)]
    index: RawIndex,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProviders {
    #[serde(rename = "claude-code")]
    claude_code: Option<RawProvider>,
    codex: Option<RawProvider>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProvider {
    roots: Vec<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawIndex {
    #[serde(default)]
    since_days: RawSinceDays,
}

#[derive(Debug)]
struct RawSinceDays(Option<u32>);

impl Default for RawSinceDays {
    fn default() -> Self {
        Self(Some(DEFAULT_SINCE_DAYS))
    }
}

impl<'de> Deserialize<'de> for RawSinceDays {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<u32>::deserialize(deserializer).map(Self)
    }
}

pub(crate) fn load(
    explicit_path: Option<&Path>,
    local_override: Option<&str>,
) -> Result<ResolvedConfig, AppError> {
    load_from_paths(
        default_config_path(),
        explicit_path.map(Path::to_path_buf),
        local_override,
    )
}

fn default_config_path() -> Result<PathBuf, AppError> {
    let path = ProjectDirs::from("dev", "jamesqo", "cass")
        .ok_or_else(|| AppError::configuration("platform configuration directory is unavailable"))?
        .config_dir()
        .join("config.json");
    if !path.is_absolute() {
        return Err(AppError::configuration(
            "platform configuration path is not absolute",
        ));
    }
    Ok(path)
}

fn load_from_paths(
    default_path: Result<PathBuf, AppError>,
    explicit_path: Option<PathBuf>,
    local_override: Option<&str>,
) -> Result<ResolvedConfig, AppError> {
    let explicit = explicit_path.is_some();
    let path = match explicit_path {
        Some(path) => path,
        None => default_path?,
    };

    if !explicit && !path.is_absolute() {
        return Err(AppError::configuration(
            "platform configuration path is not absolute",
        ));
    }

    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound && !explicit => {
            if local_override.is_some() {
                return Err(AppError::configuration(
                    "--local-node requires a loaded configuration",
                ));
            }
            let providers = built_in_providers()?;
            return Ok(ResolvedConfig {
                path,
                loaded: false,
                local: None,
                nodes: Vec::new(),
                providers,
                since_days: Some(DEFAULT_SINCE_DAYS),
            });
        }
        Err(error) => return Err(path_error("inspect", &path, &error)),
    }

    let canonical =
        fs::canonicalize(&path).map_err(|error| path_error("resolve", &path, &error))?;
    let target_metadata =
        fs::metadata(&canonical).map_err(|error| path_error("inspect", &canonical, &error))?;
    if !target_metadata.is_file() {
        return Err(not_regular_file(&canonical));
    }
    let mut file =
        File::open(&canonical).map_err(|error| path_error("open", &canonical, &error))?;
    let metadata = file
        .metadata()
        .map_err(|error| path_error("inspect", &canonical, &error))?;
    if !metadata.is_file() {
        return Err(not_regular_file(&canonical));
    }

    let metadata_size = usize::try_from(metadata.len()).unwrap_or(MAX_CONFIG_BYTES);
    let mut bytes = Vec::with_capacity(MAX_CONFIG_BYTES.min(metadata_size));
    (&mut file)
        .take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| path_error("read", &canonical, &error))?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(AppError::configuration(format!(
            "configuration exceeds {MAX_CONFIG_BYTES} bytes: {}",
            canonical.display()
        )));
    }

    let parsed = parse_document(&bytes)?;
    let selected_name = local_override.unwrap_or(&parsed.local.name);
    let local = find_local(&parsed.nodes, selected_name)?.clone();
    validate_default_remote_count(&parsed.nodes, &local.name)?;

    Ok(ResolvedConfig {
        path: canonical,
        loaded: true,
        providers: local.providers.clone(),
        since_days: local.since_days,
        local: Some(local),
        nodes: parsed.nodes,
    })
}

fn built_in_providers() -> Result<ResolvedProviders, AppError> {
    let directories = BaseDirs::new()
        .ok_or_else(|| AppError::configuration("platform home directory is unavailable"))?;
    let home = directories.home_dir();
    if !home.is_absolute() {
        return Err(AppError::configuration(
            "platform home directory is not absolute",
        ));
    }
    Ok(ResolvedProviders {
        claude_code: Some(vec![
            home.join(".claude/projects"),
            home.join(".config/claude/projects"),
        ]),
        codex: Some(vec![
            home.join(".codex/sessions"),
            home.join(".local/share/codex/sessions"),
        ]),
    })
}

fn not_regular_file(path: &Path) -> AppError {
    AppError::configuration(format!(
        "configuration path is not a regular file: {}",
        path.display()
    ))
}

fn path_error(operation: &str, path: &Path, error: &io::Error) -> AppError {
    AppError::configuration(format!(
        "failed to {operation} configuration {}: {error}",
        path.display()
    ))
}

fn parse_document(bytes: &[u8]) -> Result<ParsedDocument, AppError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| AppError::configuration(format!("invalid configuration JSON: {error}")))?;
    validate_object_shapes(&value)?;
    let raw: RawDocument = serde_json::from_value(value)
        .map_err(|error| AppError::configuration(format!("invalid configuration JSON: {error}")))?;
    if raw.version != 1 {
        return Err(AppError::configuration(format!(
            "unsupported configuration version: {}",
            raw.version
        )));
    }
    if raw.nodes.is_empty() {
        return Err(AppError::configuration(
            "configuration nodes must not be empty",
        ));
    }

    let mut names = HashSet::with_capacity(raw.nodes.len());
    let mut destinations = HashSet::with_capacity(raw.nodes.len());
    let mut nodes = Vec::with_capacity(raw.nodes.len());
    for raw_node in raw.nodes {
        validate_identifier(&raw_node.name, "node name")?;
        if raw_node.name == "local" {
            return Err(AppError::configuration("node name 'local' is reserved"));
        }
        validate_identifier(&raw_node.ssh, "SSH destination")?;
        if !names.insert(raw_node.name.clone()) {
            return Err(AppError::configuration(format!(
                "duplicate node name: {}",
                raw_node.name
            )));
        }
        if !destinations.insert(raw_node.ssh.clone()) {
            return Err(AppError::configuration(format!(
                "duplicate SSH destination: {}",
                raw_node.ssh
            )));
        }
        let since_days = raw_node.index.since_days.0;
        if matches!(since_days, Some(days) if !(1..=MAX_SINCE_DAYS).contains(&days)) {
            return Err(AppError::configuration(format!(
                "index since_days must be null or between 1 and {MAX_SINCE_DAYS}"
            )));
        }
        nodes.push(ResolvedNode {
            name: raw_node.name,
            ssh: raw_node.ssh,
            search: raw_node.search,
            providers: ResolvedProviders {
                claude_code: validate_roots(raw_node.providers.claude_code, "claude-code")?,
                codex: validate_roots(raw_node.providers.codex, "codex")?,
            },
            since_days,
        });
    }

    let local = find_local(&nodes, &raw.local_node)?.clone();
    Ok(ParsedDocument { local, nodes })
}

fn validate_object_shapes(value: &serde_json::Value) -> Result<(), AppError> {
    let document = value
        .as_object()
        .ok_or_else(|| AppError::configuration("configuration document must be a JSON object"))?;
    let Some(nodes) = document.get("nodes") else {
        return Ok(());
    };
    let Some(nodes) = nodes.as_array() else {
        return Ok(());
    };
    for node in nodes {
        let Some(node) = node.as_object() else {
            return Err(AppError::configuration(
                "each configured node must be a JSON object",
            ));
        };
        if let Some(index) = node.get("index")
            && !index.is_object()
        {
            return Err(AppError::configuration("node index must be a JSON object"));
        }
        let Some(providers) = node.get("providers") else {
            continue;
        };
        let Some(providers) = providers.as_object() else {
            return Err(AppError::configuration(
                "node providers must be a JSON object",
            ));
        };
        for provider in providers.values() {
            if !provider.is_object() {
                return Err(AppError::configuration(
                    "each provider configuration must be a JSON object",
                ));
            }
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<(), AppError> {
    let bytes = value.as_bytes();
    let valid = (1..=255).contains(&bytes.len())
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(AppError::configuration(format!("invalid {label}: {value}")))
    }
}

fn validate_roots(
    provider: Option<RawProvider>,
    provider_name: &str,
) -> Result<Option<Vec<PathBuf>>, AppError> {
    let Some(provider) = provider else {
        return Ok(None);
    };
    if provider.roots.is_empty() {
        return Err(AppError::configuration(format!(
            "provider {provider_name} roots must not be empty"
        )));
    }
    let mut unique = HashSet::with_capacity(provider.roots.len());
    for root in &provider.roots {
        if !root.is_absolute() {
            return Err(AppError::configuration(format!(
                "provider {provider_name} root is not absolute: {}",
                root.display()
            )));
        }
        if !unique.insert(root) {
            return Err(AppError::configuration(format!(
                "provider {provider_name} has duplicate root: {}",
                root.display()
            )));
        }
    }
    Ok(Some(provider.roots))
}

fn find_local<'a>(nodes: &'a [ResolvedNode], name: &str) -> Result<&'a ResolvedNode, AppError> {
    nodes
        .iter()
        .find(|node| node.name == name)
        .ok_or_else(|| AppError::configuration(format!("local node is not configured: {name}")))
}

fn validate_default_remote_count(nodes: &[ResolvedNode], local: &str) -> Result<(), AppError> {
    let remote_count = nodes
        .iter()
        .filter(|node| node.name != local && node.search)
        .count();
    if remote_count > MAX_DEFAULT_REMOTE_NODES {
        return Err(AppError::configuration(format!(
            "configuration enables more than {MAX_DEFAULT_REMOTE_NODES} remote search nodes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::TempDir;
    use veritas_test_macros as veritas;

    use super::*;

    const VALID: &str = r#"{
        "version": 1,
        "local_node": "xenia",
        "nodes": [{
            "name": "xenia",
            "ssh": "xenia",
            "search": true,
            "providers": {
                "claude-code": {"roots": ["/home/james/.claude/projects"]},
                "codex": {"roots": ["/home/james/.codex/sessions"]}
            }
        }]
    }"#;

    fn write_config(directory: &TempDir, contents: &str) -> PathBuf {
        let path = directory.path().join("config.json");
        fs::write(&path, contents).expect("temporary configuration is writable");
        path
    }

    fn assert_configuration_error(error: &crate::AppError) {
        assert_eq!(error.code, 9);
        assert_eq!(error.error.kind, "configuration");
        assert!(!error.error.retryable);
        assert_eq!(error.error.recommended_action, None);
    }

    fn assert_configuration_error_message(error: &crate::AppError, fragment: &str) {
        assert_configuration_error(error);
        assert!(
            error.error.message.contains(fragment),
            "expected configuration error message {:?} to contain {fragment:?}",
            error.error.message
        );
    }

    #[veritas::claims(
        "configuration/document-version-and-fields-are-validated",
        "configuration/provider-roots-are-valid"
    )]
    #[test]
    fn document_validation_rejects_versions_unknown_fields_and_providers() {
        let cases = [
            (
                VALID.replacen("\"version\": 1", "\"version\": 2", 1),
                "version",
            ),
            (
                VALID.replacen("\"version\": 1,", "\"version\": 1, \"extra\": true,", 1),
                "document field",
            ),
            (
                VALID.replacen("\"search\": true,", "\"search\": true, \"extra\": true,", 1),
                "node field",
            ),
            (
                VALID.replacen(
                    "{\"roots\": [\"/home/james/.codex/sessions\"]}",
                    "{\"roots\": [\"/home/james/.codex/sessions\"], \"extra\": true}",
                    1,
                ),
                "provider field",
            ),
            (
                VALID.replacen(
                    "\"providers\": {",
                    "\"index\": {\"extra\": true}, \"providers\": {",
                    1,
                ),
                "index field",
            ),
            (
                VALID.replacen("\"codex\":", "\"opencode\":", 1),
                "provider name",
            ),
        ];

        for (document, description) in cases {
            let error = parse_document(document.as_bytes()).expect_err(description);
            assert_configuration_error(&error);
        }
    }

    #[veritas::claims("configuration/document-version-and-fields-are-validated")]
    #[test]
    fn document_validation_rejects_missing_fields_and_wrong_shapes() {
        let base: serde_json::Value = serde_json::from_str(VALID).expect("valid fixture JSON");
        let mut cases = Vec::new();

        let mut missing_version = base.clone();
        missing_version
            .as_object_mut()
            .expect("document object")
            .remove("version");
        cases.push((missing_version, "missing version"));

        let mut missing_local = base.clone();
        missing_local
            .as_object_mut()
            .expect("document object")
            .remove("local_node");
        cases.push((missing_local, "missing local_node"));

        let mut missing_nodes = base.clone();
        missing_nodes
            .as_object_mut()
            .expect("document object")
            .remove("nodes");
        cases.push((missing_nodes, "missing nodes"));

        for field in ["name", "ssh", "search", "providers"] {
            let mut value = base.clone();
            value["nodes"][0]
                .as_object_mut()
                .expect("node object")
                .remove(field);
            cases.push((value, field));
        }

        let mut missing_roots = base.clone();
        missing_roots["nodes"][0]["providers"]["codex"]
            .as_object_mut()
            .expect("provider object")
            .remove("roots");
        cases.push((missing_roots, "missing roots"));

        for (pointer, replacement, description) in [
            ("/version", serde_json::json!("1"), "version type"),
            ("/local_node", serde_json::json!([]), "local_node type"),
            ("/nodes", serde_json::json!({}), "nodes type"),
            ("/nodes/0", serde_json::json!("node"), "node type"),
            (
                "/nodes/0/providers",
                serde_json::json!([]),
                "providers type",
            ),
            (
                "/nodes/0/providers/codex",
                serde_json::json!([]),
                "provider type",
            ),
            (
                "/nodes/0/providers/codex/roots",
                serde_json::json!("/tmp"),
                "roots type",
            ),
        ] {
            let mut value = base.clone();
            *value.pointer_mut(pointer).expect("fixture pointer exists") = replacement;
            cases.push((value, description));
        }

        cases.push((
            serde_json::json!({"version": 1, "local_node": "xenia", "nodes": []}),
            "empty nodes",
        ));

        for (value, description) in cases {
            let bytes = serde_json::to_vec(&value).expect("fixture serializes");
            assert_configuration_error(&parse_document(&bytes).expect_err(description));
        }

        let wrong_index = VALID.replacen("\"providers\": {", "\"index\": [], \"providers\": {", 1);
        assert_configuration_error(
            &parse_document(wrong_index.as_bytes()).expect_err("index type"),
        );
    }

    #[veritas::claims("configuration/node-inventory-is-valid")]
    #[test]
    fn node_inventory_and_local_identity_are_exact_and_bounded() {
        let duplicate = VALID.replacen(
            "]\n    }",
            ", {\"name\": \"dev\", \"ssh\": \"xenia\", \"search\": false, \"providers\": {}}]\n    }",
            1,
        );
        let duplicate_name = VALID.replacen(
            "]\n    }",
            ", {\"name\": \"xenia\", \"ssh\": \"dev\", \"search\": false, \"providers\": {}}]\n    }",
            1,
        );
        let invalid_name = VALID.replacen("\"name\": \"xenia\"", "\"name\": \"bad name\"", 1);
        let option_shaped_ssh = VALID.replacen("\"ssh\": \"xenia\"", "\"ssh\": \"-xenia\"", 1);
        let too_long = "x".repeat(256);
        let too_long_name = VALID
            .replacen(
                "\"local_node\": \"xenia\"",
                &format!("\"local_node\": \"{too_long}\""),
                1,
            )
            .replacen(
                "\"name\": \"xenia\"",
                &format!("\"name\": \"{too_long}\""),
                1,
            );
        let reserved_name = VALID
            .replacen("\"name\": \"xenia\"", "\"name\": \"local\"", 1)
            .replacen("\"local_node\": \"xenia\"", "\"local_node\": \"local\"", 1);
        let missing_local = VALID.replacen(
            "\"local_node\": \"xenia\"",
            "\"local_node\": \"missing\"",
            1,
        );

        for (document, description) in [
            (duplicate, "duplicate SSH destination"),
            (duplicate_name, "duplicate node name"),
            (invalid_name, "invalid node name"),
            (option_shaped_ssh, "option-shaped SSH destination"),
            (too_long_name, "overlong node name"),
            (reserved_name, "reserved node name"),
            (missing_local, "missing local node"),
        ] {
            let error = parse_document(document.as_bytes()).expect_err(description);
            assert_configuration_error(&error);
        }

        let boundary = "x".repeat(255);
        let valid_boundary = VALID
            .replacen(
                "\"local_node\": \"xenia\"",
                &format!("\"local_node\": \"{boundary}\""),
                1,
            )
            .replacen(
                "\"name\": \"xenia\"",
                &format!("\"name\": \"{boundary}\""),
                1,
            );
        parse_document(valid_boundary.as_bytes()).expect("255-byte node name is valid");
    }

    #[veritas::claims("configuration/provider-roots-are-valid")]
    #[test]
    fn provider_roots_are_absolute_unique_and_never_probed() {
        let relative = VALID.replacen("/home/james/.codex/sessions", "relative/codex", 1);
        let duplicate = VALID.replacen(
            "[\"/home/james/.codex/sessions\"]",
            "[\"/missing/remote\", \"/missing/remote\"]",
            1,
        );
        let empty = VALID.replacen("[\"/home/james/.codex/sessions\"]", "[]", 1);
        for (document, description) in [
            (relative, "relative root"),
            (duplicate, "duplicate root"),
            (empty, "empty roots"),
        ] {
            assert_configuration_error(
                &parse_document(document.as_bytes()).expect_err(description),
            );
        }

        let remote = VALID.replacen("/home/james/.codex/sessions", "/definitely/not/local", 1);
        let resolved = parse_document(remote.as_bytes()).expect("remote roots are lexical");
        assert_eq!(
            resolved.local.providers.codex,
            Some(vec![PathBuf::from("/definitely/not/local")])
        );

        let empty_providers = VALID.replacen(
            r#""providers": {
                "claude-code": {"roots": ["/home/james/.claude/projects"]},
                "codex": {"roots": ["/home/james/.codex/sessions"]}
            }"#,
            r#""providers": {}"#,
            1,
        );
        let resolved =
            parse_document(empty_providers.as_bytes()).expect("empty providers are valid");
        assert_eq!(resolved.local.providers, ResolvedProviders::default());
    }

    #[veritas::claims("configuration/index-horizon-is-valid")]
    #[test]
    fn horizon_defaults_accepts_bounds_and_null() {
        let cases = [
            (VALID.to_owned(), Some(90)),
            (
                VALID.replacen(
                    "\"providers\": {",
                    "\"index\": {\"since_days\": 1}, \"providers\": {",
                    1,
                ),
                Some(1),
            ),
            (
                VALID.replacen(
                    "\"providers\": {",
                    "\"index\": {\"since_days\": 36500}, \"providers\": {",
                    1,
                ),
                Some(36500),
            ),
            (
                VALID.replacen(
                    "\"providers\": {",
                    "\"index\": {\"since_days\": null}, \"providers\": {",
                    1,
                ),
                None,
            ),
        ];
        for (document, expected) in cases {
            assert_eq!(
                parse_document(document.as_bytes())
                    .expect("valid horizon")
                    .local
                    .since_days,
                expected
            );
        }

        for value in ["0", "36501", "-1", "1.5", "\"90\""] {
            let document = VALID.replacen(
                "\"providers\": {",
                &format!("\"index\": {{\"since_days\": {value}}}, \"providers\": {{"),
                1,
            );
            assert_configuration_error(&parse_document(document.as_bytes()).expect_err(value));
        }
    }

    #[test]
    fn loading_distinguishes_optional_default_and_required_explicit_paths() {
        let directory = TempDir::new().expect("temporary directory");
        let missing = directory.path().join("missing.json");
        let absent = load_from_paths(Ok(missing.clone()), None, None)
            .expect("an absent default is optional");
        assert!(!absent.loaded);
        assert_eq!(absent.path, missing);
        assert!(absent.local.is_none());
        assert_eq!(absent.since_days, Some(90));
        assert_eq!(
            absent
                .providers
                .claude_code
                .as_ref()
                .expect("built-in Claude roots")
                .len(),
            2
        );
        assert_eq!(
            absent
                .providers
                .codex
                .as_ref()
                .expect("built-in Codex roots")
                .len(),
            2
        );
        assert!(!absent.path.exists(), "optional default is not created");

        assert_configuration_error(
            &load_from_paths(Ok(missing.clone()), Some(missing.clone()), None)
                .expect_err("an explicit file is required"),
        );
        assert_configuration_error(
            &load_from_paths(Ok(missing), None, Some("xenia"))
                .expect_err("override requires loaded configuration"),
        );

        let path = write_config(&directory, VALID);
        let loaded = load_from_paths(Ok(path.clone()), Some(path), None)
            .expect("valid explicit configuration");
        assert!(loaded.loaded);
        assert!(loaded.path.is_absolute());
        assert_eq!(loaded.providers.codex.as_ref().map(Vec::len), Some(1));
        assert_eq!(loaded.since_days, Some(90));
        assert_eq!(loaded.local.expect("loaded local node").name, "xenia");
    }

    #[test]
    fn local_override_does_not_excuse_an_invalid_document_identity() {
        let directory = TempDir::new().expect("temporary directory");
        let second_node = r#", {
            "name": "dev-macbook", "ssh": "dev-macbook", "search": true,
            "providers": {}
        }"#;
        let valid = VALID.replacen("}]", &format!("}}{second_node}]"), 1);
        let path = write_config(&directory, &valid);
        let overridden = load_from_paths(Ok(path.clone()), Some(path), Some("dev-macbook"))
            .expect("exact alternate local node");
        assert_eq!(
            overridden.local.expect("overridden local node").name,
            "dev-macbook"
        );

        let invalid = valid.replacen(
            "\"local_node\": \"xenia\"",
            "\"local_node\": \"missing\"",
            1,
        );
        let invalid_path = write_config(&directory, &invalid);
        assert_configuration_error(
            &load_from_paths(
                Ok(invalid_path.clone()),
                Some(invalid_path),
                Some("dev-macbook"),
            )
            .expect_err("document identity must be valid first"),
        );

        let valid_path = write_config(&directory, &valid);
        assert_configuration_error(
            &load_from_paths(Ok(valid_path.clone()), Some(valid_path), Some("unknown"))
                .expect_err("override must name an exact node"),
        );
    }

    #[veritas::claims("configuration/document-version-and-fields-are-validated")]
    #[test]
    fn file_input_is_bounded_before_json_parsing() {
        let directory = TempDir::new().expect("temporary directory");
        let mut exactly = VALID.as_bytes().to_vec();
        exactly.resize(MAX_CONFIG_BYTES, b' ');
        let path = directory.path().join("exact.json");
        fs::write(&path, exactly).expect("bounded file is writable");
        load_from_paths(Ok(path.clone()), Some(path), None).expect("exactly 1 MiB");

        let too_large = directory.path().join("large.json");
        let bytes = vec![b'{'; MAX_CONFIG_BYTES + 1];
        fs::write(&too_large, bytes).expect("oversized file is writable");
        assert_configuration_error_message(
            &load_from_paths(Ok(too_large.clone()), Some(too_large), None)
                .expect_err("1 MiB plus one byte"),
            "exceeds 1048576 bytes",
        );
    }

    #[cfg(unix)]
    #[veritas::claims("configuration/document-version-and-fields-are-validated")]
    #[test]
    fn symlink_must_resolve_to_a_regular_file() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().expect("temporary directory");
        let target = write_config(&directory, VALID);
        let link = directory.path().join("link.json");
        symlink(&target, &link).expect("file symlink");
        let loaded =
            load_from_paths(Ok(link.clone()), Some(link), None).expect("symlink to regular file");
        assert_eq!(
            loaded.path,
            fs::canonicalize(target).expect("canonical target")
        );

        let directory_link = directory.path().join("directory.json");
        symlink(directory.path(), &directory_link).expect("directory symlink");
        assert_configuration_error(
            &load_from_paths(Ok(directory_link.clone()), Some(directory_link), None)
                .expect_err("symlink to directory"),
        );

        let broken = directory.path().join("broken.json");
        symlink(directory.path().join("absent"), &broken).expect("broken symlink");
        assert_configuration_error(
            &load_from_paths(Ok(broken.clone()), Some(broken), None).expect_err("broken symlink"),
        );
    }

    #[veritas::claims("configuration/document-version-and-fields-are-validated")]
    #[test]
    fn explicit_directory_and_malformed_file_are_typed_errors() {
        let directory = TempDir::new().expect("temporary directory");
        let directory_path = directory.path().to_path_buf();
        assert_configuration_error(
            &load_from_paths(Ok(directory_path.clone()), Some(directory_path), None)
                .expect_err("directory is not a configuration file"),
        );

        let malformed = write_config(&directory, "{");
        assert_configuration_error_message(
            &load_from_paths(Ok(malformed.clone()), Some(malformed), None)
                .expect_err("malformed JSON"),
            "invalid configuration JSON",
        );
    }

    #[veritas::claims("configuration/node-inventory-is-valid")]
    #[test]
    fn default_remote_search_membership_is_bounded() {
        let mut value: serde_json::Value = serde_json::from_str(VALID).expect("valid fixture");
        value["nodes"][0]["search"] = serde_json::json!(false);
        let nodes = value["nodes"].as_array_mut().expect("nodes array");
        for index in 0..16 {
            nodes.push(serde_json::json!({
                "name": format!("remote-{index}"),
                "ssh": format!("remote-{index}"),
                "search": true,
                "providers": {}
            }));
        }
        let directory = TempDir::new().expect("temporary directory");
        let path = write_config(
            &directory,
            &serde_json::to_string(&value).expect("fixture serializes"),
        );
        load_from_paths(Ok(path.clone()), Some(path.clone()), None)
            .expect("exactly 16 default remotes");

        value["nodes"]
            .as_array_mut()
            .expect("nodes array")
            .push(serde_json::json!({
                "name": "remote-16",
                "ssh": "remote-16",
                "search": true,
                "providers": {}
            }));
        fs::write(
            &path,
            serde_json::to_vec(&value).expect("fixture serializes"),
        )
        .expect("configuration is writable");
        assert_configuration_error_message(
            &load_from_paths(Ok(path.clone()), Some(path.clone()), None)
                .expect_err("more than 16 default remotes"),
            "more than 16",
        );
        load_from_paths(Ok(path.clone()), Some(path), Some("remote-0"))
            .expect("override leaves exactly 16 enabled nonlocal nodes");
    }

    #[cfg(unix)]
    #[test]
    fn fifo_is_rejected_without_blocking_on_open() {
        let directory = TempDir::new().expect("temporary directory");
        let fifo = directory.path().join("config.fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo is available on supported Unix platforms");
        assert!(status.success(), "mkfifo creates the test FIFO");
        assert_configuration_error_message(
            &load_from_paths(Ok(fifo.clone()), Some(fifo), None)
                .expect_err("FIFO is not a regular file"),
            "not a regular file",
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_failure_is_typed_when_permissions_are_enforced() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().expect("temporary directory");
        let path = write_config(&directory, VALID);
        let mut permissions = fs::metadata(&path)
            .expect("configuration metadata")
            .permissions();
        permissions.set_mode(0o000);
        fs::set_permissions(&path, permissions).expect("permissions are mutable");

        let result = load_from_paths(Ok(path.clone()), Some(path.clone()), None);

        let mut restored = fs::metadata(&path)
            .expect("configuration metadata")
            .permissions();
        restored.set_mode(0o600);
        fs::set_permissions(&path, restored).expect("permissions are restorable");

        let error = result.expect_err("permission bits make the configuration unreadable");
        assert_configuration_error_message(&error, "failed to open");
    }

    #[test]
    fn default_path_failure_is_typed() {
        let error = load_from_paths(
            Err(crate::AppError::configuration(
                "no absolute config directory",
            )),
            None,
            None,
        )
        .expect_err("missing platform path");
        assert_configuration_error(&error);
    }
}
