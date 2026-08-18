use super::*;
use anyhow::{Result, anyhow};

use tempfile::TempDir;

struct EnvRestore {
    saved: Vec<(String, Option<String>)>,
}

impl EnvRestore {
    fn new(keys: &[&str]) -> Self {
        let saved = keys
            .iter()
            .map(|key| (key.to_string(), std::env::var(key).ok()))
            .collect();
        Self { saved }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            if let Some(value) = value {
                crate::env::set_var(key, value);
            } else {
                crate::env::remove_var(key);
            }
        }
    }
}

async fn one_shot_http_server(
    response_body: String,
    status: u16,
) -> (
    u16,
    tokio::task::JoinHandle<(String, String, std::collections::HashMap<String, String>)>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock server");
    let port = listener.local_addr().expect("listener port").port();
    let handle = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let (stream, _) = listener.accept().await.expect("accept connection");
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .await
            .expect("read line");
        let parts: Vec<&str> = request_line.split_whitespace().collect();
        let method = parts.first().unwrap_or(&"").to_string();
        let path = parts.get(1).unwrap_or(&"").to_string();

        let mut headers = std::collections::HashMap::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.expect("read header");
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some((key, value)) = trimmed.split_once(':') {
                headers.insert(key.trim().to_lowercase(), value.trim().to_string());
            }
        }

        let response = format!(
            "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            status,
            response_body.len(),
            response_body
        );
        writer.write_all(response.as_bytes()).await.expect("write");

        (method, path, headers)
    });

    (port, handle)
}

#[test]
fn copilot_api_token_not_expired() {
    let future_ts = chrono::Utc::now().timestamp() + 3600;
    let token = CopilotApiToken {
        token: "test-token".to_string(),
        expires_at: future_ts,
    };
    assert!(!token.is_expired());
}

#[test]
fn copilot_api_token_expired() {
    let past_ts = chrono::Utc::now().timestamp() - 100;
    let token = CopilotApiToken {
        token: "test-token".to_string(),
        expires_at: past_ts,
    };
    assert!(token.is_expired());
}

#[test]
fn copilot_api_token_expiring_within_buffer() {
    let almost_ts = chrono::Utc::now().timestamp() + 30;
    let token = CopilotApiToken {
        token: "test-token".to_string(),
        expires_at: almost_ts,
    };
    assert!(token.is_expired());
}

#[test]
fn normalize_github_domain_accepts_github_and_ghe_domains() {
    assert_eq!(
        normalize_github_domain("github.com").as_deref(),
        Some("github.com")
    );
    assert_eq!(
        normalize_github_domain("https://company.ghe.com/").as_deref(),
        Some("company.ghe.com")
    );
    assert_eq!(
        normalize_github_domain("company.ghe.com").as_deref(),
        Some("company.ghe.com")
    );
    assert_eq!(
        normalize_github_domain("HTTP://COMPANY.GHE.COM").as_deref(),
        Some("company.ghe.com")
    );
}

#[test]
fn normalize_github_domain_rejects_non_github_hosts() {
    assert_eq!(normalize_github_domain("gitlab.com"), None);
    assert_eq!(normalize_github_domain("example.com"), None);
    assert_eq!(normalize_github_domain(""), None);
}

#[test]
fn copilot_base_url_prefers_account_endpoint() {
    assert_eq!(
        copilot_base_url(
            Some("https://api.business.githubcopilot.com"),
            "company.ghe.com",
        ),
        "https://api.business.githubcopilot.com"
    );
    assert_eq!(
        copilot_base_url(
            Some("https://api.business.githubcopilot.com/"),
            "github.com"
        ),
        "https://api.business.githubcopilot.com"
    );
}

#[test]
fn copilot_base_url_falls_back_for_public_and_enterprise() {
    assert_eq!(
        copilot_base_url(None, "github.com"),
        "https://api.githubcopilot.com"
    );
    assert_eq!(
        copilot_base_url(None, "company.ghe.com"),
        "https://copilot-api.company.ghe.com"
    );
}

