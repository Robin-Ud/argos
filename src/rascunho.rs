//! ZONE 4: regras do abastecimento (puras, testáveis) + ZONE 6: mensagem e botões do rascunho.
use crate::motoregis::{csv_de, MotorConfig, Motoregis};
use crate::util::{fmt_f, fmt_g, fmt_g_sinal, py_float, py_round, Erro, R};
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

pub struct Veiculo {
    pub nome: String,
    /// moto ou carro (ícone e apelido "moto"/"carro" nos atalhos): `e_moto`
    pub moto: bool,
    pub csv: String,
    pub json: String,
    pub last_odo: Option<f64>,
    pub last_fuel_odo: Option<f64>,
    pub last_was_full: bool,
    pub history: Vec<f64>,
}

/// o `context` do argos.py
pub struct Contexto {
    pub veiculos: Vec<Veiculo>,
    pub fuel_types: Vec<String>,
}

impl Contexto {
    pub fn v(&self, nome: &str) -> &Veiculo {
        self.veiculos.iter().find(|v| v.nome == nome).expect("veículo do rascunho existe no contexto")
    }
    pub fn nomes(&self) -> Vec<String> {
        self.veiculos.iter().map(|v| v.nome.clone()).collect()
    }
}

/// leitura das fotos (o JSON do structured output)
#[derive(Default, Clone)]
pub struct Leitura {
    pub total_reais: Option<f64>,
    pub litros: Option<f64>,
    pub preco_litro: Option<f64>,
    pub combustivel: Option<String>,
    pub odometro_km: Option<f64>,
    pub veiculo: Option<String>,
    pub observacao: String,
    /// CNPJ do posto lido do cupom fiscal (só dígitos), se veio foto do cupom
    pub cnpj_posto: Option<String>,
}

