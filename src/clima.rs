//! Aviso de chuva (só Rust): previsão hora a hora da Open-Meteo (grátis, sem chave) → períodos com
//! chance alta → mensagem. Os avisos não passam pelo Claude; a ferramenta `previsao_chuva` usa `tabela_chuva`.
use crate::rede::{http, ler_json};
use crate::util::{fmt_g, Erro, Origem, R};
use chrono::{NaiveDate, NaiveDateTime, Timelike};
use serde_json::Value;
use std::time::Duration;

pub const OPEN_METEO: &str = "https://api.open-meteo.com/v1";

pub struct Clima {
    agent: ureq::Agent,
    /// OPEN_METEO (trocado só nos testes, pra um servidor falso local)
    pub base: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hora {
    pub inicio: NaiveDateTime,
    /// probabilidade de chuva em %
    pub prob: f64,
    pub mm: f64,
}

/// hora da previsão do alerta de temporal (temporal.rs)
#[derive(Debug, Clone, PartialEq)]
pub struct HoraExtrema {
    pub inicio: NaiveDateTime,
    pub mm: f64,
    /// rajada a 10 m, km/h
    pub rajada: f64,
    /// código WMO (96/99 = trovoada com granizo)
    pub codigo: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Periodo {
    pub de_h: u32,
    /// exclusivo (24 = até o fim do dia)
    pub ate_h: u32,
    pub prob_max: f64,
    pub mm_total: f64,
}

impl Clima {
    pub fn new(agent: ureq::Agent) -> Clima {
        Clima { agent, base: OPEN_METEO.into() }
    }

    /// 7 dias, hora local do fuso da instância
    pub fn previsao(&self, lat: f64, lon: f64) -> R<Vec<Hora>> {
        let url = format!(
            "{}/forecast?latitude={lat}&longitude={lon}&hourly=precipitation_probability,precipitation&timezone={}&forecast_days=7",
            self.base,
            crate::util::quote(crate::util::fuso().name()).replace('/', "%2F")
        );
        let r = self.agent.get(&url).timeout(Duration::from_secs(20)).call();
        horas(&ler_json(http(r, Origem::Clima)?)?)
    }

    /// pro alerta de temporal: chuva, rajada e código do tempo, 3 dias (o `/temporal` olha 48 h)
    pub fn extremos(&self, lat: f64, lon: f64) -> R<Vec<HoraExtrema>> {
        let url = format!(
            "{}/forecast?latitude={lat}&longitude={lon}&hourly=precipitation,wind_gusts_10m,weather_code&timezone={}&forecast_days=3",
            self.base,
            crate::util::quote(crate::util::fuso().name()).replace('/', "%2F")
        );
        let r = self.agent.get(&url).timeout(Duration::from_secs(20)).call();
        horas_extremas(&ler_json(http(r, Origem::Clima)?)?)
    }
}

/// `hourly` (precipitation, wind_gusts_10m, weather_code) → horas; null vira 0
pub fn horas_extremas(resp: &Value) -> R<Vec<HoraExtrema>> {
    let h = resp.get("hourly").ok_or_else(|| Erro::Chave("'hourly'".into()))?;
    let col = |k: &str| h.get(k).and_then(Value::as_array).ok_or_else(|| Erro::Chave(format!("'{k}'")));
    let (tempos, mms, rajadas, codigos) = (col("time")?, col("precipitation")?, col("wind_gusts_10m")?, col("weather_code")?);
    tempos
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let t = t.as_str().ok_or_else(|| Erro::Valor("hora inválida".into()))?;
            let inicio = NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M").map_err(|e| Erro::Valor(format!("{t}: {e}")))?;
            let num = |c: &[Value]| c.get(i).and_then(Value::as_f64).unwrap_or(0.0);
            Ok(HoraExtrema { inicio, mm: num(mms), rajada: num(rajadas), codigo: num(codigos) as u32 })
        })
        .collect()
}

/// `hourly` da Open-Meteo → horas; null vira 0
pub fn horas(resp: &Value) -> R<Vec<Hora>> {
    let h = resp.get("hourly").ok_or_else(|| Erro::Chave("'hourly'".into()))?;
    let col = |k: &str| h.get(k).and_then(Value::as_array).ok_or_else(|| Erro::Chave(format!("'{k}'")));
    let (tempos, probs, mms) = (col("time")?, col("precipitation_probability")?, col("precipitation")?);
    tempos
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let t = t.as_str().ok_or_else(|| Erro::Valor("hora inválida".into()))?;
            let inicio = NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M").map_err(|e| Erro::Valor(format!("{t}: {e}")))?;
            let num = |c: &[Value]| c.get(i).and_then(Value::as_f64).unwrap_or(0.0);
            Ok(Hora { inicio, prob: num(probs), mm: num(mms) })
        })
        .collect()
}