#[test]
fn github_domain_url_builders() {
    assert_eq!(
        github_device_code_url("company.ghe.com"),
        "https://company.ghe.com/login/device/code"
    );
    assert_eq!(
        github_access_token_url("company.ghe.com"),
        "https://company.ghe.com/login/oauth/access_token"
    );
    assert_eq!(github_api_base("github.com"), "https://api.github.com");
    assert_eq!(
        github_api_base("company.ghe.com"),
        "https://api.company.ghe.com"
    );
    assert_eq!(
        copilot_token_url("github.com"),
        "https://api.github.com/copilot_internal/v2/token"
    );
    assert_eq!(
        copilot_token_url("company.ghe.com"),
        "https://api.company.ghe.com/copilot_internal/v2/token"
    );
    assert_eq!(
        copilot_user_url("github.com"),
        "https://api.github.com/copilot_internal/user"
    );
    assert_eq!(
        copilot_user_url("company.ghe.com"),
        "https://api.company.ghe.com/copilot_internal/user"
    );
}

#[test]
fn load_token_from_hosts_json() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let hosts_path = dir.path().join("hosts.json");
    let data = serde_json::json!({
        "github.com": {
            "oauth_token": "gho_testtoken123",
            "user": "testuser"
        }
    });
    std::fs::write(&hosts_path, serde_json::to_string(&data)?)?;

    let token = load_token_from_json(&hosts_path.to_path_buf())?;
    assert_eq!(token, "gho_testtoken123");
    Ok(())
}

#[test]
fn load_token_from_apps_json() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let apps_path = dir.path().join("apps.json");
    let data = serde_json::json!({
        "github.com": {
            "oauth_token": "ghu_vscodetoken456"
        }
    });
    std::fs::write(&apps_path, serde_json::to_string(&data)?)?;

    let token = load_token_from_json(&apps_path.to_path_buf())?;
    assert_eq!(token, "ghu_vscodetoken456");
    Ok(())
}

#[test]
fn load_token_missing_oauth_token_field() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("hosts.json");
    let data = serde_json::json!({
        "github.com": {
            "user": "testuser"
        }
    });
    std::fs::write(&path, serde_json::to_string(&data)?)?;

    let result = load_token_from_json(&path.to_path_buf());
    assert!(result.is_err());
    Ok(())
}

#[test]
fn load_token_empty_oauth_token() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("hosts.json");
    let data = serde_json::json!({
        "github.com": {
            "oauth_token": "",
            "user": "testuser"
        }
    });
    std::fs::write(&path, serde_json::to_string(&data)?)?;

    let result = load_token_from_json(&path.to_path_buf());
    assert!(result.is_err());
    Ok(())
}

#[test]
fn load_token_nonexistent_file() {
    let path = PathBuf::from("/tmp/nonexistent_auth_test_file.json");
    let result = load_token_from_json(&path);
    assert!(result.is_err());
}

#[test]
fn load_token_invalid_json() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("hosts.json");
    std::fs::write(&path, "not valid json{{{")?;

    let result = load_token_from_json(&path.to_path_buf());
    assert!(result.is_err());
    Ok(())
}

#[test]
fn load_token_from_copilot_config_json() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        serde_json::json!({
            "auth": {
                "token": "ghu_config_token"
            }
        })
        .to_string(),
    )?;

    let token = load_token_from_config_json(&path)?;
    assert_eq!(token, "ghu_config_token");
    Ok(())
}

#[test]
fn normalize_candidate_token_rejects_empty_and_unknown_values() {
    assert_eq!(
        normalize_candidate_token("gho_valid"),
        Some("gho_valid".to_string())
    );
    assert_eq!(
        normalize_candidate_token("ghu_valid"),
        Some("ghu_valid".to_string())
    );
    assert_eq!(
        normalize_candidate_token("github_pat_valid"),
        Some("github_pat_valid".to_string())
    );
    assert_eq!(normalize_candidate_token("ghp_classic"), None);
    assert_eq!(normalize_candidate_token("   "), None);
}

#[test]
fn gh_cli_fallback_requires_explicit_opt_in() {
    let key = "JCODE_COPILOT_ALLOW_GH_AUTH_TOKEN";
    let previous = std::env::var_os(key);

    crate::env::remove_var(key);
    assert!(!allow_gh_cli_fallback());

    crate::env::set_var(key, "0");
    assert!(!allow_gh_cli_fallback());

    crate::env::set_var(key, "1");
    assert!(allow_gh_cli_fallback());

    if let Some(previous) = previous {
        crate::env::set_var(key, previous);
    } else {
        crate::env::remove_var(key);
    }
}

