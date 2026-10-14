//! Só Rust: as últimas 24 h da conversa em `dados/conversa.jsonl`, pro Claude Code ler sem print
//!. Uma linha por mensagem: `{"em", "de": "eu"|"argos", "texto"}`.
//! Foto, localização, botão e edição viram texto com prefixo entre colchetes. Falha aqui nunca derruba o bot.
use crate::util::{agora, from_iso, iso, Dt, R};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::Path;

pub const HORAS: i64 = 24;

/// acrescenta uma linha; se a primeira do arquivo já passou de 24 h, regrava só as recentes
pub fn registrar(path: &Path, de: &str, texto: &str) -> R<()> {
    let now = agora();
    if primeira_vencida(path, &now) {
        let todas = std::fs::read_to_string(path)?;
        std::fs::write(path, podar(&todas, &now))?;
    }
    let linha = json!({"em": iso(&now), "de": de, "texto": texto}).to_string();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{linha}")?;
    Ok(())
}

fn velha(linha: &str, now: &Dt) -> bool {
    let em = serde_json::from_str::<Value>(linha).ok().and_then(|v| v.get("em").and_then(Value::as_str).and_then(|s| from_iso(s).ok()));
    // linha ilegível também sai
    em.map_or(true, |em| (*now - em).num_hours() >= HORAS)
}

fn primeira_vencida(path: &Path, now: &Dt) -> bool {
    let Ok(f) = std::fs::File::open(path) else { return false };
    let mut l = String::new();
    std::io::BufReader::new(f).read_line(&mut l).is_ok() && !l.trim().is_empty() && velha(l.trim(), now)
}

/// pura: só as linhas das últimas 24 h
pub fn podar(todas: &str, now: &Dt) -> String {
    todas.lines().filter(|l| !l.trim().is_empty() && !velha(l, now)).map(|l| format!("{l}\n")).collect()
}

/// o que eu mandei, como texto: foto (com legenda), localização, texto
pub fn da_mensagem(m: &Value) -> Option<String> {
    let legenda = m.get("caption").and_then(Value::as_str).map(|c| format!(" {c}")).unwrap_or_default();
    if m.get("photo").is_some() {
        Some(format!("[foto]{legenda}"))
    } else if let Some(l) = m.get("location") {
        Some(format!("[localização] {}, {}", l.get("latitude")?, l.get("longitude")?))
    } else {
        m.get("text").and_then(Value::as_str).map(str::to_string)
    }
}

/// botão tocado: o rótulo que estava na tela (procura o callback_data no teclado da mensagem), senão o data cru
pub fn do_botao(q: &Value, data: &str) -> String {
    let rotulo = q.pointer("/message/reply_markup/inline_keyboard").and_then(Value::as_array).and_then(|linhas| {
        linhas.iter().filter_map(Value::as_array).flatten().find(|b| b.get("callback_data").and_then(Value::as_str) == Some(data))?.get("text")?.as_str()
    });
    format!("[botão] {}", rotulo.unwrap_or(data))
}

/// sendMessage / editMessageText que deram certo → texto do argos (o resto não é conversa)
pub fn do_envio(method: &str, params: &Value) -> Option<String> {
    let texto = params.get("text")?.as_str()?;
    match method {
        "sendMessage" => Some(texto.to_string()),
        "editMessageText" => Some(format!("[editou] {texto}")),
        _ => None,
    }
}
