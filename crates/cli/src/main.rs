use std::{
    error::Error,
    fs,
    io::{self, Write},
    path::PathBuf,
    time::UNIX_EPOCH,
};

use clap::{ArgGroup, Args, Parser, Subcommand};
use miot_rs::{
    CloudLoginClient, CloudLoginOutcome, CloudLoginRequest, OAuthAuthorizationRequest,
    OAuthCredential, OAuthLoginClient, VerificationProof,
};
use serde::Serialize;
use url::Url;

const HA_OAUTH_CLIENT_ID: &str = "2882303761520251711";
const HA_OAUTH_REDIRECT_URL: &str = "http://homeassistant.local:8123";

#[derive(Parser)]
#[command(name = "miot", version, about = "MIoT command-line client")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Auth(AuthCommand),
}

#[derive(Args)]
struct AuthCommand {
    #[command(subcommand)]
    command: AuthSubcommand,
}

#[derive(Subcommand)]
enum AuthSubcommand {
    Login(LoginArguments),
}

#[derive(Args)]
#[command(group(ArgGroup::new("login_mode").required(true).args(["userpass", "userpass_stdin", "oauth"])))]
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

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredCredential {
    OAuth {
        access_token: String,
        refresh_token: String,
        expires_at_unix_seconds: u64,
    },
    Cloud {
        user_id: String,
        service_token: String,
        ssecurity: String,
        device_id: String,
    },
}

struct LoginResult {
    account_id: String,
    credential: StoredCredential,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run(Cli::parse()).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    match cli.command {
        Some(Command::Auth(AuthCommand {
            command: AuthSubcommand::Login(arguments),
        })) => login(arguments).await,
        None => {
            println!("miot {}", miot_rs::version());
            Ok(())
        }
    }
}

async fn login(arguments: LoginArguments) -> Result<(), Box<dyn Error>> {
    let mut result = if let Some(userpass) = arguments.userpass.clone() {
        login_with_userpass(&arguments, &userpass).await?
    } else if arguments.userpass_stdin {
        let userpass = read_userpass_stdin()?;
        login_with_userpass(&arguments, &userpass).await?
    } else {
        login_with_oauth(&arguments).await?
    };
    if let Some(account_id) = arguments.account {
        result.account_id = account_id;
    }
    let path = credential_path(&result.account_id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&result.credential)?)?;
    println!("Login succeeded. Credential saved to {}.", path.display());
    Ok(())
}

fn read_userpass_stdin() -> Result<String, Box<dyn Error>> {
    let username = read_line("username> ")?;
    let password = read_line("password> ")?;
    Ok(format!("{username}:{password}"))
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
                    "Complete account verification in a browser, then paste the verification ticket:"
                );
                println!("{url}");
                let ticket = read_line("ticket> ")?;
                outcome = client
                    .continue_verification(VerificationProof::new(ticket))
                    .await?;
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
    print!("{prompt}");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    let value = value.trim().to_owned();
    if value.is_empty() {
        return Err("a value is required".into());
    }
    Ok(value)
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
    println!("Open this URL in a browser, then paste the complete callback URL:");
    println!("{authorization_url}");
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
    let webhook_id = u64::from_le_bytes(bytes);
    let mut redirect_url = base_url.clone();
    redirect_url.set_path(&format!("/api/webhook/{webhook_id}"));
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

fn credential_path(account_id: &str) -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".local/miot.rs/accounts")
        .join(format!("{}.json", filename_component(account_id))))
}

fn filename_component(value: &str) -> String {
    let component: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    if component.is_empty() {
        "account".to_owned()
    } else {
        component
    }
}
