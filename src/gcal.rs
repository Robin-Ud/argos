//! ZONE 6b: Google Calendar (OAuth com refresh token, CRUD em `primary`). O mesmo token pode servir a um complemento (`google_escopos_extra`).
//! `google_login` (`argos --google-login`): login único no navegador (loopback + PKCE) que grava o refresh token.
use crate::rede::{http, ler_json};
use crate::util::{iso, quote, unquote_plus, urlencode, Dt, Erro, Origem, R};
use base64::Engine;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const GCAL: &str = "https://www.googleapis.com/calendar/v3";
/// o que o bot usa; `google_escopos_extra` do configs.json soma outros (ex. drive.file pra um complemento)
pub const GOOGLE_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events";

pub struct GoogleCalendar {
    agent: ureq::Agent,
    client: Value,
    refresh: String,
    access: String,
    expires: f64,
    /// GCAL (trocado só nos testes, pra um servidor falso local)
    pub base: String,
}

fn agora_s() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn ler(path: &Path) -> R<Value> {
    let b = std::fs::read(path).map_err(|e| Erro::Msg(format!("{}: {e}", path.display())))?;
    Ok(serde_json::from_slice(&b)?)
}

impl GoogleCalendar {
    pub fn new(agent: ureq::Agent, cred_dir: &Path) -> R<GoogleCalendar> {
        let client = ler(&cred_dir.join("google_client.json"))?.get("installed").cloned().ok_or_else(|| Erro::Chave("'installed'".into()))?;
        let refresh = ler(&cred_dir.join("google_token.json"))?
            .get("refresh_token")
            .and_then(Value::as_str)
            .ok_or_else(|| Erro::Chave("'refresh_token'".into()))?
            .to_string();
        Ok(GoogleCalendar { agent, client, refresh, access: String::new(), expires: 0.0, base: GCAL.into() })
    }

    fn campo(&self, k: &str) -> R<String> {
        self.client.get(k).and_then(Value::as_str).map(String::from).ok_or_else(|| Erro::Chave(format!("'{k}'")))
    }

    /// access token válido (um complemento pode usar o mesmo refresh token, com os escopos dele no login)
    pub fn token(&mut self) -> R<String> {
        if agora_s() > self.expires - 60.0 {
            let form = urlencode(&[
                ("client_id", self.campo("client_id")?),
                ("client_secret", self.campo("client_secret")?),
                ("refresh_token", self.refresh.clone()),
                ("grant_type", "refresh_token".into()),
            ]);
            let r = self
                .agent
                .post(&self.campo("token_uri")?)
                .timeout(Duration::from_secs(30))
                .set("Content-Type", "application/x-www-form-urlencoded")
                .send_string(&form);
            let t = ler_json(http(r, Origem::Google)?)?;
            self.access = t.get("access_token").and_then(Value::as_str).ok_or_else(|| Erro::Chave("'access_token'".into()))?.to_string();
            self.expires = agora_s() + t.get("expires_in").and_then(Value::as_f64).ok_or_else(|| Erro::Chave("'expires_in'".into()))?;
        }
        Ok(self.access.clone())
    }

    pub fn agente(&self) -> &ureq::Agent {
        &self.agent
    }

    pub fn call(&mut self, method: &str, path: &str, params: &[(&str, String)], body: Option<&Value>) -> R<Value> {
        let mut url = format!("{}{path}", self.base);
        if !params.is_empty() {
            url = format!("{url}?{}", urlencode(params));
        }
        let auth = format!("Bearer {}", self.token()?);
        let req = self
            .agent
            .request(method, &url)
            .timeout(Duration::from_secs(30))
            .set("Authorization", &auth)
            .set("Content-Type", "application/json");
        // corpo vazio ({}) não vai, igual ao `if body` do Python
        let r = match body.filter(|b| crate::util::verdade(b)) {
            Some(b) => req.send_string(&b.to_string()),
            None => req.call(),
        };
        ler_json(http(r, Origem::Google)?)
    }

    pub fn list_events(&mut self, start: &Dt, end: &Dt, query: Option<&str>) -> R<Vec<Value>> {
        self.list_events_em("primary", start, end, query)
    }

    /// eventos de outra agenda que a conta enxerga (compartilhada com ela); `cal` = id da agenda (e-mail)
    pub fn list_events_em(&mut self, cal: &str, start: &Dt, end: &Dt, query: Option<&str>) -> R<Vec<Value>> {
        let mut params = vec![
            ("timeMin", iso(start)),
            ("timeMax", iso(end)),
            ("singleEvents", "true".to_string()),
            ("orderBy", "startTime".to_string()),
            ("maxResults", "250".to_string()),
            ("timeZone", crate::util::fuso().name().to_string()),
        ];
        if let Some(q) = query.filter(|q| !q.is_empty()) {
            params.push(("q", q.to_string()));
        }
        let r = self.call("GET", &format!("/calendars/{}/events", quote(cal)), &params, None)?;
        Ok(r.get("items").and_then(Value::as_array).cloned().unwrap_or_default())
    }

    pub fn get(&mut self, event_id: &str) -> R<Value> {
        self.call("GET", &format!("/calendars/primary/events/{}", quote(event_id)), &[], None)
    }

    pub fn create(&mut self, body: &Value) -> R<Value> {
        self.call("POST", "/calendars/primary/events", &[], Some(body))
    }