#[test]
fn save_and_load_github_token() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let config_dir = dir.path().join("github-copilot");
    std::fs::create_dir_all(&config_dir)?;

    let hosts_path = config_dir.join("hosts.json");

    let mut config: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut entry = HashMap::new();
    entry.insert("user".to_string(), "testuser".to_string());
    entry.insert("oauth_token".to_string(), "gho_saved_token".to_string());
    config.insert("github.com".to_string(), entry);

    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(&hosts_path, json)?;

    let loaded = load_token_from_json(&hosts_path.to_path_buf())?;
    assert_eq!(loaded, "gho_saved_token");
    Ok(())
}

#[test]
fn save_github_token_creates_config_dir() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let config_dir = dir.path().join("github-copilot");
    let _env = EnvRestore::new(&["JCODE_HOME", "XDG_CONFIG_HOME"]);

    crate::env::remove_var("JCODE_HOME");
    crate::env::set_var(
        "XDG_CONFIG_HOME",
        dir.path()
            .to_str()
            .ok_or_else(|| anyhow!("temp dir path should be valid UTF-8"))?,
    );

    let result = save_github_token("gho_newtoken", "testuser");
    assert!(result.is_ok());

    let hosts_path = config_dir.join("hosts.json");
    assert!(hosts_path.exists());

    let loaded = load_token_from_json(&hosts_path)?;
    assert_eq!(loaded, "gho_newtoken");

    Ok(())
}

#[test]
fn save_github_token_for_host_writes_ghe_entry_and_endpoint() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let config_dir = dir.path().join("github-copilot");
    let _env = EnvRestore::new(&["JCODE_HOME", "XDG_CONFIG_HOME"]);

    crate::env::remove_var("JCODE_HOME");
    crate::env::set_var(
        "XDG_CONFIG_HOME",
        dir.path()
            .to_str()
            .ok_or_else(|| anyhow!("temp dir path should be valid UTF-8"))?,
    );

    save_github_token_for_host(
        "gho_ghe_token",
        "enterprise-user",
        "company.ghe.com",
        Some("https://api.business.githubcopilot.com/"),
    )?;

    let hosts_path = config_dir.join("hosts.json");
    let raw = std::fs::read_to_string(&hosts_path)?;
    assert!(raw.contains("company.ghe.com"), "hosts.json: {}", raw);
    assert!(
        raw.contains("api.business.githubcopilot.com"),
        "hosts.json: {}",
        raw
    );

    let (token, endpoint) =
        load_token_and_endpoint_from_json(&hosts_path, Some("company.ghe.com"))?;
    assert_eq!(token, "gho_ghe_token");
    assert_eq!(
        endpoint.as_deref(),
        Some("https://api.business.githubcopilot.com")
    );

    Ok(())
}

#[test]
fn save_github_token_for_host_rejects_invalid_host() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let _env = EnvRestore::new(&["JCODE_HOME", "XDG_CONFIG_HOME"]);
    crate::env::set_var("JCODE_HOME", dir.path());
    crate::env::remove_var("XDG_CONFIG_HOME");

    let err = save_github_token_for_host("gho_invalid", "user", "gitlab.com", None)
        .expect_err("invalid host must be rejected");
    assert!(err.to_string().contains("Invalid GitHub host"));
    assert!(!ExternalCopilotAuthSource::HostsJson.path().exists());
    Ok(())
}

#[test]
fn save_github_token_for_host_persists_effective_host() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let _env = EnvRestore::new(&["JCODE_HOME", "JCODE_COPILOT_GITHUB_HOST", "GH_HOST"]);

    crate::env::set_var("JCODE_HOME", dir.path());
    crate::env::remove_var("JCODE_COPILOT_GITHUB_HOST");
    crate::env::remove_var("GH_HOST");

    assert_eq!(copilot_github_host(), "github.com");

    save_github_token_for_host(
        "gho_ghe_token",
        "enterprise-user",
        "company.ghe.com",
        Some("https://api.business.githubcopilot.com"),
    )?;
    assert_eq!(copilot_github_host(), "company.ghe.com");

    crate::env::set_var("JCODE_COPILOT_GITHUB_HOST", "override.ghe.com");
    assert_eq!(copilot_github_host(), "override.ghe.com");
    crate::env::remove_var("JCODE_COPILOT_GITHUB_HOST");

    Ok(())
}

