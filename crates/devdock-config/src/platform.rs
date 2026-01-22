//! Platform-specific configuration and authentication.

use crate::ini_parser::ParsedIniConfig;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::process::Command;

/// Platform-specific configuration provider
#[async_trait]
pub trait Platform: Send + Sync {
    /// Authenticate with the platform
    async fn authenticate(&self) -> Result<()>;

    /// Update devdock.ini configuration
    fn update_devdock_config(&self, config: &mut IniConfig);

    /// Update temp_profile.yaml configuration
    fn update_temp_profile(&self, profile: &mut TempProfile);

    /// Get platform name
    fn name(&self) -> &str;

    /// Get the primary zone for this platform
    fn zone(&self) -> &str;

    /// Get the primary region for this platform
    fn region(&self) -> &str;
}

/// Simple INI configuration builder
#[derive(Debug, Default)]
pub struct IniConfig {
    sections: HashMap<String, HashMap<String, String>>,
}

impl IniConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, section: &str, key: &str, value: impl Into<String>) {
        self.sections.entry(section.to_string()).or_default().insert(key.to_string(), value.into());
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&String> {
        self.sections.get(section)?.get(key)
    }

    pub fn as_string(&self) -> String {
        let mut result = String::new();

        for (section, keys) in &self.sections {
            result.push_str(&format!("[{section}]\n"));
            for (key, value) in keys {
                result.push_str(&format!("{key} = {value}\n"));
            }
            result.push('\n');
        }

        result
    }
}

/// Temporary profile configuration
#[derive(Debug, Default)]
pub struct TempProfile {
    pub project: String,
    pub platform: String,
    pub zone: String,
    pub region: String,
    pub project_number: String,
    pub architecture: String,
    pub devdock_root: String,
    pub dns_kubectl: String,
    pub current_profile: String,
    pub vault_key: Option<String>,
}

/// DNS kubectl enabled projects
const DNS_KUBECTL_ENABLED_PROJECTS: &[&str] = &["dev-sandbox-334901", "salessavvy-test"];

/// Google Cloud Platform implementation
#[allow(dead_code)]
pub struct GooglePlatform {
    project: String,
    zone: String,
    region: String,
    project_number: String,
    dns_kubectl_enabled: bool,
}

impl GooglePlatform {
    /// Create a new GooglePlatform with zone/region from dynamic config
    pub async fn new(project: String, dynamic_config: Option<&ParsedIniConfig>) -> Result<Self> {
        Self::new_with_multi_project(project, dynamic_config, false).await
    }

    /// Create a new GooglePlatform with multi_project flag
    /// Note: multi_project parameter is for API compatibility but not stored/used
    pub async fn new_with_multi_project(
        project: String,
        dynamic_config: Option<&ParsedIniConfig>,
        _multi_project: bool,
    ) -> Result<Self> {
        // Get project number
        let project_number = Self::get_project_number(&project).await?;

        // Get zone from dynamic config first, fallback to us-central1-c (not us-central1-a!)
        // Python reads from config.get('google', 'primaryZone')
        let zone = dynamic_config
            .and_then(|c| c.get("google", "primaryZone"))
            .cloned()
            .unwrap_or_else(|| "us-central1-c".to_string());

        // Derive region from zone (remove last 2 chars: us-central1-c -> us-central1)
        let region = if zone.len() > 2 {
            zone[..zone.len() - 2].to_string()
        } else {
            "us-central1".to_string()
        };

        // Check if DNS kubectl should be enabled
        let dns_kubectl_enabled = Self::should_use_dns_kubectl(&project);

        Ok(Self { project, zone, region, project_number, dns_kubectl_enabled })
    }

    /// Check if DNS-based kubectl should be enabled
    fn should_use_dns_kubectl(project: &str) -> bool {
        let env_flag = std::env::var("ENABLE_DNS_KUBECTL")
            .map(|v| matches!(v.to_lowercase().as_str(), "true" | "1" | "yes"))
            .unwrap_or(false);
        let project_supported = DNS_KUBECTL_ENABLED_PROJECTS.contains(&project);
        env_flag && project_supported
    }

