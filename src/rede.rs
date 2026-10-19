//! ZONE 2/3/6b: HTTP. Um único ureq::Agent (keep-alive, rustls) serve Telegram, Google e Anthropic.
use crate::rascunho::Contexto;
use crate::util::{fmt_g, Erro, Origem, R};
use base64::Engine;
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;

pub fn agente() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .max_idle_connections(4)
        .max_idle_connections_per_host(1)
        .user_agent("argos-rs")
        .build()
}

/// agente sem keep-alive: o INMET derruba a conexão parada e a reaproveitada 30 min depois falhava
/// (05–13/10: "Network Error" em toda consulta das :30, a das :00 abria conexão nova e passava)
pub fn agente_sem_pool() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(20)).max_idle_connections(0).user_agent("argos-rs").build()
}

/// converte o erro do ureq; nunca põe a URL na mensagem (a do Telegram tem o token)
pub fn http(r: Result<ureq::Response, ureq::Error>, origem: Origem) -> R<ureq::Response> {
    match r {
        Ok(resp) => Ok(resp),
        Err(ureq::Error::Status(code, resp)) => {
            let motivo = resp.status_text().to_string();
            let corpo = resp.into_string().unwrap_or_default();
            Err(Erro::Http { code, motivo, corpo, origem })
        }
        Err(ureq::Error::Transport(t)) => Err(Erro::Rede(format!("{}{}", t.kind(), t.message().map(|m| format!(": {m}")).unwrap_or_default()))),
    }
}

pub fn ler_json(resp: ureq::Response) -> R<Value> {
    let s = resp.into_string().map_err(|e| Erro::Rede(e.to_string()))?;
    if s.is_empty() {
        return Ok(json!({}));
    }
    Ok(serde_json::from_str(&s)?)
}

// ==========================================
// ZONE 2: TELEGRAM
// ==========================================

pub struct Telegram {
    agent: ureq::Agent,
    /// https://api.telegram.org/bot<token> (trocado só nos testes)
    pub base: String,
    file_base: String,
    /// dados/conversa.jsonl (últimas 24 h, ver `conversa.rs`); None = não grava
    pub conversa: Option<std::path::PathBuf>,
    /// quando saiu a última mensagem (sendMessage/editMessageText que deu certo)
    pub ultimo_envio: std::cell::Cell<Option<std::time::Instant>>,
}

impl Telegram {
    pub fn new(agent: ureq::Agent, token: &str) -> Telegram {
        Telegram {
            agent,
            base: format!("https://api.telegram.org/bot{token}"),
            file_base: format!("https://api.telegram.org/file/bot{token}"),
            conversa: None,
            ultimo_envio: Default::default(),
        }
    }

    pub fn call(&self, method: &str, http_timeout: u64, params: Value) -> R<Value> {
        let r = self.call_sem_registro(method, http_timeout, &params)?;
        if let Some(t) = crate::conversa::do_envio(method, &params) {
            self.registra("argos", &t);
            self.ultimo_envio.set(Some(std::time::Instant::now()));
        }
        Ok(r)
    }

    /// grava uma fala na conversa das últimas 24 h; erro só vai pro log
    pub fn registra(&self, de: &str, texto: &str) {
        if let Some(p) = &self.conversa {
            if let Err(e) = crate::conversa::registrar(p, de, texto) {
                println!("conversa: {e}");
            }
        }
    }