impl Leitura {
    pub fn de_json(v: &Value) -> Leitura {
        let n = |k: &str| v.get(k).and_then(Value::as_f64);
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
        Leitura {
            total_reais: n("total_reais"),
            litros: n("litros"),
            preco_litro: n("preco_litro"),
            combustivel: s("combustivel"),
            odometro_km: n("odometro_km"),
            veiculo: s("veiculo"),
            observacao: s("observacao").unwrap_or_default(),
            cnpj_posto: s("cnpj_posto").as_deref().and_then(crate::postos::cnpj),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Rascunho {
    pub veiculo: String,
    pub data: String,
    pub total: Option<f64>,
    pub litros: Option<f64>,
    pub preco: Option<f64>,
    pub combustivel: Option<String>,
    pub odometro: Option<f64>,
    pub cheio: bool,
    pub gap: bool,
    pub trecho: Option<String>,
    pub obs: String,
    /// CNPJ do posto (cupom ou localização) → tag `posto=` no CSV
    pub posto: Option<String>,
    /// nome pra mostrar ("PAULISTA US (Vibra)"), se o posto está no postos.json
    pub posto_nome: Option<String>,
}

/// truthiness de número opcional (None e 0 são falsos)
fn sim(x: Option<f64>) -> bool {
    matches!(x, Some(v) if v != 0.0)
}

/// `"tipo": "moto"` no JSON do veículo; sem a chave (JSON do MotorCostTracker), moto é quem tem item de corrente
pub fn e_moto(veiculo: &Value) -> bool {
    match veiculo.get("tipo").and_then(Value::as_str) {
        Some(t) => t == "moto",
        None => veiculo
            .get("manutencoes")
            .and_then(Value::as_object)
            .is_some_and(|m| m.values().filter_map(Value::as_object).any(|o| o.keys().any(|k| k.contains("corrente")))),
    }
}

/// "Pra corrigir…": o exemplo de troca de veículo só quando tem mais de um (o 2º, palavra do nome que o
/// `apply_correction` reconhece)
pub fn texto_corrigir(veiculos: &[String], inicio: &str) -> String {
    let troca = veiculos
        .get(1)
        .and_then(|n| n.to_lowercase().split_whitespace().filter(|w| w.chars().count() > 3).last().map(String::from))
        .map(|w| format!(" ou <code>{}</code>", esc(&w)));
    match troca {
        Some(t) => format!("{inicio}, escreve tipo <code>km 3810</code>, <code>litros 8,5</code>, <code>total 52,30</code>, <code>etanol</code>{t}."),
        None => format!("{inicio}, escreve tipo <code>km 3810</code>, <code>litros 8,5</code>, <code>total 52,30</code> ou <code>etanol</code>."),
    }
}

pub fn vehicle_context(mr: &Motoregis, mconfig: &MotorConfig) -> R<Contexto> {
    let mut veiculos = vec![];
    for (nome, json_path) in &mconfig.veiculos_index {
        let csv = csv_de(json_path);
        let (last_fuel_odo, _, _, last_was_full) = mr.get_last_fuel_record(&csv)?;
        // último abastecimento, não a última linha: manutenção com km estimada pela média (09/10) pode ficar
        // acima da km real, e a foto seguinte da moto parecia "odômetro menor" e ia pro carro
        let lo = match last_fuel_odo {
            Some(o) => o,
            None => mr.get_last_odometer(&csv)?,
        };
        veiculos.push(Veiculo {
            nome: nome.clone(),
            moto: mr.load_json(json_path)?.as_ref().is_some_and(e_moto),
            history: mr.full_tank_km_history(&csv)?,
            csv,
            json: json_path.clone(),
            last_odo: if lo != 0.0 { Some(lo) } else { None },
            last_fuel_odo,
            last_was_full,
        });
    }
    Ok(Contexto { veiculos, fuel_types: mconfig.combustiveis.clone() })
}

/// Odômetro manda; o palpite do Claude (tipo de painel) desempata.
pub fn pick_vehicle(odo: Option<f64>, hint: Option<&str>, veiculos: &[Veiculo], km_max: f64) -> String {
    let plausible = |v: &Veiculo| match (odo, v.last_odo) {
        (Some(o), Some(l)) => 0.0 <= o - l && o - l <= km_max,
        _ => true,
    };
    let achado = hint.and_then(|h| veiculos.iter().find(|v| v.nome == h));
    if let Some(v) = achado {
        if plausible(v) {
            return v.nome.clone();
        }
    }
    if let Some(o) = odo {
        let mut fits: Vec<(f64, &str)> = veiculos
            .iter()
            .filter_map(|v| v.last_odo.map(|l| (o - l, v.nome.as_str())))
            .filter(|(d, _)| 0.0 <= *d && *d <= km_max)
            .collect();
        if !fits.is_empty() {
            // min() de tuplas do Python: menor distância, depois nome
            fits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(b.1)));
            return fits[0].1.to_string();
        }
        if let Some(v) = veiculos.iter().find(|v| v.last_odo.is_none()) {
            return v.nome.clone();
        }
    }
    match achado {
        Some(v) => v.nome.clone(),
        None => veiculos[0].nome.clone(),
    }
}

/// Junta duas leituras do mesmo abastecimento que chegaram separadas (bomba numa foto, painel na outra):
/// cada campo vem de quem tiver. Campo presente nas duas com valor diferente = outro abastecimento → None.
pub fn juntar(a: &Leitura, b: &Leitura) -> Option<Leitura> {
    fn num(x: Option<f64>, y: Option<f64>) -> Result<Option<f64>, ()> {
        match (x, y) {
            (Some(p), Some(q)) if (p - q).abs() > 0.011 * p.abs().max(q.abs()).max(1.0) => Err(()),
            (p, q) => Ok(p.or(q)),
        }
    }
    fn txt(x: &Option<String>, y: &Option<String>) -> Result<Option<String>, ()> {
        match (x, y) {
            (Some(p), Some(q)) if crate::agenda::fold(p) != crate::agenda::fold(q) => Err(()),
            (p, q) => Ok(p.clone().or_else(|| q.clone())),
        }
    }
    // a nota de cada foto fala do que faltava nela ("sem odômetro", "valores nulos"): junto, ficou velha (18/10)
    Some(Leitura {
        total_reais: num(a.total_reais, b.total_reais).ok()?,
        litros: num(a.litros, b.litros).ok()?,
        preco_litro: num(a.preco_litro, b.preco_litro).ok()?,
        combustivel: txt(&a.combustivel, &b.combustivel).ok()?,
        odometro_km: num(a.odometro_km, b.odometro_km).ok()?,
        veiculo: txt(&a.veiculo, &b.veiculo).ok()?,
        observacao: String::new(),
        cnpj_posto: txt(&a.cnpj_posto, &b.cnpj_posto).ok()?,
    })
}

pub fn new_draft(r: &Leitura, ctx: &Contexto, date_str: &str, km_max: f64) -> Rascunho {
    let name = pick_vehicle(r.odometro_km, r.veiculo.as_deref(), &ctx.veiculos, km_max);
    let (mut total, mut liters, price) = (r.total_reais, r.litros, r.preco_litro);
    // completa o que der pela conta da bomba
    if total.is_none() && sim(liters) && sim(price) {
        total = Some(py_round(liters.unwrap() * price.unwrap(), 2));
    }
    if liters.is_none() && sim(total) && sim(price) {
        liters = Some(py_round(total.unwrap() / price.unwrap(), 3));
    }
    let mut d = Rascunho {
        cheio: ctx.v(&name).last_was_full,
        veiculo: name,
        data: date_str.to_string(),
        total,
        litros: liters,
        preco: price,
        combustivel: r.combustivel.clone(),
        odometro: r.odometro_km,
        gap: false,
        trecho: None,
        obs: r.observacao.clone(),
        posto: r.cnpj_posto.as_deref().and_then(crate::postos::cnpj),
        posto_nome: None,
    };
    d.gap = suggest_gap(&d, ctx);
    d
}

pub fn km_driven(d: &Rascunho, ctx: &Contexto) -> Option<f64> {
    Some(d.odometro? - ctx.v(&d.veiculo).last_fuel_odo?)
}

/// statistics.mean / pstdev
fn media_desvio(h: &[f64]) -> (f64, f64) {
    let n = h.len() as f64;
    let m = h.iter().sum::<f64>() / n;
    let var = h.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / n;
    (m, var.sqrt())
}

/// Mesma regra do record_fuel: distância > média + 2σ dos trechos cheio-a-cheio.
pub fn suggest_gap(d: &Rascunho, ctx: &Contexto) -> bool {
    let v = ctx.v(&d.veiculo);
    let km = km_driven(d, ctx);
    if !(d.cheio && v.last_was_full && sim(km) && km.unwrap() > 0.0 && v.history.len() >= 3) {
        return false;
    }
    let (m, s) = media_desvio(&v.history);
    km.unwrap() > m + 2.0 * s
}

pub fn problems(d: &Rascunho, ctx: &Contexto, tolerance: f64, km_max: f64) -> Vec<String> {
    let mut out = vec![];
    for (val, label) in [(d.total, "valor total"), (d.litros, "litros"), (d.odometro, "odômetro")] {
        if val.is_none() {
            out.push(format!("falta {label}"));
        }
    }
    if d.combustivel.is_none() {
        out.push("falta combustível".into());
    }
    if sim(d.total) && sim(d.litros) && sim(d.preco) {
        let expected = d.litros.unwrap() * d.preco.unwrap();
        let total = d.total.unwrap();
        if (expected - total).abs() > tolerance * total {
            out.push(format!("litros × preço = R$ {}, mas o total lido é R$ {}", fmt_f(expected, 2), fmt_f(total, 2)));
        }
    }
    if let (Some(o), Some(last)) = (d.odometro, ctx.v(&d.veiculo).last_odo) {
        let delta = o - last;
        if delta < 0.0 {
            out.push(format!("odômetro menor que o último registro ({} km)", fmt_g(last)));
        } else if delta > km_max {
            out.push(format!("{} km desde o último registro, parece demais", fmt_g(delta)));
        }
    }
    out
}

pub fn details_field(d: &Rascunho) -> String {
    let mut s = format!("{}|{}", d.combustivel.as_deref().unwrap_or("None"), if d.cheio { "cheio" } else { "parcial" });
    if d.gap {
        s.push_str("|gap");
    }
    if let Some(t) = d.trecho.as_deref().filter(|t| !t.is_empty()) {
        s.push('|');
        s.push_str(t);
    }
    if let Some(p) = d.posto.as_deref() {
        s.push_str("|posto=");
        s.push_str(p);
    }
    s
}

pub fn csv_row(d: &Rascunho) -> R<Vec<String>> {
    let falta = |x: Option<f64>| x.ok_or_else(|| Erro::Chave("unsupported format string passed to NoneType.__format__".into()));
    Ok(vec![
        d.data.clone(),
        fmt_g(falta(d.odometro)?),
        "1".into(),
        fmt_f(falta(d.total)?, 2),
        fmt_f(falta(d.litros)?, 2),
        details_field(d),
    ])
}

fn fields() -> &'static [(&'static str, Regex); 4] {
    static F: OnceLock<[(&str, Regex); 4]> = OnceLock::new();
    F.get_or_init(|| {
        let num = r"(\d[\d.,]*)";
        let re = |p: String| Regex::new(&p).expect("regex válida");
        [
            ("odometro", re(format!(r"\b(?:odo|od[oô]metro|km)\s*[:=]?\s*{num}"))),
            ("litros", re(format!(r"\b(?:litros?|lts?|l)\s*[:=]?\s*{num}"))),
            ("total", re(format!(r"(?:\btotal|\bvalor|r\$)\s*[:=]?\s*{num}"))),
            ("preco", re(format!(r"\bpre[cç]o\s*[:=]?\s*{num}"))),
        ]
    })
}