#[test]
fn load_github_token_for_host_reads_saved_ghe_token() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let _env = EnvRestore::new(&[
        "JCODE_HOME",
        "XDG_CONFIG_HOME",
        "COPILOT_GITHUB_TOKEN",
        "GH_TOKEN",
        "GITHUB_TOKEN",
    ]);

    crate::env::set_var("JCODE_HOME", dir.path());
    crate::env::remove_var("XDG_CONFIG_HOME");
    crate::env::remove_var("COPILOT_GITHUB_TOKEN");
    crate::env::remove_var("GH_TOKEN");
    crate::env::remove_var("GITHUB_TOKEN");

    save_github_token_for_host(
        "gho_ghe_token",
        "enterprise-user",
        "company.ghe.com",
        Some("https://api.business.githubcopilot.com"),
    )?;

    assert_eq!(
        load_github_token_for_host("company.ghe.com")?,
        "gho_ghe_token"
    );

    Ok(())
}

#[test]
fn copilot_api_endpoint_for_host_reads_saved_endpoint_and_env_override() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let _env = EnvRestore::new(&[
        "JCODE_HOME",
        "JCODE_COPILOT_API_ENDPOINT",
        "XDG_CONFIG_HOME",
    ]);

    crate::env::set_var("JCODE_HOME", dir.path());
    crate::env::remove_var("JCODE_COPILOT_API_ENDPOINT");
    crate::env::remove_var("XDG_CONFIG_HOME");

    save_github_token_for_host(
        "gho_ghe_token",
        "enterprise-user",
        "company.ghe.com",
        Some("https://api.business.githubcopilot.com/"),
    )?;

    assert_eq!(
        copilot_api_endpoint_for_host("company.ghe.com").as_deref(),
        Some("https://api.business.githubcopilot.com")
    );
    assert_eq!(copilot_api_endpoint_for_host("github.com"), None);

    crate::env::set_var(
        "JCODE_COPILOT_API_ENDPOINT",
        "https://override.example.com/",
    );
    assert_eq!(
        copilot_api_endpoint_for_host("company.ghe.com").as_deref(),
        Some("https://override.example.com")
    );
    crate::env::remove_var("JCODE_COPILOT_API_ENDPOINT");

    Ok(())
}

#[test]
fn legacy_copilot_config_dir_uses_jcode_home_external_dir() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let prev = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    let path = legacy_copilot_config_dir();
    assert_eq!(
        path,
        dir.path()
            .join("external")
            .join(".config")
            .join("github-copilot")
    );

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    Ok(())
}

#[test]
fn save_github_token_makes_future_loads_available() -> Result<()> {
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let prev_jcode_home = std::env::var_os("JCODE_HOME");
    let prev_xdg_config_home = std::env::var_os("XDG_CONFIG_HOME");

    crate::env::set_var("JCODE_HOME", dir.path());
    crate::env::remove_var("XDG_CONFIG_HOME");

    save_github_token("gho_persisted_token", "testuser")?;

    let hosts_path = ExternalCopilotAuthSource::HostsJson.path();
    assert!(
        crate::config::Config::external_auth_source_allowed_for_path(
            COPILOT_HOSTS_AUTH_SOURCE_ID,
            &hosts_path
        )
    );
    assert_eq!(load_github_token()?, "gho_persisted_token");

    if let Some(prev) = prev_jcode_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }

    if let Some(prev) = prev_xdg_config_home {
        crate::env::set_var("XDG_CONFIG_HOME", prev);
    } else {
        crate::env::remove_var("XDG_CONFIG_HOME");
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn load_token_from_json_does_not_change_external_permissions() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("hosts.json");
    std::fs::write(
        &path,
        r#"{"github.com":{"oauth_token":"gho_test","user":"tester"}}"#,
    )?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;

    let token = load_token_from_json(&path)?;
    assert_eq!(token, "gho_test");

    let dir_mode = std::fs::metadata(dir.path())?.permissions().mode() & 0o777;
    let file_mode = std::fs::metadata(&path)?.permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o755);
    assert_eq!(file_mode, 0o644);
    Ok(())
}