    fn call_sem_registro(&self, method: &str, http_timeout: u64, params: &Value) -> R<Value> {
        let r = self
            .agent
            .post(&format!("{}/{method}", self.base))
            .timeout(Duration::from_secs(http_timeout))
            .set("Content-Type", "application/json")
            .send_string(&params.to_string());
        let resp = match http(r, Origem::Telegram) {
            Ok(r) => ler_json(r)?,
            // o Telegram responde 400/403 com {"ok": false, "description": ...}: vira o mesmo erro do ok=false
            Err(Erro::Http { corpo, code, motivo, origem }) => match serde_json::from_str::<Value>(&corpo) {
                Ok(v) if v.is_object() => v,
                _ => {
                    let _ = (motivo, origem);
                    return Err(Erro::Msg(format!("Telegram {method}: HTTP {code}")));
                }
            },
            Err(e) => return Err(e),
        };
        if resp.get("ok") != Some(&Value::Bool(true)) {
            let desc = resp.get("description").map(crate::util::str_num).unwrap_or_else(|| "None".into());
            return Err(Erro::Msg(format!("Telegram {method}: {desc}")));
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    pub fn send(&self, chat_id: i64, text: &str, keyboard: Option<Value>, html: bool) -> R<Value> {
        let mut p = json!({"chat_id": chat_id, "text": text});
        if html {
            p["parse_mode"] = json!("HTML");
        }
        if let Some(kb) = keyboard {
            p["reply_markup"] = json!({"inline_keyboard": kb});
        }
        self.call("sendMessage", 30, p)
    }

    pub fn edit(&self, chat_id: i64, message_id: i64, text: &str, keyboard: Option<Value>) -> R<()> {
        let p = json!({"chat_id": chat_id, "message_id": message_id, "text": text, "parse_mode": "HTML",
                       "reply_markup": {"inline_keyboard": keyboard.unwrap_or(json!([]))}});
        match self.call("editMessageText", 30, p) {
            Err(Erro::Msg(m)) if m.contains("not modified") => Ok(()),
            r => r.map(|_| ()),
        }
    }

    pub fn download_photo(&self, file_id: &str) -> R<Vec<u8>> {
        let info = self.call("getFile", 30, json!({"file_id": file_id}))?;
        let path = info.get("file_path").and_then(Value::as_str).ok_or_else(|| Erro::Chave("'file_path'".into()))?;
        let r = self.agent.get(&format!("{}/{path}", self.file_base)).timeout(Duration::from_secs(60)).call();
        let mut buf = vec![];
        // limite do getFile da Bot API é 20 MB
        http(r, Origem::Telegram)?.into_reader().take(20 << 20).read_to_end(&mut buf)?;
        Ok(buf)
    }
}

// ==========================================
// ZONE 3: CLAUDE (HTTP direto na Messages API)
// ==========================================

pub const BETA_FALLBACK: &str = "server-side-fallback-2026-07-01";
pub const BETA_BINDING: &str = "thinking-binding-controls-2026-08-01";

pub struct Claude {
    agent: ureq::Agent,
    key: String,
    pub url: String,
}

impl Claude {
    pub fn new(agent: ureq::Agent, key: &str) -> Claude {
        Claude { agent, key: key.to_string(), url: "https://api.anthropic.com/v1/messages".into() }
    }

    /// POST /v1/messages com até 2 novas tentativas (429, 5xx/529, falha de conexão), igual ao SDK
    pub fn create(&self, body: &Value, beta: Option<&str>) -> R<Value> {
        let payload = body.to_string();
        let mut tentativa = 0u32;
        loop {
            let mut req = self
                .agent
                .post(&self.url)
                .timeout(Duration::from_secs(600))
                .set("x-api-key", &self.key)
                .set("anthropic-version", "2023-06-01")
                .set("content-type", "application/json");
            if let Some(b) = beta {
                req = req.set("anthropic-beta", b);
            }
            let (retry_after, erro) = match req.send_string(&payload) {
                Ok(resp) => return ler_json(resp),
                Err(ureq::Error::Status(code, resp)) => {
                    let ra = retry_after(&resp);
                    let e = http(Err(ureq::Error::Status(code, resp)), Origem::Anthropic).unwrap_err();
                    if !(code == 429 || code >= 500) {
                        return Err(e);
                    }
                    (ra, e)
                }
                Err(t) => (None, http(Err(t), Origem::Anthropic).unwrap_err()),
            };
            if tentativa >= 2 {
                return Err(erro);
            }
            let espera = espera(tentativa, retry_after);
            println!("claude: {erro} — nova tentativa em {:.1}s", espera.as_secs_f64());
            std::thread::sleep(espera);
            tentativa += 1;
        }
    }
}

fn retry_after(resp: &ureq::Response) -> Option<f64> {
    if let Some(ms) = resp.header("retry-after-ms").and_then(|v| v.trim().parse::<f64>().ok()) {
        return Some(ms / 1000.0);
    }
    resp.header("retry-after").and_then(|v| v.trim().parse::<f64>().ok())
}

/// backoff do SDK: 0,5 s × 2^n (máx 8 s) com jitter; retry-after vale se estiver entre 0 e 60 s
fn espera(tentativa: u32, retry_after: Option<f64>) -> Duration {
    if let Some(s) = retry_after.filter(|s| *s > 0.0 && *s <= 60.0) {
        return Duration::from_secs_f64(s);
    }
    let base = (0.5 * 2f64.powi(tentativa as i32)).min(8.0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let jitter = 1.0 - 0.25 * (nanos % 1000) as f64 / 1000.0;
    Duration::from_secs_f64(base * jitter)
}

/// Haiku 4.5 não aceita effort nem fallback; Sonnet 5.5/Opus ganham os dois.
/// Haiku 5.5: effort sim, fallback não (não existe pra ele). Pensa por padrão e o thinking fica preso
/// ao prefixo; o `system` muda no meio da conversa (memória, catálogo de manutenção), então `drop_block`
/// descarta o thinking velho em vez de dar 400.
/// Devolve os campos extras do corpo e o header beta.
pub fn model_params(model: &str, effort: &str) -> (Value, Option<&'static str>) {
    if model.starts_with("claude-haiku-4") {
        return (json!({"model": model}), None);
    }
    if model.starts_with("claude-haiku") {
        let thinking = json!({"type": "adaptive", "block_binding": {"prefix_mismatch_behavior": "drop_block"}});
        return (json!({"model": model, "thinking": thinking, "output_config": {"effort": effort}}), Some(BETA_BINDING));
    }
    (json!({"model": model, "output_config": {"effort": effort}, "fallbacks": "default"}), Some(BETA_FALLBACK))
}

pub fn extraction_schema(vehicle_names: &[String], fuel_types: &[String]) -> Value {
    let num = json!({"anyOf": [{"type": "number"}, {"type": "null"}]});
    json!({
        "type": "object",
        "properties": {
            "total_reais": num,
            "litros": num,
            "preco_litro": num,
            "combustivel": {"anyOf": [{"type": "string", "enum": fuel_types}, {"type": "null"}]},
            "odometro_km": num,
            "veiculo": {"anyOf": [{"type": "string", "enum": vehicle_names}, {"type": "null"}]},
            "observacao": {"type": "string"},
            "cnpj_posto": {"anyOf": [{"type": "string"}, {"type": "null"}]},
        },
        "required": ["total_reais", "litros", "preco_litro", "combustivel", "odometro_km", "veiculo", "observacao", "cnpj_posto"],
        "additionalProperties": false,
    })
}

pub fn build_prompt(ctx: &Contexto) -> String {
    let vehicles = ctx
        .veiculos
        .iter()
        .map(|v| match v.last_odo {
            Some(o) => format!("- {}: último odômetro registrado {} km", v.nome, fmt_g(o)),
            None => format!("- {}: sem registros ainda", v.nome),
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "As fotos são de um abastecimento: o display da bomba de combustível e/ou o painel \
do veículo mostrando o odômetro.\n\n\
Leia da bomba: valor total pago (R$), litros abastecidos e preço por litro. \
Se o combustível aparecer (na bomba, no bico ou em adesivo), escolha da lista; \
senão deixe null.\n\
Leia do painel o odômetro TOTAL (ODO). Ignore TRIP/parcial, autonomia, relógio e \
temperatura. Mantenha a casa decimal se o display mostrar.\n\
Pelo tipo de painel (moto ou carro) e pelo odômetro, diga qual veículo é:\n\
{vehicles}\n\n\
Se uma das fotos for o cupom ou a nota fiscal do posto, leia o CNPJ do emitente (o posto, \
14 dígitos) em `cnpj_posto` e use o cupom pra conferir total e litros; sem cupom, null.\n\n\
Valor que não estiver legível → null. Não chute dígito: prefira null. \
Em `observacao`, uma frase curta em português só se houver algo duvidoso \
(reflexo, dígito cortado, foto que não é de abastecimento); senão string vazia."
    )
}

/// corpo da chamada de leitura (separado pra dar pra testar sem rede)
pub fn photos_body(modelo: &str, esforco: &str, photos: &[Vec<u8>], ctx: &Contexto) -> Value {
    let mut content: Vec<Value> = photos
        .iter()
        .map(|p| {
            json!({"type": "image", "source": {"type": "base64", "media_type": "image/jpeg",
                    "data": base64::engine::general_purpose::STANDARD.encode(p)}})
        })
        .collect();
    content.push(json!({"type": "text", "text": build_prompt(ctx)}));
    json!({
        "model": modelo,
        "max_tokens": 16000,
        "fallbacks": "default",
        "output_config": {"effort": esforco, "format": {"type": "json_schema", "schema": extraction_schema(&ctx.nomes(), &ctx.fuel_types)}},
        "messages": [{"role": "user", "content": content}],
    })
}

pub fn read_photos(claude: &Claude, modelo: &str, esforco: &str, photos: &[Vec<u8>], ctx: &Contexto) -> R<Value> {
    let body = photos_body(modelo, esforco, photos, ctx);
    let response = claude.create(&body, Some(BETA_FALLBACK))?;
    drop(body);
    crate::agenda::log_uso(&response);
    match response.get("stop_reason").and_then(Value::as_str) {
        Some("refusal") => return Err(Erro::Msg("o modelo recusou ler as fotos".into())),
        Some("max_tokens") => return Err(Erro::Msg("resposta cortada (max_tokens)".into())),
        _ => {}
    }
    let bl = blocos(&response);
    let text = bl
        .iter()
        .find(|b| b.get("type").and_then(Value::as_str) == Some("text"))
        .and_then(|b| b.get("text").and_then(Value::as_str))
        .ok_or_else(|| Erro::Msg("StopIteration: resposta sem bloco de texto".into()))?;
    Ok(serde_json::from_str(text)?)
}

pub fn blocos(response: &Value) -> Vec<Value> {
    response.get("content").and_then(Value::as_array).cloned().unwrap_or_default()
}
