use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use clap::Args;
use miot_rs::{MiHomeApiClient, MqttCertificateClient};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::credentials::{
    filename_component, load_oauth_credential, mqtt_certificate_directory, write_toml,
};

pub(crate) const MIHOME_MQTT_CA_CERTIFICATE: &str = concat!(
    "-----BEGIN CERTIFICATE-----\n",
    "MIIBazCCAQ+gAwIBAgIEA/UKYDAMBggqhkjOPQQDAgUAMCIxEzARBgNVBAoTCk1p\n",
    "amlhIFJvb3QxCzAJBgNVBAYTAkNOMCAXDTE2MTEyMzAxMzk0NVoYDzIwNjYxMTEx\n",
    "MDEzOTQ1WjAiMRMwEQYDVQQKEwpNaWppYSBSb290MQswCQYDVQQGEwJDTjBZMBMG\n",
    "ByqGSM49AgEGCCqGSM49AwEHA0IABL71iwLa4//4VBqgRI+6xE23xpovqPCxtv96\n",
    "2VHbZij61/Ag6jmi7oZ/3Xg/3C+whglcwoUEE6KALGJ9vccV9PmjLzAtMAwGA1Ud\n",
    "EwQFMAMBAf8wHQYDVR0OBBYEFJa3onw5sblmM6n40QmyAGDI5sURMAwGCCqGSM49\n",
    "BAMCBQADSAAwRQIgchciK9h6tZmfrP8Ka6KziQ4Lv3hKfrHtAZXMHPda4IYCIQCG\n",
    "az93ggFcbrG9u2wixjx1HKW4DUA5NXZG0wWQTpJTbQ==\n",
    "-----END CERTIFICATE-----\n",
    "-----BEGIN CERTIFICATE-----\n",
    "MIIBjzCCATWgAwIBAgIBATAKBggqhkjOPQQDAjAiMRMwEQYDVQQKEwpNaWppYSBS\n",
    "b290MQswCQYDVQQGEwJDTjAgFw0yMjA2MDkxNDE0MThaGA8yMDcyMDUyNzE0MTQx\n",
    "OFowLDELMAkGA1UEBhMCQ04xHTAbBgNVBAoMFE1JT1QgQ0VOVFJBTCBHQVRFV0FZ\n",
    "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEdYrzbnp/0x/cZLZnuEDXTFf8mhj4\n",
    "CVpZPwgj9e9Ve5r3K7zvu8Jjj7JF1JjQYvEC6yhp1SzBgglnK4L8xQzdiqNQME4w\n",
    "HQYDVR0OBBYEFCf9+YBU7pXDs6K6CAQPRhlGJ+cuMB8GA1UdIwQYMBaAFJa3onw5\n",
    "sblmM6n40QmyAGDI5sURMAwGA1UdEwQFMAMBAf8wCgYIKoZIzj0EAwIDSAAwRQIh\n",
    "AKUv+c8v98vypkGMTzMwckGjjVqTef8xodsy6PhcSCq+AiA/n9mDs62hAo5zXyJy\n",
    "Bs1s7mqXPf1XgieoxIvs1MqyiA==\n",
    "-----END CERTIFICATE-----\n",
);

#[derive(Args)]
pub struct UpdateCertArguments {
    /// Account identifier passed to `miot auth login --account`.
    #[arg(long)]
    account: Option<String>,
    /// Xiaomi cloud region.
    #[arg(long, default_value = "cn")]
    region: String,
    /// 首次创建 MQTT 身份时写入客户端证书 subject 的小米账号 UID。
    #[arg(long)]
    uid: Option<String>,
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
        let identity = load_or_create_identity(&directory, self.uid.as_deref())?;
        info!(account = %account_id, uid = %identity.uid, virtual_did = %identity.virtual_did, region = %self.region, "requesting MQTT client certificate");
        let api = MiHomeApiClient::new(&self.region, credential)?;
        let certificate = MqttCertificateClient::new(api)
            .request_client_certificate(&identity.uid, &identity.virtual_did)
            .await?;

        fs::create_dir_all(&directory)?;
        let ca_path = ensure_ca_certificate(&directory)?;
        let key_path = directory.join("client.key");
        let cert_path = directory.join("client.cert");
        write_secret_file(&key_path, certificate.private_key_pem())?;
        write_secret_file(&cert_path, certificate.certificate_pem())?;
        println!(
            "MQTT client certificate updated for account {account_id}.\nVirtual DID: {}\nCA certificate: {}\nPrivate key: {}\nCertificate: {}",
            identity.virtual_did,
            ca_path.display(),
            key_path.display(),
            cert_path.display()
        );
        Ok(())
    }
}

pub(crate) fn ensure_ca_certificate(directory: &Path) -> Result<PathBuf, Box<dyn Error>> {
    fs::create_dir_all(directory)?;
    let path = directory.join("mihome_ca.cert");
    if !path.exists() {
        fs::write(&path, MIHOME_MQTT_CA_CERTIFICATE)?;
    }
    Ok(path)
}

fn load_or_create_identity(
    directory: &Path,
    requested_uid: Option<&str>,
) -> Result<MqttIdentity, Box<dyn Error>> {
    let identity_path = directory.join("identity.toml");
    if identity_path.exists() {
        let identity: MqttIdentity = toml::from_str(&fs::read_to_string(identity_path)?)?;
        if requested_uid.is_some_and(|uid| identity.uid != uid) {
            return Err("the saved MQTT identity belongs to a different uid".into());
        }
        return Ok(identity);
    }
    let uid = requested_uid.ok_or("--uid is required when creating the first MQTT identity")?;
    if uid.trim().is_empty() {
        return Err("uid must not be empty".into());
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
    use super::{
        MIHOME_MQTT_CA_CERTIFICATE, ensure_ca_certificate, load_or_create_identity,
        write_secret_file,
    };

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
        assert!(load_or_create_identity(directory.path(), None).is_err());
        let first = load_or_create_identity(directory.path(), Some("12345")).unwrap();
        let second = load_or_create_identity(directory.path(), None).unwrap();
        assert!(first.virtual_did.parse::<u64>().is_ok());
        assert_eq!(first.virtual_did, second.virtual_did);
        assert!(load_or_create_identity(directory.path(), Some("54321")).is_err());
    }

    #[test]
    fn writes_the_fixed_mihome_ca_certificate() {
        let directory = tempfile::tempdir().unwrap();
        let path = ensure_ca_certificate(directory.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            MIHOME_MQTT_CA_CERTIFICATE
        );
    }
}