    pub fn patch(&mut self, event_id: &str, body: &Value) -> R<Value> {
        self.call("PATCH", &format!("/calendars/primary/events/{}", quote(event_id)), &[], Some(body))
    }

    pub fn delete(&mut self, event_id: &str) -> R<()> {
        self.call("DELETE", &format!("/calendars/primary/events/{}", quote(event_id)), &[], None).map(|_| ())
    }
}

fn b64url(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn aleatorio(n: usize) -> R<Vec<u8>> {
    let mut b = vec![0u8; n];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| Erro::Msg(format!("/dev/urandom: {e}")))?;
    Ok(b)
}

/// PKCE S256: code_challenge = base64url(sha256(verifier))
pub fn pkce_challenge(verifier: &str) -> String {
    b64url(&crate::util::sha256(verifier.as_bytes()))
}

/// URL de autorização (mesmos parâmetros do login do argos.py)
pub fn url_login(client: &Value, redirect: &str, state: &str, challenge: &str, escopos: &str) -> R<String> {
    let campo = |k: &str| client.get(k).and_then(Value::as_str).map(String::from).ok_or_else(|| Erro::Chave(format!("'{k}'")));
    Ok(format!("{}?{}", campo("auth_uri")?, urlencode(&[
        ("client_id", campo("client_id")?),
        ("redirect_uri", redirect.into()),
        ("response_type", "code".into()),
        ("scope", escopos.into()),
        ("access_type", "offline".into()),
        ("prompt", "consent".into()),
        ("state", state.into()),
        ("code_challenge", challenge.into()),
        ("code_challenge_method", "S256".into()),
    ])))
}

/// "GET /?state=..&code=.. HTTP/1.1" → os parâmetros, só se o `state` bate (senão None: outra requisição, ex. favicon)
pub fn retorno_login(linha: &str, state: &str) -> Option<Vec<(String, String)>> {
    let alvo = linha.split_whitespace().nth(1)?;
    let q = alvo.split_once('?')?.1;
    let ps: Vec<(String, String)> = q.split('&').filter_map(|p| p.split_once('=')).map(|(k, v)| (unquote_plus(k), unquote_plus(v))).collect();
    ps.iter().any(|(k, v)| k == "state" && v == state).then_some(ps)
}

/// login único no navegador (loopback + PKCE); grava `google_token.json` (0600) com o refresh token
pub fn google_login(agent: &ureq::Agent, cred_dir: &Path, escopos_extra: &[String]) -> R<()> {
    let escopos = std::iter::once(GOOGLE_SCOPE).chain(escopos_extra.iter().map(String::as_str)).collect::<Vec<_>>().join(" ");
    let client = ler(&cred_dir.join("google_client.json"))?.get("installed").cloned().ok_or_else(|| Erro::Chave("'installed'".into()))?;
    let campo = |k: &str| client.get(k).and_then(Value::as_str).map(String::from).ok_or_else(|| Erro::Chave(format!("'{k}'")));
    let verifier = b64url(&aleatorio(64)?);
    let state = b64url(&aleatorio(16)?);
    let server = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| Erro::Msg(format!("porta local: {e}")))?;
    let redirect = format!("http://127.0.0.1:{}", server.local_addr().map_err(|e| Erro::Msg(e.to_string()))?.port());
    let url = url_login(&client, &redirect, &state, &pkce_challenge(&verifier), &escopos)?;
    println!("Abrindo o navegador. Se não abrir, cole:\n{url}\n");
    let _ = std::process::Command::new("xdg-open").arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    let params = loop {
        let (mut sock, _) = server.accept().map_err(|e| Erro::Msg(e.to_string()))?;
        let mut linha = String::new();
        BufReader::new(&sock).read_line(&mut linha).map_err(|e| Erro::Msg(e.to_string()))?;
        let ps = retorno_login(&linha, &state);
        let ok = ps.as_ref().is_some_and(|ps| ps.iter().any(|(k, _)| k == "code"));
        let pagina = format!("<h2>Argos: {}</h2>", if ok { "autorizado ✅ pode fechar a aba" } else { "falhou ❌" });
        let _ = write!(sock, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{pagina}", pagina.len());
        if let Some(ps) = ps {
            break ps;
        }
    };
    let get = |k: &str| params.iter().find(|(p, _)| p == k).map(|(_, v)| v.clone());
    let code = get("code").ok_or_else(|| Erro::Msg(format!("autorização negada: {}", get("error").unwrap_or_default())))?;
    let form = urlencode(&[
        ("code", code),
        ("client_id", campo("client_id")?),
        ("client_secret", campo("client_secret")?),
        ("redirect_uri", redirect),
        ("grant_type", "authorization_code".into()),
        ("code_verifier", verifier),
    ]);
    let r = agent.post(&campo("token_uri")?).timeout(Duration::from_secs(30))
        .set("Content-Type", "application/x-www-form-urlencoded").send_string(&form);
    let token = ler_json(http(r, Origem::Google)?)?;
    let refresh = token.get("refresh_token").ok_or_else(|| Erro::Msg("o Google não devolveu refresh token".into()))?;
    let path = cred_dir.join("google_token.json");
    crate::util::grava_secreto(&path, &json!({"refresh_token": refresh, "scope": token.get("scope")}).to_string())?;
    println!("✅ token salvo em {}", path.display());
    Ok(())
}
