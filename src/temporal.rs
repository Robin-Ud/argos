//! Alerta de temporal (só Rust): avisos do INMET por município (código IBGE) + previsão hora a hora da
//! Open-Meteo num ponto (ex.: um sítio da família longe de onde você mora): avisar ANTES do vendaval, pra dar
//! tempo de ligar e perguntar se tá tudo bem. Nunca passa pelo Claude.
//! Do INMET só vermelho (Grande Perigo) gera aviso: o laranja errava demais (6 avisos em 8 dias num município
//! do PR, out/2026, nenhum virou temporal). Amarelo e laranja só aparecem no `/temporal`.
use crate::clima::HoraExtrema;
use crate::rascunho::esc;
use crate::rede::http;
use crate::util::{fmt_g, Erro, Origem, R};
use chrono::{Duration, NaiveDate, NaiveDateTime, Timelike};
use serde::Deserialize;

pub const INMET: &str = "https://apiprevmet3.inmet.gov.br";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0";
/// janela da previsão local que gera aviso (o `/temporal` olha 48 h)
pub const JANELA_H: i64 = 36;
/// rajada local que conta como vermelho (fura o silêncio da madrugada)
pub const RAJADA_VERMELHA: f64 = 100.0;

pub struct Inmet {
    agent: ureq::Agent,
    /// INMET (trocado só nos testes, pra um servidor falso local)
    pub base: String,
}

/// aviso do INMET; o resto do JSON (polígono, lista de municípios, ícone em base64) é ignorado
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct Aviso {
    pub id_aviso: i64,
    #[serde(default)]
    pub descricao: String,
    #[serde(default)]
    pub severidade: String,
    /// "2026-10-05 00:00" (hora local)
    #[serde(default)]
    pub inicio: String,
    #[serde(default)]
    pub fim: String,
    #[serde(default)]
    pub riscos: Vec<String>,
    #[serde(default)]
    pub geocodes: String,
    #[serde(default)]
    pub encerrado: bool,
}

#[derive(Deserialize)]
struct Ativos {
    #[serde(default)]
    hoje: Vec<Aviso>,
    #[serde(default)]
    futuro: Vec<Aviso>,
}

impl Inmet {
    pub fn new(agent: ureq::Agent) -> Inmet {
        Inmet { agent, base: INMET.into() }
    }

    /// avisos ativos (hoje + futuro) do município, de qualquer cor. Sem User-Agent o INMET devolve vazio.
    pub fn avisos(&self, geocode: &str) -> R<Vec<Aviso>> {
        let r = self.agent.get(&format!("{}/avisos/ativos", self.base)).set("User-Agent", UA).timeout(std::time::Duration::from_secs(40)).call();
        let s = http(r, Origem::Inmet)?.into_string().map_err(|e| Erro::Rede(e.to_string()))?;
        let a: Ativos = serde_json::from_str(&s)?;
        drop(s);
        Ok(do_municipio(a.hoje.into_iter().chain(a.futuro), geocode))
    }
}

/// só os do município (código exato na lista `geocodes`), sem encerrados nem repetidos
pub fn do_municipio(avisos: impl Iterator<Item = Aviso>, geocode: &str) -> Vec<Aviso> {
    let mut out: Vec<Aviso> = vec![];
    for a in avisos {
        if !a.encerrado && a.geocodes.split(',').any(|g| g.trim() == geocode) && !out.iter().any(|b| b.id_aviso == a.id_aviso) {
            out.push(Aviso { geocodes: String::new(), ..a });
        }
    }
    out.sort_by(|a, b| a.inicio.cmp(&b.inicio).then(nivel(b).cmp(&nivel(a))));
    out
}

/// 0 amarelo/outro, 1 laranja, 2 vermelho
pub fn nivel(a: &Aviso) -> u8 {
    match a.severidade.as_str() {
        "Grande Perigo" => 2,
        "Perigo" => 1,
        _ => 0,
    }
}

fn bolinha(a: &Aviso) -> &'static str {
    ["🟡", "🟠", "🔴"][nivel(a) as usize]
}

/// o que gera aviso: só vermelho
pub fn criticos(avisos: &[Aviso]) -> Vec<&Aviso> {
    avisos.iter().filter(|a| nivel(a) == 2).collect()
}

