use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RemoteConfig {
    pub kind: String,
    pub target: String,
    pub root: String,
    #[serde(default)]
    pub runtime: Option<String>,
}

/// Session-only approval supplied by the trusted editor after an explicit user
/// confirmation. This is not an authentication secret or a sandbox for LSPs.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectApproval {
    pub root: String,
    pub path: String,
    pub sha256: String,
    pub remote: Option<RemoteConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(rename = "languageServers", default)]
    pub language_servers: HashMap<String, ServerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub filetypes: Vec<String>,
    #[serde(rename = "rootPatterns", default)]
    pub root_patterns: Vec<String>,
    #[serde(rename = "initializationOptions")]
    pub initialization_options: Option<serde_json::Value>,
    /// Values returned for server-initiated `workspace/configuration`
    /// requests. Nested objects and exact dotted keys are both supported.
    #[serde(default)]
    pub settings: Option<serde_json::Value>,
    /// Higher values win when more than one server handles the same filetype.
    /// The server name is used as a stable tie breaker.
    #[serde(default)]
    pub priority: i32,
}

impl ServerConfig {
    /// Apply editor-compatible defaults while preserving explicit user
    /// overrides from simplecc.json.
    pub fn effective_initialization_options(&self, server_name: &str) -> Option<Value> {
        let defaults = match server_name {
            "julia-lsp" => json!({ "useFormatterConfigDefaults": true }),
            _ => Value::Null,
        };
        merge_json(defaults, self.initialization_options.clone())
    }

    pub fn effective_settings(&self, server_name: &str) -> Option<Value> {
        let defaults = match server_name {
            // Mirrors the Julia VS Code extension's language-server settings.
            // Hints remain enabled server-side because SimpleCC owns the
            // client-side display toggle.
            "julia-lsp" => json!({
                "julia": {
                    "lint": {
                        "call": true,
                        "iter": true,
                        "nothingcomp": true,
                        "constif": true,
                        "lazy": true,
                        "datadecl": true,
                        "typeparam": true,
                        "modname": true,
                        "pirates": true,
                        "useoffuncargs": true,
                        "run": true,
                        "missingrefs": "none",
                        "disabledDirs": ["docs", "test"]
                    },
                    "completionmode": "qualify",
                    "inlayHints": {
                        "static": {
                            "enabled": true,
                            "variableTypes": { "enabled": true },
                            "parameterNames": { "enabled": "literals" }
                        }
                    }
                }
            }),
            _ => Value::Null,
        };
        merge_json(defaults, self.settings.clone())
    }
}

fn merge_json(mut defaults: Value, overrides: Option<Value>) -> Option<Value> {
    let Some(overrides) = overrides else {
        return (!defaults.is_null()).then_some(defaults);
    };
    merge_json_value(&mut defaults, overrides);
    Some(defaults)
}

