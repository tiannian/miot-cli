use std::{error::Error, fs, path::PathBuf};

use clap::Args;
use miot_rs::{MiHomeApiClient, MqttCertificateClient};
use tracing::info;

use crate::credentials::{filename_component, load_oauth_credential, mqtt_certificate_directory};

#[derive(Args)]
pub struct UpdateCertArguments {
    /// Account identifier passed to `miot auth login --account`.
    #[arg(long)]
    account: Option<String>,
    /// Xiaomi cloud region.
    #[arg(long, default_value = "cn")]
    region: String,
    /// Xiaomi account UID embedded in the client certificate subject.
    #[arg(long)]
    uid: String,
    /// Stable virtual DID used as the MQTT client ID.
    #[arg(long)]
    virtual_did: String,
}

impl UpdateCertArguments {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        if !self.region.eq_ignore_ascii_case("cn") {
            return Err(
                "central-gateway MQTT certificates are currently supported only in the cn region"
                    .into(),
            );
        }
        let (account_id, credential) = load_oauth_credential(self.account.as_deref())?;
        info!(account = %account_id, uid = %self.uid, region = %self.region, "requesting MQTT client certificate");
        let api = MiHomeApiClient::new(&self.region, credential)?;
        let certificate = MqttCertificateClient::new(api)
            .request_client_certificate(&self.uid, &self.virtual_did)
            .await?;

        let directory = mqtt_certificate_directory()?;
        fs::create_dir_all(&directory)?;
        let stem = format!(
            "{}_{}_{}",
            filename_component(&self.uid),
            filename_component(&self.region),
            filename_component(&self.virtual_did)
        );
        let key_path = directory.join(format!("{stem}.key"));
        let cert_path = directory.join(format!("{stem}.cert"));
        write_secret_file(&key_path, certificate.private_key_pem())?;
        write_secret_file(&cert_path, certificate.certificate_pem())?;
        println!(
            "MQTT client certificate updated for account {account_id}.\nPrivate key: {}\nCertificate: {}",
            key_path.display(),
            cert_path.display()
        );
        Ok(())
    }
}

fn write_secret_file(path: &PathBuf, contents: &str) -> Result<(), Box<dyn Error>> {
    fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::write_secret_file;

    #[test]
    fn writes_private_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.key");
        write_secret_file(&path, "private key").unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "private key");
    }
}