    async fn get_project_number(project: &str) -> Result<String> {
        let output = Command::new("gcloud")
            .args(["projects", "describe", project, "--format", "value(projectNumber)"])
            .output()
            .await
            .context("Failed to execute gcloud command - is gcloud installed and in PATH?")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("\n❌ Failed to get GCP project number for '{project}'");
            eprintln!("Error: {stderr}");
            eprintln!("\nPossible causes:");
            eprintln!("  1. Not authenticated with gcloud");
            eprintln!("     → Run: gcloud auth login");
            eprintln!("  2. No access to project '{project}'");
            eprintln!("     → Verify project exists and you have access");
            eprintln!("  3. Wrong active gcloud account");
            eprintln!("     → Run: gcloud auth list");
            eprintln!("  4. Wrong gcloud project configured");
            eprintln!("     → Run: gcloud config set project {project}");
            anyhow::bail!("gcloud projects describe failed for '{}': {}", project, stderr.trim());
        }

        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    }
}

#[async_trait]
impl Platform for GooglePlatform {
    async fn authenticate(&self) -> Result<()> {
        // Switch project
        let step_start = std::time::Instant::now();
        Command::new("gcloud")
            .args(["config", "set", "project", &self.project])
            .status()
            .await
            .context("Failed to switch gcloud project")?;
        println!("      └─ gcloud set project: {:.2}s", step_start.elapsed().as_secs_f64());

        // Get cluster credentials
        let cluster = if self.project.contains("apps") || self.project.contains("canary") {
            "apps"
        } else {
            "glean-cluster"
        };

        if self.project != "scio-engineering" {
            let mut args =
                vec!["container", "clusters", "get-credentials", cluster, "--zone", &self.zone];

            // Add DNS endpoint flag if enabled
            if self.dns_kubectl_enabled {
                args.push("--dns-endpoint");
            }

            let step_start = std::time::Instant::now();
            let status = Command::new("gcloud").args(&args).status().await?;
            println!(
                "      └─ gcloud get-credentials ({}): {:.2}s",
                cluster,
                step_start.elapsed().as_secs_f64()
            );

            if !status.success() {
                eprintln!(
                    "Warning: Failed to get cluster credentials for {} in zone {}",
                    cluster, self.zone
                );
            }
        }

        Ok(())
    }

    fn update_devdock_config(&self, config: &mut IniConfig) {
        config.set("google", "storage.configbucket", format!("config-{}", self.project));
        config.set("google", "apps.useCentralApp", "false");
        config.set("google", "docbuilderRequest.enabled", "true");
    }

    fn update_temp_profile(&self, profile: &mut TempProfile) {
        profile.project = self.project.clone();
        profile.platform = "google".to_string();
        profile.zone = self.zone.clone();
        profile.region = self.region.clone();
        profile.project_number = self.project_number.clone();
        profile.dns_kubectl =
            if self.dns_kubectl_enabled { "enabled" } else { "disabled" }.to_string();
        profile.vault_key = None; // Google uses ADC, not vault key
    }

    fn name(&self) -> &'static str {
        "google"
    }

    fn zone(&self) -> &str {
        &self.zone
    }

    fn region(&self) -> &str {
        &self.region
    }
}

/// AWS Platform implementation
pub struct AWSPlatform {
    project: String,
    region: String,
    zone: String,
}

impl AWSPlatform {
    pub async fn new(project: String, dynamic_config: Option<&ParsedIniConfig>) -> Result<Self> {
        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| "us-east-1".to_string());

        // Get AWS account ID from dynamic config or fetch from AWS
        let aws_account_id = if let Some(config) = dynamic_config {
            // Try to get from dynamic config first
            config
                .get("aws", "accountId")
                .or_else(|| config.get("manifest", "project_number"))
                .map(|s| s.to_string())
                .unwrap_or_else(|| project.clone())
        } else {
            // Fallback: try to get from AWS CLI
            match Command::new("aws")
                .args(["sts", "get-caller-identity", "--query", "Account", "--output", "text"])
                .output()
                .await
            {
                Ok(output) if output.status.success() => {
                    String::from_utf8_lossy(&output.stdout).trim().to_string()
                }
                _ => project.clone(), // Fallback to project if we can't get account ID
            }
        };