#[test]
fn choose_default_model_with_opus() {
    let models = vec![
        CopilotModelInfo {
            id: "claude-sonnet-4".to_string(),
            name: String::new(),
            vendor: String::new(),
            version: String::new(),
            model_picker_enabled: false,
            capabilities: Default::default(),
        },
        CopilotModelInfo {
            id: "claude-opus-4.6".to_string(),
            name: String::new(),
            vendor: String::new(),
            version: String::new(),
            model_picker_enabled: false,
            capabilities: Default::default(),
        },
    ];
    assert_eq!(choose_default_model(&models), "claude-opus-4.6");
}

#[test]
fn choose_default_model_without_opus() {
    let models = vec![CopilotModelInfo {
        id: "claude-sonnet-4.6".to_string(),
        name: String::new(),
        vendor: String::new(),
        version: String::new(),
        model_picker_enabled: false,
        capabilities: Default::default(),
    }];
    assert_eq!(choose_default_model(&models), "claude-sonnet-4.6");
}

#[test]
fn choose_default_model_with_sonnet_4_only() {
    let models = vec![CopilotModelInfo {
        id: "claude-sonnet-4".to_string(),
        name: String::new(),
        vendor: String::new(),
        version: String::new(),
        model_picker_enabled: false,
        capabilities: Default::default(),
    }];
    assert_eq!(choose_default_model(&models), "claude-sonnet-4");
}

#[test]
fn choose_default_model_empty_list() {
    let models: Vec<CopilotModelInfo> = vec![];
    assert_eq!(choose_default_model(&models), "claude-sonnet-4");
}

#[test]
fn copilot_account_type_display() {
    assert_eq!(CopilotAccountType::Individual.to_string(), "individual");
    assert_eq!(CopilotAccountType::Business.to_string(), "business");
    assert_eq!(CopilotAccountType::Enterprise.to_string(), "enterprise");
    assert_eq!(CopilotAccountType::Unknown.to_string(), "unknown");
}

#[test]
fn device_code_response_deserialize() -> Result<()> {
    let json = r#"{
            "device_code": "dc_1234",
            "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 5
        }"#;
    let resp: DeviceCodeResponse = serde_json::from_str(json)?;
    assert_eq!(resp.device_code, "dc_1234");
    assert_eq!(resp.user_code, "ABCD-1234");
    assert_eq!(resp.verification_uri, "https://github.com/login/device");
    assert_eq!(resp.expires_in, 900);
    assert_eq!(resp.interval, 5);
    Ok(())
}

#[test]
fn access_token_response_success() -> Result<()> {
    let json = r#"{
            "access_token": "gho_xxx123",
            "token_type": "bearer",
            "scope": "read:user"
        }"#;
    let resp: AccessTokenResponse = serde_json::from_str(json)?;
    assert_eq!(
        resp.access_token
            .ok_or_else(|| anyhow!("missing access token"))?,
        "gho_xxx123"
    );
    assert!(resp.error.is_none());
    Ok(())
}

#[test]
fn access_token_response_pending() -> Result<()> {
    let json = r#"{
            "error": "authorization_pending",
            "error_description": "The authorization request is still pending."
        }"#;
    let resp: AccessTokenResponse = serde_json::from_str(json)?;
    assert!(resp.access_token.is_none());
    assert_eq!(
        resp.error.ok_or_else(|| anyhow!("missing error"))?,
        "authorization_pending"
    );
    Ok(())
}

#[test]
fn access_token_response_expired() -> Result<()> {
    let json = r#"{
            "error": "expired_token",
            "error_description": "The device code has expired."
        }"#;
    let resp: AccessTokenResponse = serde_json::from_str(json)?;
    assert_eq!(
        resp.error.ok_or_else(|| anyhow!("missing error"))?,
        "expired_token"
    );
    Ok(())
}