/// Depois do ✏️ Corrigir, número sem nome de campo ("55.41 n . 43", 18/10): vai pro campo cujo valor atual está
/// a até 5% dele (o mais perto); km perto (até `km_max`) do odômetro do rascunho ou do último abastecimento →
/// odômetro. Número que não bate com nada é ignorado. Devolve os campos mudados.
pub fn numero_solto(d: &mut Rascunho, text: &str, last_odo: Option<f64>, km_max: f64) -> Vec<String> {
    static NUM: OnceLock<Regex> = OnceLock::new();
    let num = NUM.get_or_init(|| Regex::new(r"\d+(?:[.,]\d+)?").expect("regex válida"));
    let mut changed: Vec<String> = vec![];
    for m in num.find_iter(text) {
        let Ok(v) = m.as_str().replace(',', ".").parse::<f64>() else { continue };
        let perto = |atual: Option<f64>| atual.filter(|a| *a > 0.0).map(|a| (v - a).abs() / a).filter(|r| *r <= 0.05);
        let mut alvo: Vec<(&str, f64)> = [("total", perto(d.total)), ("litros", perto(d.litros)), ("preco", perto(d.preco))]
            .into_iter().filter_map(|(c, r)| r.map(|r| (c, r))).collect();
        let base_km = d.odometro.or(last_odo);
        if base_km.is_some_and(|o| v >= 100.0 && (v - o).abs() <= km_max) {
            alvo.push(("odometro", 1.0));
        }
        alvo.retain(|(c, _)| !changed.iter().any(|x| x == c));
        alvo.sort_by(|a, b| a.1.total_cmp(&b.1));
        let Some((campo, _)) = alvo.first() else { continue };
        match *campo {
            "total" => d.total = Some(v),
            "litros" => d.litros = Some(v),
            "preco" => d.preco = Some(v),
            _ => d.odometro = Some(v),
        }
        changed.push(campo.to_string());
    }
    if !changed.is_empty() {
        d.obs.clear();
    }
    changed
}