        // Download RDS SSL certificate (matches Python behavior)
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let cert_path = format!("{}/.devdock/global-bundle.pem", home);
        if !std::path::Path::new(&cert_path).exists() {
            let _ = Command::new("curl")
                .args([
                    "-L",
                    "-o",
                    &cert_path,
                    "https://truststore.pki.rds.amazonaws.com/global/global-bundle.pem",
                ])
                .status()
                .await;
        }

        // Create stackdriver-exporter-key.json for AWS->GCP monitoring
        // This enables AWS credentials to authenticate to GCP services via Workload Identity Federation
        // Matches Python behavior in platforms.py:AWSPlatform.__init__

        // Get gcpConnector info from dynamic_config (now properly fetched from S3)
        if let Some(config) = dynamic_config {
            if let (Some(gcp_project), Some(gcp_project_number)) = (
                config.get("aws", "gcpConnectorProject"),
                config.get("aws", "gcpConnectorProjectNumber"),
            ) {
                let credentials_json = format!(
                    r#"{{
  "type": "external_account",
  "project_id": "{}",
  "audience": "//iam.googleapis.com/projects/{}/locations/global/workloadIdentityPools/aws-access-pool/providers/aws-provider",
  "subject_token_type": "urn:ietf:params:aws:token-type:aws4_request",
  "service_account_impersonation_url": "https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/stackdriver-exporter@{}.iam.gserviceaccount.com:generateAccessToken",
  "token_url": "https://sts.googleapis.com/v1/token",
  "credential_source": {{
    "environment_id": "aws1",
    "regional_cred_verification_url": "https://sts.{}.amazonaws.com?Action=GetCallerIdentity&Version=2011-06-15"
  }}
}}"#,
                    gcp_project, gcp_project_number, gcp_project, region
                );

                // Write to /tmp/stackdriver-exporter-key.json
                match tokio::fs::write("/tmp/stackdriver-exporter-key.json", &credentials_json)
                    .await
                {
                    Ok(_) => {
                        println!("✅ Created stackdriver-exporter-key.json for GCP monitoring");
                        println!(
                            "   GCP Connector: {} (project #{})",
                            gcp_project, gcp_project_number
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "⚠️  Warning: Failed to write stackdriver-exporter-key.json: {}",
                            e
                        );
                    }
                }
            } else {
                eprintln!("⚠️  Warning: gcpConnectorProject not found in dynamic config");
                eprintln!("   GCP monitoring (Stackdriver) will not be available");
                eprintln!("   This is expected for local dev environments");
            }
        } else {
            eprintln!("⚠️  Warning: No dynamic config loaded for AWS platform");
            eprintln!("   Stackdriver monitoring key cannot be generated");
        }

        Ok(Self {
            project: aws_account_id,
            zone: String::new(), // AWS doesn't use zones the same way
            region,
        })
    }
}

#[async_trait]
impl Platform for AWSPlatform {
    async fn authenticate(&self) -> Result<()> {
        // Update kubeconfig for EKS
        let status = Command::new("aws")
            .args(["eks", "update-kubeconfig", "--name", "glean-cluster", "--region", &self.region])
            .status()
            .await?;

        if !status.success() {
            eprintln!("Warning: Failed to update EKS kubeconfig");
        }

        Ok(())
    }

    fn update_devdock_config(&self, config: &mut IniConfig) {
        config.set("aws", "region", &self.region);
        config.set("aws", "storage.configbucket", format!("config-{}", self.project));
        config.set("aws", "apps.useCentralApp", "false");
        config.set("aws", "docbuilderRequest.enabled", "true");
    }

    fn update_temp_profile(&self, profile: &mut TempProfile) {
        profile.project = self.project.clone();
        profile.platform = "aws".to_string();
        profile.region = self.region.clone();
        profile.dns_kubectl = "disabled".to_string();
        // Set vault_key for GCP monitoring (matches Python platforms.py:190)
        profile.vault_key = Some("/tmp/stackdriver-exporter-key.json".to_string());
    }