pub fn chave_inmet(a: &Aviso) -> String {
    format!("inmet:{}:{}", a.id_aviso, nivel(a))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Limites {
    pub rajada_kmh: f64,
    pub chuva_mm_h: f64,
    pub chuva_mm_dia: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tipo {
    Rajada,
    Chuva,
    ChuvaDia,
    Granizo,
}

/// extremo da previsão local num dia: horas de/até, pico e a hora do pico
#[derive(Debug, Clone, PartialEq)]
pub struct Extremo {
    pub tipo: Tipo,
    pub dia: NaiveDate,
    pub de_h: u32,
    /// exclusivo
    pub ate_h: u32,
    pub pico: f64,
    pub hora_pico: u32,
}

impl Extremo {
    pub fn vermelho(&self) -> bool {
        self.tipo == Tipo::Rajada && self.pico >= RAJADA_VERMELHA
    }

    pub fn chave(&self) -> String {
        format!("local:{}:{:?}{}", self.dia.format("%Y-%m-%d"), self.tipo, if self.vermelho() { "!" } else { "" })
    }
}

/// horas de `agora` (a hora cheia) até `agora + janela_h` que passam de algum limite, um Extremo por dia e tipo.
/// Granizo = weather_code 96/99.
pub fn extremos_locais(horas: &[HoraExtrema], agora: NaiveDateTime, janela_h: i64, lim: &Limites) -> Vec<Extremo> {
    let ini = agora.with_minute(0).and_then(|t| t.with_second(0)).unwrap_or(agora);
    let fim = ini + Duration::hours(janela_h);
    let janela: Vec<&HoraExtrema> = horas.iter().filter(|h| h.inicio >= ini && h.inicio < fim).collect();
    let mut out: Vec<Extremo> = vec![];
    let mut junta = |tipo: Tipo, h: &HoraExtrema, valor: f64| {
        let (dia, hr) = (h.inicio.date(), h.inicio.hour());
        match out.iter_mut().find(|e| e.tipo == tipo && e.dia == dia) {
            Some(e) => {
                e.ate_h = hr + 1;
                if valor > e.pico {
                    (e.pico, e.hora_pico) = (valor, hr);
                }
            }
            None => out.push(Extremo { tipo, dia, de_h: hr, ate_h: hr + 1, pico: valor, hora_pico: hr }),
        }
    };
    for h in &janela {
        if h.rajada >= lim.rajada_kmh {
            junta(Tipo::Rajada, h, h.rajada);
        }
        if h.mm >= lim.chuva_mm_h {
            junta(Tipo::Chuva, h, h.mm);
        }
        if h.codigo == 96 || h.codigo == 99 {
            junta(Tipo::Granizo, h, 0.0);
        }
    }
    let mut dias: Vec<NaiveDate> = janela.iter().map(|h| h.inicio.date()).collect();
    dias.dedup();
    for d in dias {
        let do_dia: Vec<&&HoraExtrema> = janela.iter().filter(|h| h.inicio.date() == d).collect();
        let soma: f64 = do_dia.iter().map(|h| h.mm).sum();
        if soma >= lim.chuva_mm_dia {
            let p = do_dia.iter().fold(do_dia[0], |m, h| if h.mm > m.mm { h } else { m });
            out.push(Extremo {
                tipo: Tipo::ChuvaDia,
                dia: d,
                de_h: do_dia[0].inicio.hour(),
                ate_h: do_dia[do_dia.len() - 1].inicio.hour() + 1,
                pico: (soma * 10.0).round() / 10.0,
                hora_pico: p.inicio.hour(),
            });
        }
    }
    out.sort_by_key(|e| (e.dia, e.de_h));
    out
}

fn rotulo_dia(d: NaiveDate, hoje: NaiveDate) -> String {
    match (d - hoje).num_days() {
        0 => "hoje".into(),
        1 => "amanhã".into(),
        _ => d.format("%d/%m").to_string(),
    }
}

fn hora_br(t: NaiveDateTime) -> String {
    if t.minute() == 0 {
        format!("{}h", t.hour())
    } else {
        format!("{}h{:02}", t.hour(), t.minute())
    }
}

/// "hoje 0h → 23h59" / "hoje 8h → amanhã 12h"
pub fn faixa_inmet(a: &Aviso, hoje: NaiveDate) -> String {
    let p = |s: &str| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").ok();
    match (p(&a.inicio), p(&a.fim)) {
        (Some(i), Some(f)) if i.date() == f.date() => format!("{} {} → {}", rotulo_dia(i.date(), hoje), hora_br(i), hora_br(f)),
        (Some(i), Some(f)) => format!("{} {} → {} {}", rotulo_dia(i.date(), hoje), hora_br(i), rotulo_dia(f.date(), hoje), hora_br(f)),
        _ => format!("{} → {}", a.inicio, a.fim),
    }
}

/// primeira frase dos riscos (a parte com mm/h e km/h)
fn risco_curto(a: &Aviso) -> String {
    let r = a.riscos.first().map(String::as_str).unwrap_or("").trim();
    match r.find(". ") {
        Some(i) => r[..=i].to_string(),
        None => r.to_string(),
    }
}

fn linha_inmet(a: &Aviso, hoje: NaiveDate) -> String {
    let risco = risco_curto(a);
    let risco = if risco.is_empty() { String::new() } else { format!("\n{}", esc(&risco)) };
    format!("{} <b>{} ({})</b> — {}{risco}", bolinha(a), esc(&a.descricao), esc(&a.severidade), faixa_inmet(a, hoje))
}

fn linha_local(e: &Extremo, hoje: NaiveDate) -> String {
    let dia = rotulo_dia(e.dia, hoje);
    let faixa = if e.ate_h >= 24 { format!("{}h–meia-noite", e.de_h) } else { format!("{}h–{}h", e.de_h, e.ate_h) };
    match e.tipo {
        Tipo::Rajada => format!("• {dia} {faixa}: rajadas até {} km/h ({}h)", fmt_g(e.pico.round()), e.hora_pico),
        Tipo::Chuva => format!("• {dia} {faixa}: chuva forte, até {} mm/h ({}h)", fmt_g((e.pico * 10.0).round() / 10.0), e.hora_pico),
        Tipo::Granizo => format!("• {dia} {faixa}: risco de granizo"),
        Tipo::ChuvaDia => format!("• {dia}: {} mm de chuva no dia", fmt_g(e.pico)),
    }
}

fn bloco_local(locais: &[Extremo], hoje: NaiveDate) -> String {
    let l: Vec<String> = locais.iter().map(|e| linha_local(e, hoje)).collect();
    format!("📍 <b>Na propriedade</b> (previsão):\n{}", l.join("\n"))
}

/// mensagem do aviso automático (só o que é novo)
pub fn texto_alerta(nome: &str, inmet: &[&Aviso], locais: &[Extremo], hoje: NaiveDate) -> String {
    let mut partes = vec![format!("⚠️ <b>Temporal em {}</b>", esc(nome))];
    partes.extend(inmet.iter().map(|a| linha_inmet(a, hoje)));
    if !locais.is_empty() {
        partes.push(bloco_local(locais, hoje));
    }
    partes.push("👉 Vale ligar pra ver se tá tudo bem.".into());
    partes.join("\n\n")
}

/// resposta do `/temporal`: avisos de qualquer cor + extremos de 48 h; sem nada, o pico do vento e da chuva.
/// Fonte que falhou vira uma linha avisando.
pub fn texto_status(nome: &str, inmet: &R<Vec<Aviso>>, horas: &R<Vec<HoraExtrema>>, agora: NaiveDateTime, lim: &Limites) -> String {
    let hoje = agora.date();
    let mut partes = vec![];
    match inmet {
        Ok(a) if !a.is_empty() => partes.extend(a.iter().map(|a| linha_inmet(a, hoje))),
        Ok(_) => partes.push("INMET: nenhum aviso pra cá.".into()),
        Err(e) => partes.push(format!("INMET fora do ar ({})", esc(&e.to_string()))),
    }
    match horas {
        Ok(h) => {
            let ext = extremos_locais(h, agora, 48, lim);
            if ext.is_empty() {
                let ini = agora.with_minute(0).unwrap_or(agora);
                let jan: Vec<&HoraExtrema> = h.iter().filter(|x| x.inicio >= ini && x.inicio < ini + Duration::hours(48)).collect();
                let vento = jan.iter().fold(None::<&&HoraExtrema>, |m, x| match m { Some(y) if y.rajada >= x.rajada => Some(y), _ => Some(x) });
                let chuva = jan.iter().fold(None::<&&HoraExtrema>, |m, x| match m { Some(y) if y.mm >= x.mm => Some(y), _ => Some(x) });
                if let (Some(v), Some(c)) = (vento, chuva) {
                    partes.push(format!(
                        "📍 Na propriedade, 48 h: rajada máx {} km/h ({} {}h), chuva máx {} mm/h.",
                        fmt_g(v.rajada.round()),
                        rotulo_dia(v.inicio.date(), hoje),
                        v.inicio.hour(),
                        fmt_g((c.mm * 10.0).round() / 10.0)
                    ));
                }
            } else {
                partes.push(bloco_local(&ext, hoje));
            }
        }
        Err(e) => partes.push(format!("Previsão local fora do ar ({})", esc(&e.to_string()))),
    }
    let critico = inmet.as_ref().map(|a| !criticos(a).is_empty()).unwrap_or(false)
        || horas.as_ref().map(|h| !extremos_locais(h, agora, 48, lim).is_empty()).unwrap_or(false);
    let cabeca = if critico { format!("⚠️ <b>Temporal em {}</b>", esc(nome)) } else { format!("✅ <b>{}</b>: nada crítico previsto", esc(nome)) };
    format!("{cabeca}\n\n{}", partes.join("\n\n"))
}

/// "23:00"–"06:00" atravessa a meia-noite
pub fn em_silencio(hm: &str, janela: &(String, String)) -> bool {
    let (a, b) = (janela.0.as_str(), janela.1.as_str());
    if a <= b {
        hm >= a && hm < b
    } else {
        hm >= a || hm < b
    }
}
