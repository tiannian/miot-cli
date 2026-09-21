use std::{
    error::Error,
    fs,
    io::{self, Write},
    time::UNIX_EPOCH,
};

use clap::{ArgGroup, Args, Subcommand};
use miot_rs::{
    CloudLoginClient, CloudLoginOutcome, CloudLoginRequest, OAuthAuthorizationRequest,
    OAuthCredential, OAuthLoginClient, QrLoginClient, VerificationProof,
};
use url::Url;

use crate::credentials::{StoredCredential, credential_path, write_toml};

const HA_OAUTH_CLIENT_ID: &str = "2882303761520251711";
const HA_OAUTH_REDIRECT_URL: &str = "http://homeassistant.local:8123";

#[derive(Args)]
pub struct AuthCommand {
    #[command(subcommand)]
    command: AuthSubcommand,
}

#[derive(Subcommand)]
enum AuthSubcommand {
    Login(LoginArguments),
}

impl AuthCommand {
    pub async fn run(self) -> Result<(), Box<dyn Error>> {
        match self.command {
            AuthSubcommand::Login(arguments) => login(arguments).await,
        }
    }
}

#[derive(Args)]
#[command(group(ArgGroup::new("login_mode").required(true).args(["userpass", "userpass_stdin", "oauth", "qr"])))]
struct LoginArguments {
    /// Sign in with USERNAME:PASSWORD.
    #[arg(long)]
    userpass: Option<String>,
    /// Read username and password from separate standard-input lines.
    #[arg(long)]
    userpass_stdin: bool,
    /// Sign in through the OAuth authorization-code flow.
    #[arg(long)]
    oauth: bool,
    /// Sign in by scanning a Xiaomi Home QR code.
    #[arg(long)]
    qr: bool,
    /// Xiaomi cloud region.
    #[arg(long, default_value = "cn")]
    region: String,
    /// Stable local device identifier.
    #[arg(long)]
    device_id: Option<String>,
    /// Xiaomi service identifier for user-password login.
    #[arg(long, default_value = "xiaomiio")]
    sid: String,
    /// OAuth application client identifier.
    #[arg(long, default_value = HA_OAUTH_CLIENT_ID)]
    client_id: String,
    /// OAuth redirect URL registered for the application.
    #[arg(long, default_value = HA_OAUTH_REDIRECT_URL)]
    redirect_url: Url,
    /// Space-separated OAuth scopes.
    #[arg(long, value_delimiter = ' ')]
    scope: Vec<String>,
    /// Account identifier used as the credential file name.
    #[arg(long)]
    account: Option<String>,
}

struct LoginResult {
    account_id: String,
    credential: StoredCredential,
}

async fn login(arguments: LoginArguments) -> Result<(), Box<dyn Error>> {
    let mut result = if let Some(userpass) = arguments.userpass.clone() {
        login_with_userpass(&arguments, &userpass).await?
    } else if arguments.userpass_stdin {
        login_with_userpass(&arguments, &read_userpass_stdin()?).await?
    } else if arguments.qr {
        login_with_qr(&arguments).await?
    } else {
        login_with_oauth(&arguments).await?
    };
    if let Some(account_id) = arguments.account {
        result.account_id = account_id;
    }
    let path = credential_path(&result.account_id)?;
    write_toml(&path, &result.credential)?;
    println!("Login succeeded. Credential saved to {}.", path.display());
    Ok(())
}

async fn login_with_qr(arguments: &LoginArguments) -> Result<LoginResult, Box<dyn Error>> {
    let device_id = arguments
        .device_id
        .clone()
        .unwrap_or_else(generate_xiaomi_client_id);
    let mut client = QrLoginClient::new(&device_id)?;
    let challenge = client.begin_qr_login().await?;
    println!(
        "Scan this Xiaomi Home QR login URL:\n{}",
        challenge.login_url
    );
    if let Some(image_url) = challenge.image_url {
        println!("QR image: {image_url}");
    }
    println!("Waiting for confirmation in Xiaomi Home...");
    let credential = match client.wait_for_qr_login().await? {
        CloudLoginOutcome::Success(credential) => credential,
        CloudLoginOutcome::VerificationRequired { .. } | CloudLoginOutcome::CaptchaRequired(_) => {
            return Err("QR login returned an unexpected verification challenge".into());
        }
    };
    Ok(LoginResult {
        account_id: credential.user_id().to_owned(),
        credential: StoredCredential::Cloud {
            user_id: credential.user_id().to_owned(),
            service_token: credential.service_token().to_owned(),
            ssecurity: credential.ssecurity().to_owned(),
            device_id,
        },
    })
}

fn read_userpass_stdin() -> Result<String, Box<dyn Error>> {
    Ok(format!(
        "{}:{}",
        read_line("username> ")?,
        read_password_line("password> ")?
    ))
}
fn read_password_line(prompt: &str) -> Result<String, Box<dyn Error>> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    let value = value.trim_end_matches(['\r', '\n']).to_owned();
    if value.is_empty() {
        Err("a value is required".into())
    } else {
        Ok(value)
    }
}