pub fn parse_number(raw: &str, field: &str) -> R<f64> {
    static MILHAR: OnceLock<Regex> = OnceLock::new();
    let milhar = MILHAR.get_or_init(|| Regex::new(r"^\d{1,3}(\.\d{3})+$").unwrap());
    let raw = raw.trim_end_matches(['.', ',']);
    let s = if field == "odometro" && milhar.is_match(raw) {
        raw.replace('.', "") // 3.810 no odômetro é milhar
    } else if raw.contains(',') {
        raw.replace('.', "").replace(',', ".")
    } else {
        raw.to_string()
    };
    py_float(&s)
}

/// Aplica correções em texto livre ('km 3810, litros 8,5'). Retorna o que mudou.
pub fn apply_correction(d: &mut Rascunho, text: &str, fuel_types: &[String], veiculos: &[String]) -> R<Vec<String>> {
    let low = text.to_lowercase();
    let mut changed = vec![];
    for (field, re) in fields() {
        if let Some(c) = re.captures(&low) {
            let v = Some(parse_number(&c[1], field)?);
            match *field {
                "odometro" => d.odometro = v,
                "litros" => d.litros = v,
                "total" => d.total = v,
                _ => d.preco = v,
            }
            changed.push(field.to_string());
        }
    }
    for fuel in fuel_types {
        let tok = fuel.rsplit('_').next().unwrap_or("");
        if low.contains(tok) {
            d.combustivel = Some(fuel.clone());
            changed.push("combustivel".into());
            break;
        }
    }
    for name in veiculos {
        if name.to_lowercase().split_whitespace().any(|tok| tok.chars().count() > 3 && low.contains(tok)) {
            d.veiculo = name.clone();
            changed.push("veiculo".into());
            break;
        }
    }
    if !changed.is_empty() {
        // a nota 💬 da leitura ("valores lidos com incerteza") não vale mais pro que ele corrigiu
        d.obs.clear();
    }
    Ok(changed)
}