/// horas do `dia` a partir de `desde_h` com prob >= limiar, juntas em períodos; um buraco de 1 h no
/// meio não quebra o período (60% entre dois 80% é a mesma chuva)
pub fn periodos_chuva(horas: &[Hora], dia: NaiveDate, desde_h: u32, limiar: f64) -> Vec<Periodo> {
    let do_dia: Vec<&Hora> = horas.iter().filter(|h| h.inicio.date() == dia && h.inicio.hour() >= desde_h).collect();
    let mut out: Vec<Periodo> = vec![];
    for h in do_dia.iter().filter(|h| h.prob >= limiar) {
        let hr = h.inicio.hour();
        match out.last_mut() {
            Some(p) if hr <= p.ate_h + 1 => p.ate_h = hr + 1,
            _ => out.push(Periodo { de_h: hr, ate_h: hr + 1, prob_max: 0.0, mm_total: 0.0 }),
        }
    }
    for p in &mut out {
        for h in do_dia.iter().filter(|h| (p.de_h..p.ate_h).contains(&h.inicio.hour())) {
            p.prob_max = p.prob_max.max(h.prob);
            p.mm_total += h.mm;
        }
    }
    out
}

fn faixa(p: &Periodo) -> String {
    let fim = if p.ate_h >= 24 { "meia-noite".to_string() } else { format!("{}h", p.ate_h) };
    format!("{}h–{fim}", p.de_h)
}

/// "🌧️ <b>Chuva amanhã</b>\n• 14h–18h — até 85%, ~6 mm"
pub fn texto_chuva(rotulo: &str, periodos: &[Periodo]) -> String {
    let linhas: Vec<String> = periodos
        .iter()
        .map(|p| {
            let mm = (p.mm_total * 10.0).round() / 10.0;
            let chuva = if mm > 0.0 { format!(", ~{} mm", fmt_g(mm)) } else { String::new() };
            format!("• {} — até {}%{chuva}", faixa(p), fmt_g(p.prob_max))
        })
        .collect();
    format!("🌧️ <b>Chuva {rotulo}</b>\n{}", linhas.join("\n"))
}

/// pro atalho quando não passa do limiar: "☀️ hoje sem chuva forte prevista (máx 40% às 17h)"
pub fn resumo_sem_chuva(rotulo: &str, horas: &[Hora], dia: NaiveDate, desde_h: u32) -> String {
    let pico = horas.iter().filter(|h| h.inicio.date() == dia && h.inicio.hour() >= desde_h).fold(None::<&Hora>, |m, h| match m {
        Some(x) if x.prob >= h.prob => Some(x),
        _ => Some(h),
    });
    match pico {
        Some(h) if h.prob > 0.0 => format!("☀️ {rotulo} sem chuva forte prevista (máx {}% às {}h)", fmt_g(h.prob), h.inicio.hour()),
        Some(_) => format!("☀️ {rotulo} sem chuva prevista"),
        None => format!("Sem previsão pra {rotulo}."),
    }
}

/// pra ferramenta previsao_chuva: períodos >= limiar e as horas com alguma chance (o resto é 0%)
pub fn tabela_chuva(horas: &[Hora], dia: NaiveDate, desde_h: u32, limiar: f64) -> Value {
    let periodos: Vec<Value> = periodos_chuva(horas, dia, desde_h, limiar)
        .iter()
        .map(|p| serde_json::json!({"faixa": faixa(p), "prob_max": p.prob_max, "mm": (p.mm_total * 10.0).round() / 10.0}))
        .collect();
    let por_hora: Vec<Value> = horas
        .iter()
        .filter(|h| h.inicio.date() == dia && h.inicio.hour() >= desde_h && h.prob > 0.0)
        .map(|h| serde_json::json!({"hora": format!("{}h", h.inicio.hour()), "prob": h.prob, "mm": h.mm}))
        .collect();
    serde_json::json!({"dia": dia.format("%Y-%m-%d").to_string(), "desde": format!("{desde_h}h"), "limiar": limiar,
                       "periodos_acima_do_limiar": periodos, "horas_com_chance": por_hora,
                       "obs": "horas fora da lista: 0%"})
}
