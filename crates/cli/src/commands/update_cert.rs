use std::{error::Error, fs, path::Path};

use clap::Args;
use miot_rs::{MiHomeApiClient, MqttCertificateClient};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::credentials::{
    filename_component, load_oauth_credential, mqtt_certificate_directory, write_toml,
};

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
}

#[derive(Deserialize, Serialize)]
struct MqttIdentity {
    uid: String,
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
        let directory = mqtt_certificate_directory()?
            .join(filename_component(&account_id))
            .join(filename_component(&self.region));
        let identity = load_or_create_identity(&directory, &self.uid)?;
        info!(account = %account_id, uid = %self.uid, virtual_did = %identity.virtual_did, region = %self.region, "requesting MQTT client certificate");
        let api = MiHomeApiClient::new(&self.region, credential)?;
        let certificate = MqttCertificateClient::new(api)
            .request_client_certificate(&identity.uid, &identity.virtual_did)
            .await?;

        fs::create_dir_all(&directory)?;
        let key_path = directory.join("client.key");
        let cert_path = directory.join("client.cert");
        write_secret_file(&key_path, certificate.private_key_pem())?;
        write_secret_file(&cert_path, certificate.certificate_pem())?;
        println!(
            "MQTT client certificate updated for account {account_id}.\nVirtual DID: {}\nPrivate key: {}\nCertificate: {}",
            identity.virtual_did,
            key_path.display(),
            cert_path.display()
        );
        Ok(())
    }
}

fn load_or_create_identity(directory: &Path, uid: &str) -> Result<MqttIdentity, Box<dyn Error>> {
    if uid.trim().is_empty() {
        return Err("uid must not be empty".into());
    }
    let identity_path = directory.join("identity.toml");
    if identity_path.exists() {
        let identity: MqttIdentity = toml::from_str(&fs::read_to_string(identity_path)?)?;
        if identity.uid != uid {
            return Err("the saved MQTT identity belongs to a different uid".into());
        }
        return Ok(identity);
    }
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes)
        .map_err(|_| std::io::Error::other("secure random generation failed"))?;
    let identity = MqttIdentity {
        uid: uid.to_owned(),
        virtual_did: u64::from_be_bytes(bytes).to_string(),
    };
    write_toml(&identity_path, &identity)?;
    Ok(identity)
}

fn write_secret_file(path: &Path, contents: &str) -> Result<(), Box<dyn Error>> {
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
    use super::{load_or_create_identity, write_secret_file};

    #[test]
    fn writes_private_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("client.key");
        write_secret_file(&path, "private key").unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "private key");
    }

    #[test]
    fn creates_and_reuses_a_virtual_did() {
        let directory = tempfile::tempdir().unwrap();
        let first = load_or_create_identity(directory.path(), "12345").unwrap();
        let second = load_or_create_identity(directory.path(), "12345").unwrap();
        assert!(first.virtual_did.parse::<u64>().is_ok());
        assert_eq!(first.virtual_did, second.virtual_did);
        assert!(load_or_create_identity(directory.path(), "54321").is_err());
    }
}