fn merge_json_value(base: &mut Value, override_value: Value) {
    match (base, override_value) {
        (Value::Object(base), Value::Object(overrides)) => {
            for (key, value) in overrides {
                if let Some(current) = base.get_mut(&key) {
                    merge_json_value(current, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, value) => *base = value,
    }
}

/// Startup script for LanguageServer.jl. Loads the server from the dedicated
/// `@simplecc` shared environment (~/.julia/environments/simplecc) so it never
/// pollutes or conflicts with the project being edited, then points the server
/// at the user's own project (detected from cwd / load path).
const JULIA_LSP_SCRIPT: &str = concat!(
    "ls_env = joinpath(get(DEPOT_PATH, 1, joinpath(homedir(), \".julia\")), \"environments\", \"simplecc\"); ",
    "pushfirst!(LOAD_PATH, ls_env); ",
    "using LanguageServer; ",
    "popfirst!(LOAD_PATH); ",
    "depot = get(ENV, \"JULIA_DEPOT_PATH\", \"\"); ",
    "project = dirname(something(Base.current_project(pwd()), get(Base.load_path(), 1, nothing), Base.active_project())); ",
    "server = LanguageServer.LanguageServerInstance(stdin, stdout, project, depot); ",
    "run(server)"
);

impl Config {
    pub fn apply_python_environment(&mut self, python_path: &str, lsp_path: &str) {
        for server in self
            .language_servers
            .values_mut()
            .filter(|server| server.filetypes.iter().any(|filetype| filetype == "python"))
        {
            if !lsp_path.is_empty() {
                server.command = lsp_path.to_string();
            }
            if !python_path.is_empty() {
                let mut settings = server.settings.take().unwrap_or_else(|| json!({}));
                merge_json_value(
                    &mut settings,
                    json!({
                        "python": {
                            "pythonPath": python_path,
                            "defaultInterpreterPath": python_path
                        }
                    }),
                );
                server.settings = Some(settings);
            }
        }
    }

    /// Load config from a path.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read configuration {}", path.display()))?;
        Self::parse(&content)
            .map_err(|error| anyhow::anyhow!("invalid configuration {}: {error:#}", path.display()))
    }

    pub fn parse(content: &str) -> Result<Self> {
        let config: Self = serde_json::from_str(content).context("invalid SimpleCC JSON")?;
        // Remote workspaces pass the configuration text directly and never
        // call load().  Validation belongs here so local and remote configs
        // reject the same semantic errors.
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        for (name, server) in &self.language_servers {
            if name.trim().is_empty() {
                bail!("language server names must not be empty");
            }
            if server.command.trim().is_empty() {
                bail!("language server '{name}' has an empty command");
            }
            if server
                .filetypes
                .iter()
                .any(|filetype| filetype.trim().is_empty())
            {
                bail!("language server '{name}' contains an empty filetype");
            }
            if server
                .root_patterns
                .iter()
                .any(|pattern| pattern.trim().is_empty())
            {
                // directory.join("") is the directory itself and therefore
                // always exists.  Accepting an empty marker silently made
                // every source directory its own server root.
                bail!("language server '{name}' contains an empty root pattern");
            }
        }
        Ok(())
    }

    /// Load the currently selected configuration without hiding parse or I/O
    /// errors. Used by live reload so an invalid edit never silently replaces
    /// the running settings with defaults.
    #[cfg(test)]
    pub fn load_selected(project_root: &str, explicit_path: Option<&str>) -> Result<Self> {
        Self::load_workspace(project_root, explicit_path, None, None, None)
    }

    pub fn load_workspace(
        project_root: &str,
        explicit_path: Option<&str>,
        remote: Option<&RemoteConfig>,
        remote_content: Option<&str>,
        approval: Option<&ProjectApproval>,
    ) -> Result<Self> {
        if let Some(remote) = remote {
            // Never discover a remote workspace's configuration on the local disk.
            if let Some(content) = remote_content {
                Self::check_approval(project_root, "", Some(remote), content, approval)?;
                return Self::parse(content);
            }
            return Self::load_global();
        }
        if remote_content.is_some() {
            bail!("remote configuration requires a remote workspace identity");
        }
        // This path is an explicit user/editor override, not project discovery.
        if let Some(path) = explicit_path.filter(|path| !path.is_empty()) {
            return Self::load(Path::new(path));
        }
        for name in ["simplecc.json", ".simplecc.json"] {
            let path = Path::new(project_root).join(name);
            if path.exists() {
                let root = std::fs::canonicalize(project_root)?;
                let path = std::fs::canonicalize(path)?;
                let content = std::fs::read_to_string(&path)?;
                Self::check_approval(
                    &root.to_string_lossy(),
                    &path.to_string_lossy(),
                    None,
                    &content,
                    approval,
                )?;
                // Parse precisely the bytes checked above; never reopen after approval.
                return Self::parse(&content);
            }
        }
        Self::load_global()
    }

    fn check_approval(
        root: &str,
        path: &str,
        remote: Option<&RemoteConfig>,
        content: &str,
        approval: Option<&ProjectApproval>,
    ) -> Result<()> {
        let digest = format!("{:x}", Sha256::digest(content.as_bytes()));
        if approval.is_some_and(|a| {
            a.root == root && a.path == path && a.remote.as_ref() == remote && a.sha256 == digest
        }) {
            return Ok(());
        }
        bail!(
            "project configuration is not approved or has changed; review it and run :SimpleCCTrustProject (session-only)"
        )
    }

    fn load_global() -> Result<Self> {
        if let Some(path) = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".config/simplecc/simplecc.json"))
            .filter(|path| path.exists())
        {
            return Self::load(&path);
        }
        Ok(Self::default())
    }

    /// Find the deterministic primary server for a filetype. Higher explicit
    /// priority wins; names provide a stable tie breaker for legacy configs.
    pub fn server_for_filetype(&self, filetype: &str) -> Option<(&str, &ServerConfig)> {
        self.servers_for_filetype(filetype).into_iter().next()
    }

    /// Find all servers that handle a given filetype.
    pub fn servers_for_filetype(&self, filetype: &str) -> Vec<(&str, &ServerConfig)> {
        let mut servers: Vec<_> = self
            .language_servers
            .iter()
            .filter(|(_, cfg)| cfg.filetypes.iter().any(|ft| ft == filetype))
            .map(|(name, cfg)| (name.as_str(), cfg))
            .collect();
        servers.sort_by(|(left_name, left), (right_name, right)| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left_name.cmp(right_name))
        });
        servers
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut servers = HashMap::new();

        servers.insert(
            "rust-analyzer".to_string(),
            ServerConfig {
                command: "rust-analyzer".to_string(),
                args: vec![],
                filetypes: vec!["rust".to_string()],
                root_patterns: vec!["Cargo.toml".to_string()],
                initialization_options: None,
                settings: None,
                priority: 0,
            },
        );

        servers.insert(
            "clangd".to_string(),
            ServerConfig {
                command: "clangd".to_string(),
                args: vec![],
                filetypes: vec!["c".to_string(), "cpp".to_string()],
                root_patterns: vec!["compile_commands.json".to_string(), ".clangd".to_string()],
                initialization_options: None,
                settings: None,
                priority: 0,
            },
        );

        servers.insert(
            "pyright".to_string(),
            ServerConfig {
                command: "pyright-langserver".to_string(),
                args: vec!["--stdio".to_string()],
                filetypes: vec!["python".to_string()],
                root_patterns: vec!["pyproject.toml".to_string(), "setup.py".to_string()],
                initialization_options: None,
                settings: None,
                priority: 0,
            },
        );

        servers.insert(
            "typescript-language-server".to_string(),
            ServerConfig {
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
                filetypes: vec![
                    "typescript".to_string(),
                    "javascript".to_string(),
                    "typescriptreact".to_string(),
                    "javascriptreact".to_string(),
                ],
                root_patterns: vec!["package.json".to_string(), "tsconfig.json".to_string()],
                initialization_options: None,
                settings: None,
                priority: 0,
            },
        );

        servers.insert(
            "lua-language-server".to_string(),
            ServerConfig {
                command: "lua-language-server".to_string(),
                args: vec![],
                filetypes: vec!["lua".to_string()],
                root_patterns: vec![".luarc.json".to_string()],
                initialization_options: None,
                settings: None,
                priority: 0,
            },
        );

        servers.insert(
            "gopls".to_string(),
            ServerConfig {
                command: "gopls".to_string(),
                args: vec![],
                filetypes: vec!["go".to_string(), "gomod".to_string()],
                root_patterns: vec!["go.mod".to_string()],
                initialization_options: None,
                settings: None,
                priority: 0,
            },
        );

        // LanguageServer.jl runs through the `julia` binary, loaded from the
        // dedicated `@simplecc` named environment (see JULIA_LSP_SCRIPT).
        servers.insert(
            "julia-lsp".to_string(),
            ServerConfig {
                command: "julia".to_string(),
                args: vec![
                    "--startup-file=no".to_string(),
                    "--history-file=no".to_string(),
                    "-e".to_string(),
                    JULIA_LSP_SCRIPT.to_string(),
                ],
                filetypes: vec!["julia".to_string()],
                root_patterns: vec!["Project.toml".to_string(), "JuliaProject.toml".to_string()],
                initialization_options: Some(json!({ "useFormatterConfigDefaults": true })),
                settings: None,
                priority: 0,
            },
        );

        Config {
            language_servers: servers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "simplecc-trust-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        std::fs::canonicalize(path).unwrap()
    }

    fn approval(root: &Path, path: &Path, content: &str) -> ProjectApproval {
        ProjectApproval {
            root: root.to_string_lossy().into_owned(),
            path: path.to_string_lossy().into_owned(),
            sha256: format!("{:x}", Sha256::digest(content.as_bytes())),
            remote: None,
        }
    }

    #[test]
    fn project_requires_exact_content_source_and_workspace_approval() {
        let root = workspace("exact");
        let path = root.join("simplecc.json");
        let content = r#"{"languageServers":{}}"#;
        std::fs::write(&path, content).unwrap();
        let load = |a: Option<&ProjectApproval>| {
            Config::load_workspace(root.to_str().unwrap(), None, None, None, a)
        };
        assert!(load(None).unwrap_err().to_string().contains("not approved"));
        let good = approval(&root, &path, content);
        assert!(load(Some(&good)).unwrap().language_servers.is_empty());
        for changed in [
            ProjectApproval {
                root: "/other".into(),
                ..good.clone()
            },
            ProjectApproval {
                path: "/other/simplecc.json".into(),
                ..good.clone()
            },
            ProjectApproval {
                sha256: "".into(),
                ..good.clone()
            },
        ] {
            assert!(load(Some(&changed)).is_err());
        }
        std::fs::write(&path, format!("{content} ")).unwrap();
        assert!(load(Some(&good)).is_err());
        // An explicit editor/user override intentionally keeps its old behavior.
        assert!(
            Config::load_workspace(root.to_str().unwrap(), path.to_str(), None, None, None).is_ok()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn approval_does_not_follow_project_symlink_retargeting() {
        let root = workspace("symlink");
        let a = root.join("a.json");
        let b = root.join("b.json");
        let selected = root.join("simplecc.json");
        let content = r#"{"languageServers":{}}"#;
        std::fs::write(&a, content).unwrap();
        std::fs::write(&b, content).unwrap();
        std::os::unix::fs::symlink(&a, &selected).unwrap();
        let approved = approval(&root, &a, content);
        assert!(
            Config::load_workspace(root.to_str().unwrap(), None, None, None, Some(&approved))
                .is_ok()
        );
        std::fs::remove_file(&selected).unwrap();
        std::os::unix::fs::symlink(&b, &selected).unwrap();
        assert!(
            Config::load_workspace(root.to_str().unwrap(), None, None, None, Some(&approved))
                .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remote_approval_is_bound_to_host_runtime_root_and_bytes() {
        let content = r#"{"languageServers":{}}"#;
        let remote = RemoteConfig {
            kind: "ssh".into(),
            target: "host-a".into(),
            root: "/project".into(),
            runtime: Some("".into()),
        };
        let a = ProjectApproval {
            root: "/project".into(),
            path: "".into(),
            sha256: format!("{:x}", Sha256::digest(content)),
            remote: Some(remote.clone()),
        };
        let load = |r: &RemoteConfig, c: &str, a| {
            Config::load_workspace("/project", None, Some(r), Some(c), a)
        };
        assert!(load(&remote, content, None).is_err());
        assert!(load(&remote, content, Some(&a)).is_ok());
        for changed in [
            RemoteConfig {
                target: "host-b".into(),
                ..remote.clone()
            },
            RemoteConfig {
                runtime: Some("other".into()),
                ..remote.clone()
            },
            RemoteConfig {
                root: "/elsewhere".into(),
                ..remote.clone()
            },
        ] {
            assert!(load(&changed, content, Some(&a)).is_err());
        }
        assert!(load(&remote, "{}", Some(&a)).is_err());
        assert!(Config::load_workspace("/project", None, None, Some(content), Some(&a)).is_err());
    }

    #[test]
    fn julia_defaults_are_merged_with_user_overrides() {
        let config = ServerConfig {
            command: "julia".to_string(),
            args: vec![],
            filetypes: vec!["julia".to_string()],
            root_patterns: vec!["Project.toml".to_string()],
            initialization_options: Some(json!({ "custom": true })),
            settings: Some(json!({
                "julia": { "lint": { "missingrefs": "symbols" } }
            })),
            priority: 0,
        };

        let init = config
            .effective_initialization_options("julia-lsp")
            .unwrap();
        assert_eq!(init["useFormatterConfigDefaults"], json!(true));
        assert_eq!(init["custom"], json!(true));

        let settings = config.effective_settings("julia-lsp").unwrap();
        assert_eq!(settings["julia"]["lint"]["missingrefs"], json!("symbols"));
        assert_eq!(settings["julia"]["completionmode"], json!("qualify"));
    }

    #[test]
    fn live_reload_reports_invalid_json_instead_of_using_defaults() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "simplecc-invalid-config-{}-{unique}.json",
            std::process::id()
        ));
        std::fs::write(&path, "{ invalid json").unwrap();

        let result = Config::load_selected("/tmp", path.to_str());
        let _ = std::fs::remove_file(path);
        assert!(result.is_err());
    }

    #[test]
    fn server_selection_is_stable_and_respects_priority() {
        let server = |priority| ServerConfig {
            command: "server".to_string(),
            args: vec![],
            filetypes: vec!["rust".to_string()],
            root_patterns: vec![],
            initialization_options: None,
            settings: None,
            priority,
        };
        let config = Config {
            language_servers: HashMap::from([
                ("zeta".to_string(), server(0)),
                ("alpha".to_string(), server(0)),
                ("preferred".to_string(), server(10)),
            ]),
        };

        let ordered: Vec<_> = config
            .servers_for_filetype("rust")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(ordered, ["preferred", "alpha", "zeta"]);
        assert_eq!(config.server_for_filetype("rust").unwrap().0, "preferred");
    }

    #[test]
    fn configuration_validation_rejects_empty_commands() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "simplecc-empty-command-{}-{unique}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{"languageServers":{"broken":{"command":" ","filetypes":["rust"]}}}"#,
        )
        .unwrap();

        let result = Config::load(&path);
        let _ = std::fs::remove_file(path);
        assert!(result.unwrap_err().to_string().contains("empty command"));
    }

    #[test]
    fn configuration_validation_rejects_empty_root_patterns() {
        let error = Config::parse(
            r#"{"languageServers":{"broken":{"command":"server","filetypes":["rust"],"rootPatterns":[" "]}}}"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("empty root pattern"));
    }
}
