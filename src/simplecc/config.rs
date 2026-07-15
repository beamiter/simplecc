use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    /// Load config from a path.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read configuration {}", path.display()))?;
        let config: Config = serde_json::from_str(&content)
            .with_context(|| format!("invalid JSON in configuration {}", path.display()))?;
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
        }
        Ok(())
    }

    /// Load the currently selected configuration without hiding parse or I/O
    /// errors. Used by live reload so an invalid edit never silently replaces
    /// the running settings with defaults.
    pub fn load_selected(project_root: &str, explicit_path: Option<&str>) -> Result<Self> {
        if let Some(path) = explicit_path.filter(|path| !path.is_empty()) {
            return Self::load(Path::new(path));
        }
        if let Some(path) = Self::find_path(project_root) {
            return Self::load(&path);
        }
        Ok(Self::default())
    }

    fn find_path(project_root: &str) -> Option<PathBuf> {
        let project_config = Path::new(project_root).join("simplecc.json");
        if project_config.exists() {
            return Some(project_config);
        }

        let dot_config = Path::new(project_root).join(".simplecc.json");
        if dot_config.exists() {
            return Some(dot_config);
        }

        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".config/simplecc/simplecc.json"))
            .filter(|path| path.exists())
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
}