// ==========================================
// ZONE 6: INTERFACE (mensagens e botões)
// ==========================================

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// '❓' se None, senão número com vírgula
fn fmt_num(v: Option<f64>, f: fn(f64) -> String) -> String {
    match v {
        None => "❓".into(),
        Some(x) => f(x).replace('.', ","),
    }
}

fn f2(x: f64) -> String {
    fmt_f(x, 2)
}

pub fn render(d: &Rascunho, ctx: &Contexto, tolerancia: f64, km_max: f64) -> String {
    let icon = if ctx.veiculos.iter().any(|v| v.nome == d.veiculo && v.moto) { "🏍" } else { "🚗" };
    let mut odo = fmt_num(d.odometro, fmt_g) + " km";
    if let Some(km) = km_driven(d, ctx) {
        odo += &format!(" ({} km)", fmt_g_sinal(km));
    }
    let fuel = d.combustivel.clone().unwrap_or_else(|| "❓".into());
    let fuel = if fuel.is_empty() { "❓".into() } else { fuel };
    let mut lines = vec![
        format!("{icon} <b>{}</b> · {}", esc(&d.veiculo), d.data),
        format!("R$ {} · {} L · R$ {}/L", fmt_num(d.total, f2), fmt_num(d.litros, f2), fmt_num(d.preco, f2)),
        format!("{fuel} · {odo}"),
        format!(
            "tanque: <b>{}</b>{}{}",
            if d.cheio { "cheio" } else { "parcial" },
            if d.gap { " · <b>gap</b>" } else { "" },
            d.trecho.as_deref().filter(|t| !t.is_empty()).map(|t| format!(" · {t}")).unwrap_or_default()
        ),
    ];
    match (d.posto_nome.as_deref(), d.posto.as_deref()) {
        (Some(n), _) => lines.push(format!("⛽ {}", esc(n))),
        (None, Some(c)) => lines.push(format!("⛽ CNPJ {}", crate::postos::cnpj_fmt(c))),
        _ => {}
    }
    if !d.obs.is_empty() {
        lines.push(format!("\n💬 {}", esc(&d.obs)));
    }
    let issues = problems(d, ctx, tolerancia, km_max);
    if !issues.is_empty() {
        lines.push(format!("\n⚠️ {}", issues.iter().map(|i| esc(i)).collect::<Vec<_>>().join("\n⚠️ ")));
    }
    if d.gap {
        lines.push("\nℹ️ distância bem acima da média: marquei como gap (abastecimento não registrado no meio)".into());
    }
    lines.join("\n")
}

pub fn botao(text: &str, data: &str) -> Value {
    json!({"text": text, "callback_data": data})
}

pub fn keyboard(d: &Rascunho, ctx: &Contexto, tolerancia: f64, km_max: f64) -> Value {
    let blocked = problems(d, ctx, tolerancia, km_max).iter().any(|p| p.starts_with("falta") || p.contains("menor"));
    let mut first = vec![botao("✏️ Corrigir", "fix")];
    if !blocked {
        first.insert(0, botao("✅ Salvar", "save"));
    }
    json!([
        first,
        [botao("cheio ⇄ parcial", "tank"), botao("gap", "gap"), botao("viagem", "trip:viagem"), botao("cidade", "trip:cidade")],
        [botao("🔁 outro veículo", "veh"), botao("❌ Cancelar", "cancel")],
    ])
}
