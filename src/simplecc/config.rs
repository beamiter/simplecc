use anyhow::Result;
use serde::Deserialize;
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
}

impl Config {
    /// Load config from a path.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Search for config: project root first, then ~/.config/simplecc/
    pub fn find_and_load(project_root: &str) -> Config {
        // 1. Project root
        let project_config = Path::new(project_root).join("simplecc.json");
        if project_config.exists() {
            if let Ok(c) = Self::load(&project_config) {
                eprintln!("[simplecc] loaded config from {}", project_config.display());
                return c;
            }
        }
        // .simplecc.json
        let dot_config = Path::new(project_root).join(".simplecc.json");
        if dot_config.exists() {
            if let Ok(c) = Self::load(&dot_config) {
                eprintln!("[simplecc] loaded config from {}", dot_config.display());
                return c;
            }
        }
        // 2. ~/.config/simplecc/simplecc.json
        if let Some(home) = std::env::var_os("HOME") {
            let global = PathBuf::from(home).join(".config/simplecc/simplecc.json");
            if global.exists() {
                if let Ok(c) = Self::load(&global) {
                    eprintln!("[simplecc] loaded config from {}", global.display());
                    return c;
                }
            }
        }
        // 3. Default with common servers
        eprintln!("[simplecc] no config found, using defaults");
        Config::default()
    }

    /// Find which server handles a given filetype.
    pub fn server_for_filetype(&self, filetype: &str) -> Option<(&str, &ServerConfig)> {
        for (name, cfg) in &self.language_servers {
            if cfg.filetypes.iter().any(|ft| ft == filetype) {
                return Some((name, cfg));
            }
        }
        None
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut servers = HashMap::new();

        servers.insert("rust-analyzer".to_string(), ServerConfig {
            command: "rust-analyzer".to_string(),
            args: vec![],
            filetypes: vec!["rust".to_string()],
            root_patterns: vec!["Cargo.toml".to_string()],
            initialization_options: None,
        });

        servers.insert("clangd".to_string(), ServerConfig {
            command: "clangd".to_string(),
            args: vec![],
            filetypes: vec!["c".to_string(), "cpp".to_string()],
            root_patterns: vec!["compile_commands.json".to_string(), ".clangd".to_string()],
            initialization_options: None,
        });

        servers.insert("pyright".to_string(), ServerConfig {
            command: "pyright-langserver".to_string(),
            args: vec!["--stdio".to_string()],
            filetypes: vec!["python".to_string()],
            root_patterns: vec!["pyproject.toml".to_string(), "setup.py".to_string()],
            initialization_options: None,
        });

        servers.insert("typescript-language-server".to_string(), ServerConfig {
            command: "typescript-language-server".to_string(),
            args: vec!["--stdio".to_string()],
            filetypes: vec![
                "typescript".to_string(), "javascript".to_string(),
                "typescriptreact".to_string(), "javascriptreact".to_string(),
            ],
            root_patterns: vec!["package.json".to_string(), "tsconfig.json".to_string()],
            initialization_options: None,
        });

        servers.insert("lua-language-server".to_string(), ServerConfig {
            command: "lua-language-server".to_string(),
            args: vec![],
            filetypes: vec!["lua".to_string()],
            root_patterns: vec![".luarc.json".to_string()],
            initialization_options: None,
        });

        servers.insert("gopls".to_string(), ServerConfig {
            command: "gopls".to_string(),
            args: vec![],
            filetypes: vec!["go".to_string(), "gomod".to_string()],
            root_patterns: vec!["go.mod".to_string()],
            initialization_options: None,
        });

        Config { language_servers: servers }
    }
}