    fn name(&self) -> &'static str {
        "aws"
    }

    fn zone(&self) -> &str {
        &self.zone
    }

    fn region(&self) -> &str {
        &self.region
    }
}

/// Azure Platform implementation
pub struct AzurePlatform {
    project: String,
    subscription_id: String,
    region: String,
}

impl AzurePlatform {
    pub async fn new(project: String, _dynamic_config: Option<&ParsedIniConfig>) -> Result<Self> {
        // Get Azure subscription ID
        let output = Command::new("az")
            .args(["account", "show", "--query", "id", "--output", "tsv"])
            .output()
            .await?;

        let subscription_id = if output.status.success() {
            String::from_utf8(output.stdout)?.trim().to_string()
        } else {
            String::new()
        };

        Ok(Self { project, subscription_id, region: "uswest2".to_string() })
    }
}

#[async_trait]
impl Platform for AzurePlatform {
    async fn authenticate(&self) -> Result<()> {
        // Get AKS credentials
        let status = Command::new("az")
            .args([
                "aks",
                "get-credentials",
                "--resource-group",
                "glean-rg",
                "--name",
                "glean-cluster",
            ])
            .status()
            .await?;

        if !status.success() {
            eprintln!("Warning: Failed to get AKS credentials");
        }

        Ok(())
    }

    fn update_devdock_config(&self, config: &mut IniConfig) {
        config.set("azure", "subscription_id", &self.subscription_id);
    }

    fn update_temp_profile(&self, profile: &mut TempProfile) {
        profile.project = self.project.clone();
        profile.platform = "azure".to_string();
        profile.region = self.region.clone();
        profile.dns_kubectl = "disabled".to_string();
        profile.vault_key = None; // Azure uses az cli auth, not vault key
    }

    fn name(&self) -> &'static str {
        "azure"
    }

    fn zone(&self) -> &'static str {
        ""
    }

    fn region(&self) -> &str {
        &self.region
    }
}

/// Create platform from profile with optional dynamic config for zone/region
pub async fn create_platform(
    platform_name: &str,
    project: String,
    dynamic_config: Option<&ParsedIniConfig>,
) -> Result<Box<dyn Platform>> {
    create_platform_with_multi_project(platform_name, project, dynamic_config, false).await
}

/// Create a mock platform for benchmarking/testing (no external dependencies)
pub fn create_mock_platform(project: String) -> Box<dyn Platform> {
    Box::new(MockPlatform { project })
}

/// Mock platform for benchmarking (no external API calls)
struct MockPlatform {
    project: String,
}

#[async_trait]
impl Platform for MockPlatform {
    async fn authenticate(&self) -> Result<()> {
        // No-op for mock
        Ok(())
    }

    fn update_devdock_config(&self, config: &mut IniConfig) {
        config.set("manifest", "project", &self.project);
        config.set("manifest", "zone", "us-central1-a");
        config.set("manifest", "region", "us-central1");
        config.set("manifest", "project_number", "000000000000");
    }

    fn update_temp_profile(&self, profile: &mut TempProfile) {
        profile.project = self.project.clone();
        profile.zone = "us-central1-a".to_string();
        profile.region = "us-central1".to_string();
        profile.vault_key = None; // Mock platform doesn't need vault key
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn zone(&self) -> &str {
        "us-central1-a"
    }

    fn region(&self) -> &str {
        "us-central1"
    }
}

/// Create platform with multi_project flag
pub async fn create_platform_with_multi_project(
    platform_name: &str,
    project: String,
    dynamic_config: Option<&ParsedIniConfig>,
    multi_project: bool,
) -> Result<Box<dyn Platform>> {
    match platform_name.to_lowercase().as_str() {
        "google" | "gcp" => Ok(Box::new(
            GooglePlatform::new_with_multi_project(project, dynamic_config, multi_project).await?,
        )),
        "aws" => {
            // AWS doesn't support multi_project yet
            Ok(Box::new(AWSPlatform::new(project, dynamic_config).await?))
        }
        "azure" => {
            // Azure doesn't support multi_project yet
            Ok(Box::new(AzurePlatform::new(project, dynamic_config).await?))
        }
        _ => anyhow::bail!("Unsupported platform: {platform_name}"),
    }
}
