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
    OAuthCredential, OAuthLoginClient,
};
use serde::Serialize;
use url::Url;

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
#[command(group(ArgGroup::new("login_mode").required(true).args(["userpass", "oauth"])))]
struct LoginArguments {
    /// Sign in with USERNAME:PASSWORD.
    #[arg(long)]
    userpass: Option<String>,
    /// Sign in through the OAuth authorization-code flow.
    #[arg(long)]
    oauth: bool,
    /// Xiaomi cloud region.
    #[arg(long, default_value = "cn")]
    region: String,
    /// Stable local device identifier.
    #[arg(long, default_value = "miot-cli")]
    device_id: String,
    /// Xiaomi service identifier for user-password login.
    #[arg(long, default_value = "xiaomiio")]
    sid: String,
    /// OAuth application client identifier.
    #[arg(long, required_if_eq("oauth", "true"))]
    client_id: Option<String>,
    /// OAuth redirect URL registered for the application.
    #[arg(long, required_if_eq("oauth", "true"))]
    redirect_url: Option<Url>,
    /// Space-separated OAuth scopes.
    #[arg(long, value_delimiter = ' ')]
    scope: Vec<String>,
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
    },
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
    let credential = if let Some(userpass) = arguments.userpass.clone() {
        login_with_userpass(&arguments, &userpass).await?
    } else {
        login_with_oauth(&arguments).await?
    };
    let path = credential_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_vec_pretty(&credential)?)?;
    println!("Login succeeded. Credential saved to {}.", path.display());
    Ok(())
}

async fn login_with_userpass(
    arguments: &LoginArguments,
    userpass: &str,
) -> Result<StoredCredential, Box<dyn Error>> {
    let (account, password) = userpass
        .split_once(':')
        .ok_or("--userpass must be in USERNAME:PASSWORD format")?;
    if account.is_empty() || password.is_empty() {
        return Err("--userpass must contain both username and password".into());
    }
    let mut client =
        CloudLoginClient::new(&arguments.region, &arguments.sid, &arguments.device_id)?;
    match client
        .login(CloudLoginRequest {
            account: account.to_owned(),
            password: password.to_owned(),
        })
        .await?
    {
        CloudLoginOutcome::Success(credential) => Ok(StoredCredential::Cloud {
            user_id: credential.user_id().to_owned(),
            service_token: credential.service_token().to_owned(),
            ssecurity: credential.ssecurity().to_owned(),
        }),
        CloudLoginOutcome::VerificationRequired { url } => {
            Err(format!("account verification is required: {url}").into())
        }
        CloudLoginOutcome::CaptchaRequired(_) => Err("captcha verification is required".into()),
    }
}

async fn login_with_oauth(arguments: &LoginArguments) -> Result<StoredCredential, Box<dyn Error>> {
    let client_id = arguments
        .client_id
        .as_deref()
        .ok_or("--client-id is required with --oauth")?;
    let redirect_url = arguments
        .redirect_url
        .clone()
        .ok_or("--redirect-url is required with --oauth")?;
    let mut client = OAuthLoginClient::new(
        client_id,
        redirect_url,
        &arguments.region,
        &arguments.device_id,
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
    stored_oauth_credential(&credential)
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

fn credential_path() -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/miot.rs"))
}