async fn login_with_userpass(
    arguments: &LoginArguments,
    userpass: &str,
) -> Result<LoginResult, Box<dyn Error>> {
    let (account, password) = userpass
        .split_once(':')
        .ok_or("--userpass must be in USERNAME:PASSWORD format")?;
    if account.is_empty() || password.is_empty() {
        return Err("--userpass must contain both username and password".into());
    }
    let device_id = arguments
        .device_id
        .clone()
        .unwrap_or_else(generate_xiaomi_client_id);
    let mut client = CloudLoginClient::new(&arguments.region, &arguments.sid, &device_id)?;
    let mut outcome = client
        .login(CloudLoginRequest {
            account: account.to_owned(),
            password: password.to_owned(),
        })
        .await?;
    loop {
        match outcome {
            CloudLoginOutcome::Success(credential) => {
                return Ok(LoginResult {
                    account_id: credential.user_id().to_owned(),
                    credential: StoredCredential::Cloud {
                        user_id: credential.user_id().to_owned(),
                        service_token: credential.service_token().to_owned(),
                        ssecurity: credential.ssecurity().to_owned(),
                        device_id: device_id.clone(),
                    },
                });
            }
            CloudLoginOutcome::VerificationRequired { url } => {
                println!(
                    "Open this page and complete the required verification. Paste the SMS, email, or device-confirmation code below.\n{url}"
                );
                match read_optional_line("ticket (or press Enter to cancel)> ")? { Some(ticket) => outcome = client.continue_verification(VerificationProof::new(ticket)).await?, None => return Err("verification cancelled; retrying login would discard the active verification transaction".into()) }
            }
            CloudLoginOutcome::CaptchaRequired(challenge) => {
                let image_path =
                    std::env::temp_dir().join(format!("miot-captcha-{}.png", challenge.id));
                fs::write(&image_path, &challenge.image)?;
                println!(
                    "Open the CAPTCHA image at {} and enter its text.",
                    image_path.display()
                );
                if challenge.rejected_previous_answer {
                    println!(
                        "The previous CAPTCHA answer was rejected; use the replacement image."
                    );
                }
                let captcha = read_line("captcha> ")?;
                let _ = fs::remove_file(&image_path);
                outcome = client.submit_captcha(captcha).await?;
            }
        }
    }
}

fn read_line(prompt: &str) -> Result<String, Box<dyn Error>> {
    read_optional_line(prompt)?.ok_or_else(|| "a value is required".into())
}
fn read_optional_line(prompt: &str) -> Result<Option<String>, Box<dyn Error>> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    let value = value.trim().to_owned();
    Ok((!value.is_empty()).then_some(value))
}

async fn login_with_oauth(arguments: &LoginArguments) -> Result<LoginResult, Box<dyn Error>> {
    let redirect_url = oauth_redirect_url(&arguments.redirect_url)?;
    let mut client = OAuthLoginClient::new(
        &arguments.client_id,
        redirect_url,
        &arguments.region,
        arguments.device_id.as_deref().unwrap_or("miot-cli"),
    )?;
    let authorization_url = client.authorization_url(OAuthAuthorizationRequest {
        scopes: arguments.scope.clone(),
        skip_confirm: false,
    })?;
    println!(
        "Open this URL in a browser, then paste the complete callback URL:\n{authorization_url}"
    );
    print!("> ");
    io::stdout().flush()?;
    let mut callback = String::new();
    io::stdin().read_line(&mut callback)?;
    let credential = client
        .complete_callback(Url::parse(callback.trim())?)
        .await?;
    Ok(LoginResult {
        account_id: arguments.client_id.clone(),
        credential: stored_oauth_credential(&credential)?,
    })
}

fn generate_xiaomi_client_id() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).expect("secure random generation failed");
    bytes
        .iter()
        .map(|byte| ALPHABET[usize::from(*byte) % ALPHABET.len()] as char)
        .collect()
}
fn oauth_redirect_url(base_url: &Url) -> Result<Url, Box<dyn Error>> {
    if base_url.path() != "/" || base_url.query().is_some() || base_url.fragment().is_some() {
        return Ok(base_url.clone());
    }
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| io::Error::other("secure random generation failed"))?;
    let mut redirect_url = base_url.clone();
    redirect_url.set_path(&format!("/api/webhook/{}", u64::from_le_bytes(bytes)));
    Ok(redirect_url)
}
fn stored_oauth_credential(
    credential: &OAuthCredential,
) -> Result<StoredCredential, Box<dyn Error>> {
    Ok(StoredCredential::OAuth {
        access_token: credential.access_token().to_owned(),
        refresh_token: credential.refresh_token().to_owned(),
        expires_at_unix_seconds: credential
            .expires_at()
            .duration_since(UNIX_EPOCH)?
            .as_secs(),
    })
}
