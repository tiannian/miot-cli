use std::{
    error::Error,
    fs,
    io::{self, Write},
    path::PathBuf,
    time::UNIX_EPOCH,
};

use clap::{ArgGroup, Args, Parser, Subcommand};
use miot_rs::{
    ApiClient, CloudCredential, CloudLoginClient, CloudLoginOutcome, CloudLoginRequest,
    HomeDeviceListQuery, OAuthAuthorizationRequest, OAuthCredential, OAuthLoginClient,
    VerificationProof,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    Update(UpdateArguments),
}

#[derive(Args)]
struct UpdateArguments {
    /// Account identifier passed to `miot auth login --account`.
    #[arg(long)]
    account: Option<String>,
    /// Xiaomi cloud region.
    #[arg(long, default_value = "cn")]
    region: String,
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

#[derive(Deserialize, Serialize)]
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
        Some(Command::Update(arguments)) => update(arguments).await,
        None => {
            println!("miot {}", miot_rs::version());
            Ok(())
        }
    }
}

async fn update(arguments: UpdateArguments) -> Result<(), Box<dyn Error>> {
    let (account_id, credential) = load_cloud_credential(arguments.account.as_deref())?;
    let client = ApiClient::new(&arguments.region, credential)?;
    let homes = client.home_merged().await?;
    write_json(&state_directory()?.join("homes.json"), &homes)?;

    let homes_to_update = homes
        .get("homelist")
        .or_else(|| homes.get("home_list"))
        .and_then(Value::as_array)
        .ok_or("cloud home response did not contain homelist")?;
    let mut updated = 0_usize;
    for home in homes_to_update {
        let home_id = required_i64(home, &["id", "home_id"])?;
        let home_owner = required_i64(home, &["owner_id", "home_owner", "uid"])?;
        let devices = update_home_devices(&client, home_owner, home_id).await?;
        write_json(
            &state_directory()?
                .join("devices")
                .join(format!("{home_id}.json")),
            &devices,
        )?;
        updated += 1;
    }
    println!(
        "Updated {updated} home(s) for account {account_id}. State saved to {}.",
        state_directory()?.display()
    );
    Ok(())
}

async fn update_home_devices(
    client: &ApiClient,
    home_owner: i64,
    home_id: i64,
) -> Result<Value, Box<dyn Error>> {
    let mut query = HomeDeviceListQuery::new(home_owner, home_id);
    let mut devices = Vec::new();
    loop {
        let page = client.home_device_list(&query).await?;
        let list = page
            .get("list")
            .or_else(|| page.get("device_info"))
            .and_then(Value::as_array)
            .ok_or("cloud home device response did not contain device list")?;
        devices.extend(list.iter().cloned());
        let next_start_did = page
            .get("next_start_did")
            .or_else(|| page.get("start_did"))
            .or_else(|| page.get("max_did"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if !page
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            break;
        }
        let Some(next_start_did) = next_start_did else {
            return Err(
                "cloud home device response indicated more pages without next_start_did".into(),
            );
        };
        if next_start_did == query.start_did {
            return Err("cloud home device pagination did not advance".into());
        }
        query.start_did = next_start_did;
    }
    Ok(serde_json::json!({
        "home_id": home_id,
        "home_owner": home_owner,
        "devices": devices,
    }))
}

fn required_i64(home: &Value, keys: &[&str]) -> Result<i64, Box<dyn Error>> {
    keys.iter()
        .find_map(|key| home.get(key).and_then(value_as_i64))
        .ok_or_else(|| {
            format!(
                "cloud home record did not contain any of {}",
                keys.join(", ")
            )
            .into()
        })
}

fn value_as_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| value.as_str().and_then(|number| number.parse().ok()))
}

fn load_cloud_credential(
    account: Option<&str>,
) -> Result<(String, CloudCredential), Box<dyn Error>> {
    let accounts_directory = state_directory()?.join("accounts");
    let path = match account {
        Some(account) => accounts_directory.join(format!("{}.json", filename_component(account))),
        None => {
            let mut paths = fs::read_dir(&accounts_directory)?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "json")
                })
                .collect::<Vec<_>>();
            paths.sort();
            match paths.as_slice() {
                [path] => path.clone(),
                [] => return Err("no saved account found; run `miot auth login` first".into()),
                _ => return Err("multiple saved accounts found; specify one with --account".into()),
            }
        }
    };
    let stored: StoredCredential = serde_json::from_slice(&fs::read(&path)?)?;
    let StoredCredential::Cloud {
        user_id,
        service_token,
        ssecurity,
        device_id: _,
    } = stored
    else {
        return Err("the selected account does not have a Xiaomi cloud credential".into());
    };
    let account_id = path
        .file_stem()
        .and_then(|name| name.to_str())
        .ok_or("saved account path has no valid file name")?
        .to_owned();
    Ok((
        account_id,
        CloudCredential::new(user_id, service_token, ssecurity),
    ))
}

fn write_json(path: &std::path::Path, value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    let parent = path.parent().ok_or("state path has no parent directory")?;
    fs::create_dir_all(parent)?;
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn state_directory() -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/miot.rs"))
}

async fn login(arguments: LoginArguments) -> Result<(), Box<dyn Error>> {
    let mut result = if let Some(userpass) = arguments.userpass.clone() {
        login_with_userpass(&arguments, &userpass).await?
    } else if arguments.userpass_stdin {
        let userpass = read_userpass_stdin()?;
        login_with_userpass(&arguments, &userpass).await?
    } else if arguments.qr {
        login_with_qr(&arguments).await?
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

async fn login_with_qr(arguments: &LoginArguments) -> Result<LoginResult, Box<dyn Error>> {
    let device_id = arguments
        .device_id
        .clone()
        .unwrap_or_else(generate_xiaomi_client_id);
    let mut client = CloudLoginClient::new(&arguments.region, &arguments.sid, &device_id)?;
    let challenge = client.begin_qr_login().await?;
    println!("Scan this Xiaomi Home QR login URL:");
    println!("{}", challenge.login_url);
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
    let username = read_line("username> ")?;
    let password = read_password_line("password> ")?;
    Ok(format!("{username}:{password}"))
}

fn read_password_line(prompt: &str) -> Result<String, Box<dyn Error>> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    let value = value.trim_end_matches(['\r', '\n']).to_owned();
    if value.is_empty() {
        return Err("a value is required".into());
    }
    Ok(value)
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
    let request = CloudLoginRequest {
        account: account.to_owned(),
        password: password.to_owned(),
    };
    let mut outcome = client.login(request).await?;
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
                    "Open this page and complete the required verification. Paste the SMS, email, or device-confirmation code below."
                );
                println!("{url}");
                match read_optional_line("ticket (or press Enter to cancel)> ")? {
                    Some(ticket) => {
                        outcome = client
                            .continue_verification(VerificationProof::new(ticket))
                            .await?;
                    }
                    None => {
                        return Err("verification cancelled; retrying login would discard the active verification transaction".into());
                    }
                }
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
    let value = read_optional_line(prompt)?;
    value.ok_or_else(|| "a value is required".into())
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

#[cfg(test)]
mod tests {
    use super::required_i64;
    use serde_json::json;

    #[test]
    fn reads_numeric_home_identifiers_from_cloud_response_strings() {
        let home = json!({ "id": "123", "uid": "456" });

        assert_eq!(required_i64(&home, &["id", "home_id"]).unwrap(), 123);
        assert_eq!(required_i64(&home, &["owner_id", "uid"]).unwrap(), 456);
    }
}