#[test]
fn copilot_token_response_roundtrip() -> Result<()> {
    let resp = CopilotTokenResponse {
        token: "bearer_token_xxx".to_string(),
        expires_at: 1700000000,
    };
    let json = serde_json::to_string(&resp)?;
    let parsed: CopilotTokenResponse = serde_json::from_str(&json)?;
    assert_eq!(parsed.token, "bearer_token_xxx");
    assert_eq!(parsed.expires_at, 1700000000);
    Ok(())
}

#[test]
fn copilot_model_info_deserialize() -> Result<()> {
    let json = r#"{
            "id": "claude-sonnet-4",
            "name": "Claude Sonnet 4",
            "vendor": "anthropic",
            "version": "2025-01-01",
            "model_picker_enabled": true,
            "capabilities": {
                "type": "chat",
                "family": "claude-sonnet-4"
            }
        }"#;
    let model: CopilotModelInfo = serde_json::from_str(json)?;
    assert_eq!(model.id, "claude-sonnet-4");
    assert_eq!(model.vendor, "anthropic");
    assert!(model.model_picker_enabled);
    Ok(())
}

#[test]
fn copilot_model_info_minimal() -> Result<()> {
    let json = r#"{"id": "gpt-4o"}"#;
    let model: CopilotModelInfo = serde_json::from_str(json)?;
    assert_eq!(model.id, "gpt-4o");
    assert_eq!(model.name, "");
    assert!(!model.model_picker_enabled);
    Ok(())
}

#[tokio::test]
async fn fetch_available_models_uses_provided_api_base() -> Result<()> {
    let response_body = serde_json::json!({
        "data": [{
            "id": "claude-sonnet-4",
            "name": "Claude Sonnet 4",
            "vendor": "anthropic",
            "version": "1",
            "model_picker_enabled": true
        }]
    })
    .to_string();
    let (port, handle) = one_shot_http_server(response_body, 200).await;
    let api_base = format!("http://127.0.0.1:{port}");

    let models = fetch_available_models(&reqwest::Client::new(), "bearer-123", &api_base).await?;
    let (method, path, headers) = handle.await.map_err(|error| anyhow!(error))?;

    assert_eq!(method, "GET");
    assert_eq!(path, "/models");
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some("Bearer bearer-123")
    );
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "claude-sonnet-4");
    Ok(())
}

#[tokio::test]
async fn exchange_github_token_with_url_hits_token_endpoint() -> Result<()> {
    let expires = chrono::Utc::now().timestamp() + 3600;
    let response_body = serde_json::json!({
        "token": "bearer-123",
        "expires_at": expires
    })
    .to_string();
    let (port, handle) = one_shot_http_server(response_body, 200).await;
    let url = format!("http://127.0.0.1:{port}/copilot_internal/v2/token");

    let token = exchange_github_token_with_url(&reqwest::Client::new(), "gho-123", &url).await?;
    let (method, path, headers) = handle.await.map_err(|error| anyhow!(error))?;

    assert_eq!(method, "GET");
    assert_eq!(path, "/copilot_internal/v2/token");
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some("Token gho-123")
    );
    assert_eq!(token.token, "bearer-123");
    assert_eq!(token.expires_at, expires);
    Ok(())
}

#[tokio::test]
async fn poll_for_access_token_with_url_posts_and_returns_token() -> Result<()> {
    let response_body = serde_json::json!({ "access_token": "gho-polled" }).to_string();
    let (port, handle) = one_shot_http_server(response_body, 200).await;
    let url = format!("http://127.0.0.1:{port}/login/oauth/access_token");

    let token =
        poll_for_access_token_with_url(&reqwest::Client::new(), "device-123", 0, &url).await?;
    let (method, path, _headers) = handle.await.map_err(|error| anyhow!(error))?;

    assert_eq!(method, "POST");
    assert_eq!(path, "/login/oauth/access_token");
    assert_eq!(token, "gho-polled");
    Ok(())
}

