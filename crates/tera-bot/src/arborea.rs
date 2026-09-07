use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

const BASE: &str = "https://arborea-reborn.com";
pub const BUILD_VERSION: u32 = 10002;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SavedArborea {
    pub refresh_token: String,
}

pub struct GameLogin {
    pub name: String,
    pub ticket: String,
    pub server: String,
    pub display_name: String,
}

#[derive(Deserialize)]
struct Grant {
    token: String,
    #[serde(rename = "grantType")]
    grant_type: String,
}

#[derive(Deserialize)]
struct GrantResponse {
    result: Vec<Grant>,
}

fn tokens_from(response: GrantResponse) -> Result<(String, String)> {
    let mut access = None;
    let mut refresh = None;
    for grant in response.result {
        match grant.grant_type.as_str() {
            "AccessToken" => access = Some(grant.token),
            "RefreshToken" => refresh = Some(grant.token),
            _ => {}
        }
    }
    match (access, refresh) {
        (Some(access), Some(refresh)) => Ok((access, refresh)),
        _ => bail!("réponse d'auth Arborea sans AccessToken/RefreshToken"),
    }
}

pub fn login(email: &str, password: &str, captcha: &str) -> Result<SavedArborea> {
    let response = ureq::post(&format!("{BASE}/api/authentication/login"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("Origin", BASE)
        .header("Referer", &format!("{BASE}/"))
        .header("X-Requested-With", "XMLHttpRequest")
        .header("Captcha", captcha)
        .send_json(serde_json::json!({ "email": email, "password": password, "code": null }))
        .context("login Arborea")?
        .body_mut()
        .read_json::<GrantResponse>()?;
    let (_, refresh_token) = tokens_from(response)?;
    Ok(SavedArborea { refresh_token })
}

fn refresh(refresh_token: &str) -> Result<String> {
    let value = ureq::post(&format!("{BASE}/api/Authentication/Token/Refresh"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .send_json(serde_json::Value::String(refresh_token.to_string()))
        .context("refresh Arborea")?
        .body_mut()
        .read_json::<serde_json::Value>()?;
    value
        .get("result")
        .and_then(|result| result.get("token"))
        .and_then(|token| token.as_str())
        .map(str::to_string)
        .context("refresh Arborea sans token")
}

fn server_list(access_token: &str) -> Result<String> {
    let xml = ureq::get(&format!("{BASE}/api/v2/sls"))
        .header("Authorization", &format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .call()
        .context("sls Arborea")?
        .body_mut()
        .read_to_string()?;
    Ok(xml)
}

fn user_me(access_token: &str) -> Result<(String, String, String)> {
    let value = ureq::get(&format!("{BASE}/api/User/Me"))
        .header("Authorization", &format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .call()
        .context("User/Me Arborea")?
        .body_mut()
        .read_json::<serde_json::Value>()?;
    let user = value.get("result").unwrap_or(&value);
    let field = |names: &[&str]| -> Option<String> {
        for name in names {
            if let Some(found) = user.get(name) {
                return match found {
                    serde_json::Value::String(text) => Some(text.clone()),
                    other => Some(other.to_string()),
                };
            }
        }
        None
    };
    let id = field(&["id", "Id"]).context("User/Me sans id")?;
    let name = field(&["userName", "UserName"]).unwrap_or_default();
    let ticket = field(&["authToken", "AuthToken"]).context("User/Me sans authToken")?;
    Ok((id, name, ticket))
}

fn extract(tag: &str, xml: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_string())
}

pub fn prepare(saved: &SavedArborea, _auth_path: &Path) -> Result<GameLogin> {
    let access_token = refresh(&saved.refresh_token)?;
    let xml = server_list(&access_token)?;
    let (id, display_name, ticket) = user_me(&access_token)?;
    let ip = extract("ip", &xml).context("sls sans <ip>")?;
    let port = extract("port", &xml).context("sls sans <port>")?;
    Ok(GameLogin {
        name: id,
        ticket,
        server: format!("{ip}:{port}"),
        display_name,
    })
}

pub fn load(path: &Path) -> Result<SavedArborea> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("lecture {} (fais d'abord --arborea-login)", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

pub fn save(path: &Path, saved: &SavedArborea) -> Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(saved)?)?;
    Ok(())
}