#[tokio::test]
async fn fetch_github_username_with_url_hits_user_endpoint() -> Result<()> {
    let response_body = serde_json::json!({ "login": "octocat" }).to_string();
    let (port, handle) = one_shot_http_server(response_body, 200).await;
    let url = format!("http://127.0.0.1:{port}/user");

    let username = fetch_github_username_with_url(&reqwest::Client::new(), "gho-123", &url).await?;
    let (method, path, headers) = handle.await.map_err(|error| anyhow!(error))?;

    assert_eq!(method, "GET");
    assert_eq!(path, "/user");
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some("Bearer gho-123")
    );
    assert_eq!(username, "octocat");
    Ok(())
}

#[tokio::test]
async fn fetch_copilot_api_endpoint_with_url_parses_endpoints_api() -> Result<()> {
    let response_body = serde_json::json!({
        "endpoints": { "api": "https://api.business.githubcopilot.com/" }
    })
    .to_string();
    let (port, handle) = one_shot_http_server(response_body, 200).await;
    let url = format!("http://127.0.0.1:{port}/copilot_internal/user");

    let endpoint =
        fetch_copilot_api_endpoint_with_url(&reqwest::Client::new(), "gho-123", &url).await?;
    let (method, path, headers) = handle.await.map_err(|error| anyhow!(error))?;

    assert_eq!(method, "GET");
    assert_eq!(path, "/copilot_internal/user");
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some("Bearer gho-123")
    );
    assert_eq!(
        headers.get("x-github-api-version").map(String::as_str),
        Some(COPILOT_USER_API_VERSION)
    );
    assert_eq!(
        endpoint.as_deref(),
        Some("https://api.business.githubcopilot.com")
    );
    Ok(())
}

#[tokio::test]
async fn fetch_copilot_api_endpoint_with_url_returns_none_on_error() -> Result<()> {
    let response_body = serde_json::json!({ "message": "forbidden" }).to_string();
    let (port, handle) = one_shot_http_server(response_body, 403).await;
    let url = format!("http://127.0.0.1:{port}/copilot_internal/user");

    let endpoint =
        fetch_copilot_api_endpoint_with_url(&reqwest::Client::new(), "gho-123", &url).await?;
    handle.await.map_err(|error| anyhow!(error))?;

    assert_eq!(endpoint, None);
    Ok(())
}

#[test]
fn load_token_multiple_hosts() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("hosts.json");
    let data = serde_json::json!({
        "api.github.com": {
            "oauth_token": "gho_api",
            "user": "user1"
        },
        "github.com": {
            "oauth_token": "gho_primary",
            "user": "user2"
        },
        "https://github.com/extra/path": {
            "oauth_token": "gho_path",
            "user": "user3"
        }
    });
    std::fs::write(&path, serde_json::to_string(&data)?)?;

    let token = load_token_from_json(&path.to_path_buf())?;
    assert_eq!(token, "gho_primary");
    Ok(())
}

#[test]
fn load_token_and_endpoint_prefers_requested_host() -> Result<()> {
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let path = dir.path().join("hosts.json");
    let data = serde_json::json!({
        "github.com": {
            "oauth_token": "gho_public",
            "user": "public"
        },
        "company.ghe.com": {
            "oauth_token": "gho_enterprise",
            "user": "enterprise",
            "api_endpoint": "https://api.business.githubcopilot.com/"
        }
    });
    std::fs::write(&path, serde_json::to_string(&data)?)?;

    let (token, endpoint) =
        load_token_and_endpoint_from_json(&path.to_path_buf(), Some("company.ghe.com"))?;
    assert_eq!(token, "gho_enterprise");
    assert_eq!(
        endpoint.as_deref(),
        Some("https://api.business.githubcopilot.com")
    );
    Ok(())
}

#[test]
fn normalize_github_host_key_accepts_common_forms() {
    assert_eq!(
        normalize_github_host_key("https://github.com/login"),
        Some("github.com".to_string())
    );
    assert_eq!(
        normalize_github_host_key("api.github.com"),
        Some("api.github.com".to_string())
    );
    assert_eq!(
        normalize_github_host_key("sub.github.com/path"),
        Some("sub.github.com".to_string())
    );
    assert_eq!(
        normalize_github_host_key("company.ghe.com"),
        Some("company.ghe.com".to_string())
    );
}

#[test]
fn normalize_github_host_key_rejects_non_github_hosts() {
    assert_eq!(normalize_github_host_key("gitlab.com"), None);
    assert_eq!(normalize_github_host_key(""), None);
}

/// Regression test for issue #641: GitHub's `apps.json` keys carry a client-id
/// suffix after a colon. Those entries hold valid tokens and must normalize,
/// otherwise the credential is silently dropped and jcode falls through to a
/// possibly stale token from a lower-priority source.
#[test]
fn normalize_github_host_key_accepts_apps_json_client_id_suffix() {
    assert_eq!(
        normalize_github_host_key("github.com:Iv1.b507a08c87ecfe98"),
        Some("github.com".to_string())
    );
    assert_eq!(
        normalize_github_host_key("api.github.com:Iv1.b507a08c87ecfe98"),
        Some("api.github.com".to_string())
    );
    // A colon suffix does not make a non-GitHub host acceptable.
    assert_eq!(normalize_github_host_key("gitlab.com:Iv1.abc"), None);
}

#[test]
fn live_verify_failure_record_blocks_auto_use() -> Result<()> {
    // Lock the test env and isolate JCODE_HOME so the validation record we write
    // does not touch the developer's real auth-validation.json.
    let _guard = crate::storage::lock_test_env();
    let dir = TempDir::new().map_err(|e| anyhow!(e))?;
    let prev_jcode_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", dir.path());

    // Ensure no env token is present, otherwise the gate short-circuits to
    // "allowed" regardless of the saved record.
    let prev_env: Vec<(&str, Option<std::ffi::OsString>)> =
        ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"]
            .into_iter()
            .map(|k| (k, std::env::var_os(k)))
            .collect();
    for (k, _) in &prev_env {
        crate::env::remove_var(k);
    }

    // A banned/ineligible account produces exactly this failure summary shape
    // from `exchange_github_token`; `verify_copilot_credentials_live` persists
    // it verbatim. The auto-use gate must treat it as blocking.
    let record = crate::auth::validation::ProviderValidationRecord {
        checked_at_ms: chrono::Utc::now().timestamp_millis(),
        success: false,
        provider_smoke_ok: Some(false),
        tool_smoke_ok: None,
        summary: "Copilot token exchange failed (HTTP 403): access blocked".to_string(),
    };
    crate::auth::validation::save("copilot", record)?;

    assert!(
        validation_failure_blocks_auto_use(),
        "a recent banned-account exchange failure must block auto-use"
    );

    // A successful exchange record must NOT block auto-use.
    let ok_record = crate::auth::validation::ProviderValidationRecord {
        checked_at_ms: chrono::Utc::now().timestamp_millis(),
        success: true,
        provider_smoke_ok: Some(true),
        tool_smoke_ok: None,
        summary: "copilot token exchange ok".to_string(),
    };
    crate::auth::validation::save("copilot", ok_record)?;
    assert!(
        !validation_failure_blocks_auto_use(),
        "a successful exchange must not block auto-use"
    );

    for (k, prev) in prev_env {
        if let Some(prev) = prev {
            crate::env::set_var(k, prev);
        }
    }
    if let Some(prev) = prev_jcode_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    Ok(())
}

#[test]
fn token_exchange_backoff_is_exponential_and_capped() {
    assert_eq!(super::token_exchange_backoff_ms(1), 500);
    assert_eq!(super::token_exchange_backoff_ms(2), 1_000);
    assert_eq!(super::token_exchange_backoff_ms(3), 2_000);
    assert_eq!(super::token_exchange_backoff_ms(4), 4_000);
    assert_eq!(super::token_exchange_backoff_ms(5), 4_000);
    assert_eq!(super::token_exchange_backoff_ms(100), 4_000);
}

#[test]
fn token_exchange_retries_only_5xx() {
    assert!(super::token_exchange_retryable_status(500));
    assert!(super::token_exchange_retryable_status(502));
    assert!(super::token_exchange_retryable_status(503));
    assert!(super::token_exchange_retryable_status(599));
    assert!(!super::token_exchange_retryable_status(400));
    assert!(!super::token_exchange_retryable_status(401));
    assert!(!super::token_exchange_retryable_status(403));
    assert!(!super::token_exchange_retryable_status(404));
    assert!(!super::token_exchange_retryable_status(429));
    assert!(!super::token_exchange_retryable_status(200));
}
