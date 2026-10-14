//! ZONE 6a: avisos ativos (tarefas + manutenção) — estado em dados/estado.json.
//! ZONE 5: gravação no MotorCostTracker (save, git).
use crate::motoregis::{compute_urgency, csv_de, display_name, MotorConfig, Motoregis};
use crate::rascunho::{csv_row, Contexto, Rascunho};
use crate::util::{dia_semana, fmt_f, fmt_g, from_iso, iso, parse_iso, so_data, Dt, Erro, R};
use chrono::{Duration, NaiveDate, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::Path;
use std::process::{Command, Stdio};

// ==========================================
// ESTADO (mesmo formato JSON do argos.py)
// ==========================================

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Tarefa {
    pub id: i64,
    pub texto: String,
    pub proximo: String,
    pub hora: String,
    pub repetir_dias: Option<i64>,
    pub desde_feito: bool,
    pub pendente: bool,
    pub cobrar_em: Option<String>,
    pub cobrancas: i64,
    /// campos que o argos.py (ou versão futura) gravou e este binário não conhece: voltam intactos
    #[serde(flatten)]
    pub resto: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Estado {
    #[serde(default)]
    pub tarefas: Vec<Tarefa>,
    #[serde(default)]
    pub avisos_manutencao: Map<String, Value>,
    #[serde(default = "um")]
    pub proximo_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manutencao_checada: Option<String>,
    #[serde(default)]
    pub memoria: Vec<Fato>,
    /// últimos grupos de ações que dá pra desfazer (ver desfazer.rs)
    #[serde(default)]
    pub desfazer: Vec<crate::desfazer::Entrada>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memoria_checada: Option<String>,
    /// dias já conferidos pelo aviso de chuva (clima.rs)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chuva_noite_checada: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chuva_manha_checada: Option<String>,
    /// dia em que o `argos --anp` já foi disparado (postos.rs)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postos_checado: Option<String>,
    /// alerta de temporal (temporal.rs): última consulta, avisos já mandados (chave → dia) e falha das fontes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerta_checado: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub alerta_enviados: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerta_falha_desde: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alerta_falha_avisada: Option<String>,
    /// chaves de topo desconhecidas (ex.: recursos novos do argos.py): preservadas no save
    #[serde(flatten)]
    pub resto: Map<String, Value>,
}

/// fato da memória permanente: {id, texto, criado, revisado, ate}
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Fato {
    pub id: i64,
    pub texto: String,
    pub criado: String,
    pub revisado: String,
    /// 'AAAA-MM-DD': último dia em que vale (None = permanente)
    #[serde(default)]
    pub ate: Option<String>,
    #[serde(flatten)]
    pub resto: Map<String, Value>,
}

fn um() -> i64 {
    1
}

impl Default for Estado {
    fn default() -> Self {
        Estado {
            tarefas: vec![],
            avisos_manutencao: Map::new(),
            proximo_id: 1,
            manutencao_checada: None,
            memoria: vec![],
            desfazer: vec![],
            memoria_checada: None,
            chuva_noite_checada: None,
            chuva_manha_checada: None,
            postos_checado: None,
            alerta_checado: None,
            alerta_enviados: Map::new(),
            alerta_falha_desde: None,
            alerta_falha_avisada: None,
            resto: Map::new(),
        }
    }
}

/// JSON quebrado ou ausente → estado vazio (igual ao Python). JSON válido com formato
/// inesperado → erro (melhor não subir do que sobrescrever as tarefas).
pub fn load_state(path: &Path) -> R<Estado> {
    let Ok(b) = std::fs::read(path) else { return Ok(Estado::default()) };
    let v: Value = match serde_json::from_slice(&b) {
        Ok(v) => v,
        Err(_) => return Ok(Estado::default()),
    };
    if !v.is_object() {
        return Ok(Estado::default());
    }
    serde_json::from_value(v).map_err(|e| Erro::Msg(format!("{}: formato inesperado ({e})", path.display())))
}

/// json.dump(indent=2, ensure_ascii=False) num .tmp + os.replace
pub fn save_state(state: &Estado, path: &Path) -> R<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_file_name(format!("{}.tmp", path.file_name().unwrap().to_string_lossy()));
    std::fs::write(&tmp, serde_json::to_string_pretty(state)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

// ==========================================
// TAREFAS (puras)
// ==========================================

pub struct CfgCobranca {
    pub cobrar_a_cada_h: f64,
    pub janela_cobranca: (String, String),
}

/// day.replace(hour=h, minute=m, second=0, microsecond=0)
pub fn at_hour(day: &Dt, hhmm: &str) -> R<Dt> {
    let (h, m) = hhmm.split_once(':').ok_or_else(|| Erro::Valor(format!("not enough values to unpack: {hhmm:?}")))?;
    let (h, m): (u32, u32) = (crate::util::py_int(h)? as u32, crate::util::py_int(m)? as u32);
    let t = NaiveTime::from_hms_opt(h, m, 0).ok_or_else(|| Erro::Valor("hour must be in 0..23".into()))?;
    Ok(day.offset().from_local_datetime(&day.date_naive().and_time(t)).single().unwrap())
}

/// Empurra pra dentro da janela de cobrança (ex.: 08:00–22:00).
pub fn into_window(dt: &Dt, w: &(String, String)) -> R<Dt> {
    let (start, end) = (at_hour(dt, &w.0)?, at_hour(dt, &w.1)?);
    if *dt < start {
        return Ok(start);
    }
    if *dt > end {
        return at_hour(&(*dt + Duration::days(1)), &w.0);
    }
    Ok(*dt)
}

/// Primeiro instante >= dt dentro de alguma das janelas ('08:00-11:59'), olhando até 8 dias.
pub fn into_janelas(dt: &Dt, js: &[(NaiveTime, NaiveTime)]) -> Dt {
    let mut js = js.to_vec();
    js.sort();
    for d in 0..8 {
        let dia = dt.date_naive() + Duration::days(d);
        for (a, b) in &js {
            let local = |t: &NaiveTime| dt.offset().from_local_datetime(&dia.and_time(*t)).single().unwrap();
            if *dt <= local(b) {
                return (*dt).max(local(a));
            }
        }
    }
    *dt
}

/// Janelas próprias da tarefa (`janelas` no JSON, ex.: só horário útil) ou a janela de cobrança geral.
pub fn janela_da_tarefa(t: &Tarefa, dt: &Dt, w: &(String, String)) -> R<Dt> {
    let js: Vec<(NaiveTime, NaiveTime)> = t.resto.get("janelas").and_then(Value::as_array).map(|a| {
        a.iter().filter_map(|j| crate::agenda::parse_janela(j.as_str().unwrap_or_default()).ok()).collect()
    }).unwrap_or_default();
    if js.is_empty() { into_window(dt, w) } else { Ok(into_janelas(dt, &js)) }
}

/// fim da pausa da tarefa (`pausada_ate` via `resto`), vencida ou não
pub fn pausa_ate(t: &Tarefa) -> Option<Dt> {
    t.resto.get("pausada_ate").and_then(Value::as_str).and_then(|s| from_iso(s).ok())
}

/// Pausa até `ate` (None = tira: vale a partir do próximo tick, que faz o `fim_da_pausa`).
pub fn set_pausa(t: &mut Tarefa, ate: Option<&Dt>, now: &Dt) {
    t.resto.insert("pausada_ate".into(), json!(iso(ate.unwrap_or(now))));
}

/// Pausa acabou (ex.: fim de semana fora, e cobrava regar planta e ligar pro médico).
/// Repetição fixa ("todo dia") pula o que caiu na pausa e, se estava cobrando, para (aquela vez passou);
/// o resto (uma vez só, "desde a última vez") volta a vencer/cobrar no fim da pausa, dentro das janelas.
fn fim_da_pausa(t: &mut Tarefa, fim: &Dt, w: &(String, String)) -> R<()> {
    t.resto.remove("pausada_ate");
    let mut due = from_iso(&t.proximo)?;
    match t.repetir_dias.filter(|d| *d > 0) {
        Some(d) if !t.desde_feito => {
            while due < *fim {
                due = due + Duration::days(d);
            }
            t.proximo = iso(&due);
            if t.pendente {
                t.pendente = false;
                t.cobrar_em = None;
                t.cobrancas = 0;
            }
        }
        _ if !t.pendente && due < *fim => t.proximo = iso(&janela_da_tarefa(t, fim, w)?),
        _ => {}
    }
    Ok(())
}

/// " · ⏸ até seg 19/10 00:00" se a pausa ainda vale
pub fn nota_pausa(t: &Tarefa, now: &Dt) -> String {
    match pausa_ate(t) {
        Some(fim) if fim > *now => fmt_when(&iso(&fim)).map(|q| format!(" · ⏸ até {q}")).unwrap_or_default(),
        _ => String::new(),
    }
}

/// horas entre cobranças: o `cobrar_a_cada_h` da tarefa (via `resto`) ou o padrão do configs.json
pub fn intervalo_h(t: &Tarefa, padrao: f64) -> f64 {
    t.resto.get("cobrar_a_cada_h").and_then(Value::as_f64).filter(|h| *h > 0.0).unwrap_or(padrao)
}

/// Grava o intervalo próprio da tarefa (horas, > 0); None = mantém.
pub fn set_intervalo(t: &mut Tarefa, h: Option<f64>) -> R<()> {
    let Some(h) = h else { return Ok(()) };
    if !(h > 0.0 && h <= 24.0 * 30.0) {
        return Err(Erro::Valor(format!("cobrar_a_cada_h inválido: {}", fmt_g(h))));
    }
    t.resto.insert("cobrar_a_cada_h".into(), json!(h));
    Ok(())
}

/// palavras que não identificam tarefa (verbo comum, tempo, pedido)
const PALAVRAS_VAZIAS: [&str; 30] = [
    "fazer", "comprar", "ligar", "pagar", "verificar", "conferir", "limpar", "trocar", "levar", "buscar", "mandar",
    "enviar", "falar", "marcar", "lembrar", "lembra", "acertar", "regar", "tomar", "arrumar", "tarefa", "hoje",
    "amanha", "todo", "toda", "dias", "semana", "cobrar", "aumente", "intervalo",
];

/// Tarefa existente que parece ser a mesma de `texto` (palavra de 4+ letras em comum, fora as vazias);
/// pendente primeiro. O criar_tarefa usa pra barrar a duplicata (13/10: "academia me lembra daqui 2 dias,
/// aumente o intervalo" virou tarefa nova "Academia: aumente o intervalo" com a #11 cobrando).
pub fn tarefa_parecida<'a>(tarefas: &'a [Tarefa], texto: &str) -> Option<&'a Tarefa> {
    let palavras = |s: &str| -> Vec<String> {
        crate::agenda::fold(s).split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 4 && !PALAVRAS_VAZIAS.contains(w)).map(String::from).collect()
    };
    let novas = palavras(texto);
    let bate = |t: &&Tarefa| palavras(&t.texto).iter().any(|w| novas.contains(w));
    tarefas.iter().filter(|t| t.pendente).find(bate).or_else(|| tarefas.iter().find(bate))
}

/// Grava as janelas da tarefa (valida o formato); vazio = mantém as que ela já tem.
pub fn set_janelas(t: &mut Tarefa, janelas: &[String]) -> R<()> {
    if janelas.is_empty() {
        return Ok(());
    }
    for j in janelas {
        crate::agenda::parse_janela(j)?;
    }
    t.resto.insert("janelas".into(), json!(janelas));
    Ok(())
}

/// when_to_aware: sem fuso → o da instância
pub fn when_to_aware(value: &str) -> R<Dt> {
    from_iso(value)
}

pub fn new_task(state: &mut Estado, text: &str, when: &str, every_days: Option<i64>, from_done: bool, default_hour: &str) -> R<Tarefa> {
    let when = if so_data(when) { format!("{when}T{default_hour}") } else { when.to_string() };
    let due = when_to_aware(&when)?;
    let every = every_days.filter(|d| *d != 0);
    let t = Tarefa {
        id: state.proximo_id,
        texto: text.to_string(),
        proximo: iso(&due),
        hora: due.format("%H:%M").to_string(),
        repetir_dias: every,
        desde_feito: from_done && every.is_some(),
        pendente: false,
        cobrar_em: None,
        cobrancas: 0,
        resto: Map::new(),
    };
    state.proximo_id += 1;
    state.tarefas.push(t.clone());
    Ok(t)
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Aviso {
    Vence,
    Cobra,
}

/// Avança o relógio de uma tarefa. Retorna Vence / Cobra quando é hora de mandar mensagem.
pub fn task_tick(t: &mut Tarefa, now: &Dt, cfg: &CfgCobranca) -> R<Option<Aviso>> {
    let every = Duration::milliseconds((intervalo_h(t, cfg.cobrar_a_cada_h) * 3_600_000.0) as i64);
    let w = &cfg.janela_cobranca;
    if let Some(fim) = pausa_ate(t) {
        if *now < fim {
            // pausada: nada vence nem cobra; cobrança marcada pra dentro da pausa vai pro fim dela
            if let Some(c) = t.cobrar_em.clone().filter(|c| t.pendente && !c.is_empty()) {
                if from_iso(&c)? < fim {
                    t.cobrar_em = Some(iso(&janela_da_tarefa(t, &fim, w)?));
                }
            }
            return Ok(None);
        }
        fim_da_pausa(t, &fim, w)?;
    }
    let mut due = from_iso(&t.proximo)?;
    if due <= *now && !(t.pendente && t.desde_feito) {
        // (repetir_dias <= 0 travaria o loop do Python pra sempre; aqui só não avança)
        if let Some(d) = t.repetir_dias.filter(|d| *d > 0) {
            if !t.desde_feito {
                while due <= *now {
                    // agenda fixa: já marca a próxima ocorrência
                    due = due + Duration::days(d);
                }
                t.proximo = iso(&due);
            }
        }
        if !t.pendente {
            t.pendente = true;
            t.cobrancas = 0;
            t.cobrar_em = Some(iso(&janela_da_tarefa(t, &(*now + every), w)?));
            return Ok(Some(Aviso::Vence));
        }
    }
    if let Some(c) = t.cobrar_em.clone().filter(|c| t.pendente && !c.is_empty()) {
        if from_iso(&c)? <= *now {
            let dentro = janela_da_tarefa(t, now, w)?;
            if dentro != *now {
                // fora da janela: espera amanhecer
                t.cobrar_em = Some(iso(&dentro));
                return Ok(None);
            }
            t.cobrancas += 1;
            t.cobrar_em = Some(iso(&janela_da_tarefa(t, &(*now + every), w)?));
            return Ok(Some(Aviso::Cobra));
        }
    }
    Ok(None)
}

/// O `task_tick` mandou avisar, mas ele está em compromisso até `fim`: não manda, cobra em `fim` (dentro das janelas).
/// Cobra desconta a contagem (a próxima é a mesma cobrança); Vence já ficou pendente e volta como ⏰.
pub fn adiar_por_compromisso(t: &mut Tarefa, k: Aviso, fim: &Dt, cfg: &CfgCobranca) -> R<()> {
    if k == Aviso::Cobra {
        t.cobrancas -= 1;
    }
    t.cobrar_em = Some(iso(&janela_da_tarefa(t, fim, &cfg.janela_cobranca)?));
    Ok(())
}

pub fn task_done(state: &mut Estado, id: i64, now: &Dt) -> R<String> {
    let pos = state.tarefas.iter().position(|t| t.id == id).ok_or_else(|| Erro::Valor(format!("tarefa {id} não existe")))?;
    let t = &mut state.tarefas[pos];
    let was_pending = t.pendente;
    t.pendente = false;
    t.cobrar_em = None;
    t.cobrancas = 0;
    let Some(rep) = t.repetir_dias else {
        state.tarefas.remove(pos);
        return Ok("feito ✅".into());
    };
    if t.desde_feito {
        t.proximo = iso(&at_hour(&(*now + Duration::days(rep)), &t.hora)?);
    } else if !was_pending {
        // fez adiantado: pula a próxima ocorrência se ela está perto (menos de meio intervalo)
        let nxt = from_iso(&t.proximo)?;
        if nxt - *now < Duration::milliseconds(rep * 86_400_000 / 2) {
            t.proximo = iso(&(nxt + Duration::days(rep)));
        }
    }
    Ok(format!("feito ✅ — próxima: {}", fmt_when(&t.proximo)?))
}

/// tarefa marcada como hábito (`habito: true`, via `resto`): cada conclusão vira linha no tracker.csv
pub fn e_habito(t: &Tarefa) -> bool {
    t.resto.get("habito") == Some(&Value::Bool(true))
}

pub const TRACKER_HEADER: [&str; 6] = ["em", "tarefa_id", "tarefa", "repetir_dias", "previsto", "estava_cobrando"];

/// linha do tracker pra uma conclusão agora (antes do `task_done`, que muda o `proximo`)
pub fn tracker_row(t: &Tarefa, now: &Dt) -> Vec<String> {
    let rep = t.repetir_dias.map(|r| r.to_string()).unwrap_or_default();
    vec![iso(now), t.id.to_string(), t.texto.clone(), rep, t.proximo.clone(), if t.pendente { "sim" } else { "nao" }.into()]
}

/// acrescenta no dados/tracker.csv (header na primeira vez), fim de linha \r\n como o csv.writer
pub fn append_tracker(path: &Path, row: &[String]) -> R<()> {
    use std::io::Write;
    let novo = !path.exists();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    if novo {
        write!(f, "{}\r\n", crate::motoregis::csv_line(&TRACKER_HEADER.map(String::from)))?;
    }
    write!(f, "{}\r\n", crate::motoregis::csv_line(row))?;
    Ok(())
}

pub fn task_snooze(t: &mut Tarefa, now: &Dt, kind: &str, cfg: &CfgCobranca) -> R<String> {
    let target = if kind == "1h" { *now + Duration::hours(1) } else { at_hour(&(*now + Duration::days(1)), &t.hora)? };
    t.pendente = true;
    let c = task_move(t, &target, cfg)?;
    Ok(format!("⏰ te cobro {}", fmt_when(&c)?))
}

/// Remaneja uma tarefa que já existe. Pendente: só a próxima cobrança vai pro alvo (a repetição não muda).
/// Ainda não venceu: o vencimento vai pro alvo (e o horário, se não repete). Devolve o novo quando (ISO).
pub fn task_move(t: &mut Tarefa, alvo: &Dt, cfg: &CfgCobranca) -> R<String> {
    if t.pendente {
        let c = iso(&janela_da_tarefa(t, alvo, &cfg.janela_cobranca)?);
        t.cobrar_em = Some(c.clone());
        return Ok(c);
    }
    t.proximo = iso(alvo);
    if t.repetir_dias.is_none() {
        t.hora = alvo.format("%H:%M").to_string();
    }
    Ok(t.proximo.clone())
}

/// Resposta ao "pra quando?" do botão 🕒: "18", "18h30", "18:30", "às 9", "amanhã 9", "sexta às 14", "hj as 16",
/// "daqui 2 dias", "em meia hora", "dia 15 às 9". Só o dia ("amanhã", "sexta", "daqui 2 dias") → na `hora_padrao`
/// (a hora da tarefa). Sem dia e horário já passado → amanhã. Qualquer outra coisa → None (o texto segue pra agenda).
pub fn parse_hora_livre(text: &str, now: &Dt, hora_padrao: &str) -> Option<Dt> {
    use chrono::Datelike;
    use std::sync::OnceLock;
    static RE: OnceLock<[regex::Regex; 3]> = OnceLock::new();
    const DIAS: [&str; 7] = ["segunda", "terca", "quarta", "quinta", "sexta", "sabado", "domingo"];
    const NUMS: [&str; 10] = ["um", "dois", "tres", "quatro", "cinco", "seis", "sete", "oito", "nove", "dez"];
    const HORA: &str = r"(?:(?:as|a|pras|para as) )?(\d{1,2})(?:[h:](\d{2})?)?";
    let [abs, rel, dia_mes] = RE.get_or_init(|| [
        regex::Regex::new(&format!(r"^(?:(hoje|amanha|depois de amanha|segunda|terca|quarta|quinta|sexta|sabado|domingo)(?:-feira)?(?: |$))?(?:{HORA})?$")).unwrap(),
        regex::Regex::new(&format!(r"^(?:daqui(?: a)?|em|dentro de) (\d{{1,2}}|uma?|dois|duas|tres|quatro|cinco|seis|sete|oito|nove|dez|meia) ?(dias?|horas?|hs?|min|minutos?)(?: {HORA})?$")).unwrap(),
        regex::Regex::new(&format!(r"^dia (\d{{1,2}})(?: {HORA})?$")).unwrap(),
    ]);
    let t = crate::agenda::fold(text.trim());
    // abreviações do teclado do celular
    let t: Vec<&str> = t.trim_end_matches(['.', '!', ' ']).split_whitespace().map(|w| match w {
        "hj" => "hoje",
        "amnh" | "amn" | "amanh" => "amanha",
        "dps" => "depois",
        "daki" => "daqui",
        w => w,
    }).collect();
    let t = t.join(" ");
    // "HH:MM" validado; sem hora = None
    let hhmm = |h: Option<regex::Match>, m: Option<regex::Match>| -> Option<Option<String>> {
        let Some(h) = h else { return Some(None) };
        let h: u32 = h.as_str().parse().ok()?;
        let m: u32 = m.map_or(Some(0), |x| x.as_str().parse().ok())?;
        (h <= 23 && m <= 59).then(|| Some(format!("{h:02}:{m:02}")))
    };
    let futuro = |d: Dt| (d > *now).then_some(d);

    if let Some(m) = rel.captures(&t) {
        let n: i64 = match &m[1] {
            "um" | "uma" => 1,
            "duas" => 2,
            "meia" => 0,
            x => x.parse().ok().or_else(|| NUMS.iter().position(|w| *w == x).map(|i| i as i64 + 1))?,
        };
        let unidade = &m[2];
        let hora = hhmm(m.get(3), m.get(4))?;
        if &m[1] == "meia" {
            return (unidade.starts_with('h') && hora.is_none()).then(|| *now + Duration::minutes(30));
        }
        if n == 0 {
            return None;
        }
        return if unidade.starts_with('d') {
            futuro(at_hour(&(*now + Duration::days(n)), hora.as_deref().unwrap_or(hora_padrao)).ok()?)
        } else if hora.is_some() {
            None
        } else if unidade.starts_with('h') {
            Some(*now + Duration::hours(n))
        } else {
            Some(*now + Duration::minutes(n))
        };
    }

    if let Some(m) = dia_mes.captures(&t) {
        let d: u32 = m[1].parse().ok()?;
        let hora = hhmm(m.get(2), m.get(3))?.unwrap_or_else(|| hora_padrao.to_string());
        let (mut y, mut mes) = (now.year(), now.month());
        for _ in 0..3 {
            if let Some(data) = NaiveDate::from_ymd_opt(y, mes, d) {
                let alvo = at_hour(&now.offset().from_local_datetime(&data.and_hms_opt(0, 0, 0)?).single()?, &hora).ok()?;
                if alvo > *now {
                    return Some(alvo);
                }
            }
            (y, mes) = if mes == 12 { (y + 1, 1) } else { (y, mes + 1) };
        }
        return None;
    }

    let m = abs.captures(&t)?;
    let dia = m.get(1).map(|d| d.as_str());
    let hora = hhmm(m.get(2), m.get(3))?;
    let hora = match (dia, hora) {
        (None, None) => return None,
        (_, Some(h)) => h,
        (Some(_), None) => hora_padrao.to_string(),
    };
    let ahead = match dia {
        None | Some("hoje") => 0,
        Some("amanha") => 1,
        Some("depois de amanha") => 2,
        Some(d) => {
            let i = DIAS.iter().position(|x| *x == d)? as i64;
            (i - now.weekday().num_days_from_monday() as i64).rem_euclid(7)
        }
    };
    let alvo = at_hour(&(*now + Duration::days(ahead)), &hora).ok()?;
    if alvo > *now {
        return Some(alvo);
    }
    match dia {
        None => at_hour(&(*now + Duration::days(1)), &hora).ok(),
        // "sexta 9" na sexta depois das 9 = a próxima sexta
        Some(d) if DIAS.contains(&d) => at_hour(&(*now + Duration::days(7)), &hora).ok(),
        _ => None,
    }
}

pub fn fmt_when(s: &str) -> R<String> {
    let (naive, _) = parse_iso(s)?;
    Ok(format!("{} {}", dia_semana(&naive), naive.format("%d/%m %H:%M")))
}

pub fn describe_task(t: &Tarefa) -> R<Value> {
    let mut out = json!({"id": t.id, "texto": t.texto,
        "status": if t.pendente { "PENDENTE (já venceu, cobrando)" } else { "agendada" },
        "proxima": fmt_when(&t.proximo)?});
    if let Some(r) = t.repetir_dias {
        out["repete"] = json!(format!("a cada {r} dia(s){}", if t.desde_feito { " contando de quando for feita" } else { " (agenda fixa)" }));
    }
    if e_habito(t) {
        out["habito"] = json!(true);
    }
    if privado(&t.resto) {
        out["privada"] = json!(true);
    }
    if let Some(h) = t.resto.get("cobrar_a_cada_h").and_then(Value::as_f64) {
        out["cobra_a_cada_h"] = json!(h);
    }
    if let Some(fim) = pausa_ate(t).filter(|f| *f > crate::util::agora()) {
        out["pausada_ate"] = json!(fmt_when(&iso(&fim))?);
    }
    Ok(out)
}

pub fn tasks_overview(state: &Estado) -> R<String> {
    use crate::rascunho::esc;
    let pend: Vec<&Tarefa> = state.tarefas.iter().filter(|t| t.pendente).collect();
    let mut rest: Vec<&Tarefa> = state.tarefas.iter().filter(|t| !t.pendente).collect();
    rest.sort_by(|a, b| a.proximo.cmp(&b.proximo));
    let mut lines = vec![];
    if !pend.is_empty() {
        lines.push("<b>Pendentes:</b>".to_string());
        for t in pend {
            lines.push(format!("• {}{}{}{}", esc(&t.texto), if e_habito(t) { " 📊" } else { "" }, if t.cobrancas != 0 { format!(" (cobrei {}×)", t.cobrancas) } else { String::new() }, nota_pausa(t, &crate::util::agora())));
        }
    }
    if !rest.is_empty() {
        lines.push("\n<b>Próximas:</b>".into());
        for t in rest {
            lines.push(format!(
                "• {}{} — {}{}{}",
                esc(&t.texto),
                if e_habito(t) { " 📊" } else { "" },
                fmt_when(&t.proximo)?,
                t.repetir_dias.map(|r| format!(" · a cada {r}d")).unwrap_or_default(),
                nota_pausa(t, &crate::util::agora())
            ));
        }
    }
    let s = lines.join("\n");
    Ok(if s.is_empty() { "Nenhuma tarefa. Manda tipo: <i>toda sexta 18h varrer a casa</i>".into() } else { s })
}

// ==========================================
// MEMÓRIA (puras)
// ==========================================

pub fn remember(state: &mut Estado, text: &str, now: &Dt, until: Option<String>) -> Fato {
    let f = Fato { id: state.proximo_id, texto: text.trim().to_string(), criado: iso(now), revisado: iso(now), ate: until, resto: Map::new() };
    state.proximo_id += 1;
    state.memoria.push(f.clone());
    f
}

pub fn validity_note(f: &Fato) -> String {
    match f.ate.as_deref().filter(|a| !a.is_empty()) {
        None => String::new(),
        Some(a) => {
            let c: Vec<char> = a.chars().collect();
            let fatia = |x: usize, y: usize| c.get(x..y.min(c.len())).map(|s| s.iter().collect::<String>()).unwrap_or_default();
            format!(" (até {}/{})", fatia(8, 10), fatia(5, 7))
        }
    }
}

/// Tira da memória o que passou da validade (vale até o fim do dia 'ate').
pub fn prune_expired(state: &mut Estado, now: &Dt) -> Vec<Fato> {
    let today = now.format("%Y-%m-%d").to_string();
    let vencido = |f: &Fato| f.ate.as_deref().is_some_and(|a| !a.is_empty() && a < today.as_str());
    let gone: Vec<Fato> = state.memoria.iter().filter(|f| vencido(f)).cloned().collect();
    state.memoria.retain(|f| !vencido(f));
    gone
}

pub fn memory_block(state: &Estado, dele: &str) -> String {
    if state.memoria.is_empty() {
        return "\n\nMemória permanente: vazia.".into();
    }
    format!(
        "\n\nMemória permanente (o que você sabe {dele}; [id] pra corrigir/esquecer):\n{}",
        state.memoria.iter().map(|f| format!("- [{}] {}{}{}", f.id, f.texto, validity_note(f), if privado(&f.resto) { " 🔒 privado" } else { "" }))
            .collect::<Vec<_>>().join("\n")
    )
}

/// fato ou tarefa que não aparece pro bot do parceiro (`privado: true` no `resto`)
pub fn privado(resto: &Map<String, Value>) -> bool {
    resto.get("privado").and_then(Value::as_bool).unwrap_or(false)
}

pub fn set_privado(resto: &mut Map<String, Value>, sim: bool) {
    if sim {
        resto.insert("privado".into(), Value::Bool(true));
    } else {
        resto.remove("privado");
    }
}

pub fn facts_to_review(state: &Estado, now: &Dt, every_days: f64) -> R<Vec<Fato>> {
    let limit = *now - Duration::milliseconds((every_days * 86_400_000.0) as i64);
    let mut out = vec![];
    for f in &state.memoria {
        if f.ate.as_deref().is_none_or(str::is_empty) && from_iso(&f.revisado)? <= limit {
            out.push(f.clone());
        }
    }
    Ok(out)
}

// ==========================================
// MANUTENÇÃO
// ==========================================

/// chaves de manutenção do veículo, na ordem em que aparecem (sem repetir)
fn chaves(vehicle: &Value) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    if let Some(m) = vehicle.get("manutencoes").and_then(Value::as_object) {
        for cat in m.values() {
            if let Some(o) = cat.as_object() {
                for k in o.keys() {
                    if !out.contains(k) {
                        out.push(k.clone());
                    }
                }
            }
        }
    }
    out
}

/// {veículo: [(chave, nome_display)]} com urgência >= 1, usando a régua do motoregis
pub fn overdue_maintenance(mr: &Motoregis, mconfig: &MotorConfig, today: &Dt) -> R<Vec<(String, Vec<(String, String)>)>> {
    let mut out = vec![];
    let day: NaiveDate = today.date_naive();
    for (name, json_path) in &mconfig.veiculos_index {
        let csv = csv_de(json_path);
        let data = mr.read_csv(&csv)?;
        let Some(vehicle) = mr.load_json(json_path)? else { continue };
        if data.len() <= 1 {
            continue;
        }
        let odo = mr.get_last_odometer(&csv)?;
        let mut late = vec![];
        for k in chaves(&vehicle) {
            let u = compute_urgency(&k, &vehicle, &data, odo, day)?;
            if u >= 1.0 {
                late.push((k, u));
            }
        }
        // mais urgente primeiro (sort estável, como o sorted do Python)
        late.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        if !late.is_empty() {
            out.push((name.clone(), late.into_iter().map(|(k, _)| {
                let d = display_name(&k, Some(&vehicle));
                (k, d)
            }).collect()));
        }
    }
    Ok(out)
}

pub fn maintenance_items(mr: &Motoregis, mconfig: &MotorConfig) -> R<Vec<(String, Vec<String>)>> {
    let mut out = vec![];
    for (name, json_path) in &mconfig.veiculos_index {
        let vehicle = mr.load_json(json_path)?.unwrap_or(json!({}));
        let mut ks = chaves(&vehicle);
        ks.sort();
        out.push((name.clone(), ks));
    }
    Ok(out)
}

/// Odômetro numa data em que ninguém disse a km (repetir a última leitura
/// deixava manutenção com a km de um abastecimento de dias antes). Só abastecimentos (categoria 1) contam:
/// entre dois, interpola pela km/dia do trecho; depois do último, projeta a média km/dia do histórico todo.
/// None: menos de 2 abastecimentos em dias diferentes, ou data antes do primeiro.
pub fn estimate_odometer(data: &crate::motoregis::Linhas, day: NaiveDate) -> Option<f64> {
    let mut fuel: Vec<(NaiveDate, f64)> = data
        .iter()
        .skip(1)
        .filter(|r| r.len() > 2 && r[2] == "1")
        .filter_map(|r| Some((crate::util::parse_ddmmyy(&r[0]).ok()?, crate::util::py_float(&r[1]).ok()?)))
        .collect();
    fuel.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let (first, last) = (*fuel.first()?, *fuel.last()?);
    let dias = |a: NaiveDate, b: NaiveDate| (b - a).num_days() as f64;
    if day < first.0 || dias(first.0, last.0) <= 0.0 {
        return None;
    }
    let km = if day >= last.0 {
        last.1 + (last.1 - first.1) / dias(first.0, last.0) * dias(last.0, day)
    } else {
        let a = *fuel.iter().filter(|f| f.0 <= day).next_back()?;
        let b = *fuel.iter().find(|f| f.0 > day)?;
        a.1 + (b.1 - a.1) / dias(a.0, b.0) * dias(a.0, day)
    };
    Some((km * 10.0).round() / 10.0)
}

/// km pra gravar manutenção em `day`: a estimativa (pode ficar abaixo da última leitura: manutenção de ontem
/// marcada hoje, depois de um abastecimento); sem estimativa, a última leitura. true = estimada.
pub fn maintenance_odometer(mr: &Motoregis, csv: &str, day: NaiveDate) -> R<(f64, bool)> {
    Ok(match estimate_odometer(&mr.read_csv(csv)?, day) {
        Some(km) => (km, true),
        None => (mr.get_last_odometer(csv)?, false),
    })
}

/// Mesmo formato do record_scheduled_maintenance, custo dividido igual; odômetro = `odo` (o que ele disse)
/// ou o `maintenance_odometer`. Devolve o odômetro, se foi estimado, e as linhas gravadas (pro desfazer).
pub fn record_maintenance(mr: &Motoregis, mconfig: &MotorConfig, vehicle: &str, items: &[String], cost: f64, date_str: &str, odo: Option<f64>) -> R<(f64, bool, Vec<Vec<String>>)> {
    let csv = csv_de(mconfig.json_de(vehicle)?);
    let (odo, estimado) = match odo {
        Some(o) => (o, false),
        None => maintenance_odometer(mr, &csv, crate::util::parse_ddmmyy(date_str)?)?,
    };
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| vec![date_str.into(), fmt_g(odo), "2".into(), fmt_f(cost / items.len() as f64, 2), "0.00".into(), item.clone()])
        .collect();
    for row in &rows {
        mr.append_record(&csv, row)?;
    }
    Ok((odo, estimado, rows))
}

// ==========================================
// ZONE 5: GRAVAR NO MOTORCOSTTRACKER
// ==========================================

pub const HEADER: [&str; 6] = ["data", "odometro", "categoria", "custo", "litros", "detalhes"];

pub fn save(mr: &Motoregis, d: &Rascunho, ctx: &Contexto, mconfig: &MotorConfig) -> R<String> {
    let v = ctx.v(&d.veiculo);
    if !mr.caminho(&v.csv).exists() {
        mr.append_record(&v.csv, &HEADER.map(String::from))?;
    }
    mr.append_record(&v.csv, &csv_row(d)?)?;
    // mesmo relatório que o motoregis imprime
    let odo = d.odometro.ok_or_else(|| Erro::Chave("odometro".into()))?;
    let mut out = mr.check_maintenance(&v.csv, &v.json, odo, &d.data)?;
    out.push_str(&mr.show_cost_analysis(&v.csv, &d.veiculo, mconfig)?);
    Ok(out.trim().to_string())
}

pub struct Git {
    pub code: i32,
    pub stderr: String,
}

/// subprocess.run(['git', '-C', dir, ...], capture_output=True, timeout=60)
pub fn git(dir: &Path, args: &[&str]) -> R<Git> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let fim = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if std::time::Instant::now() > fim {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Erro::Msg(format!("Command 'git {}' timed out after 60 seconds", args.join(" "))));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let out = child.wait_with_output()?;
    Ok(Git { code: out.status.code().unwrap_or(-1), stderr: String::from_utf8_lossy(&out.stderr).into_owned() })
}

fn git_on(git_sync: bool, motor_dir: &Path) -> bool {
    git_sync && motor_dir.join(".git").is_dir()
}

/// argos --veiculo: cadastra um veículo no motor_dir (cria a pasta no formato do MotorCostTracker se não
/// existir) a partir de exemplos/veiculo-<tipo>.json. Devolve o caminho do JSON, pra ajustar os intervalos.
pub fn novo_veiculo(motor_dir: &Path, tipo: &str, nome: &str, ano: i32) -> R<std::path::PathBuf> {
    let modelo = match tipo {
        "moto" => include_str!("../exemplos/veiculo-moto.json"),
        "carro" => include_str!("../exemplos/veiculo-carro.json"),
        _ => return Err(Erro::Msg(format!("tipo {tipo:?}: use moto ou carro"))),
    };
    let nome = nome.trim();
    if nome.is_empty() {
        return Err(Erro::Msg("nome vazio".into()));
    }
    let mr = Motoregis::new(motor_dir);
    std::fs::create_dir_all(motor_dir.join("veiculos"))?;
    let mut cfg: Value = mr.load_json("configs.json")?.unwrap_or_else(|| {
        json!({"veiculos_index": {}, "combustiveis": ["gasolina_comum", "gasolina_aditivada", "etanol", "diesel"],
               "categorias_menu": ["Abastecimento", "Manutenção Preventiva", "Acessórios / Upgrades", "Manutenção Corretiva",
                                   "Burocracia", "Alterar Veículo", "Alterar Data"]})
    });
    let idx = cfg.get_mut("veiculos_index").and_then(Value::as_object_mut).ok_or_else(|| Erro::Chave("'veiculos_index'".into()))?;
    if idx.contains_key(nome) {
        return Err(Erro::Msg(format!("{nome:?} já está cadastrado")));
    }
    // "Honda CG 160 Titan" → titan_22.json (última palavra que não é número)
    let f = crate::agenda::fold(nome);
    let slug: String = f.split_whitespace().filter(|w| !w.chars().all(|c| c.is_ascii_digit())).last().unwrap_or("veiculo")
        .chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let json_rel = format!("veiculos/{slug}_{:02}.json", ano.rem_euclid(100));
    if mr.caminho(&json_rel).exists() {
        return Err(Erro::Msg(format!("{json_rel} já existe")));
    }
    let mut v: Value = serde_json::from_str(modelo)?;
    v["veiculo"] = json!(nome);
    v["ano"] = json!(ano);
    std::fs::write(mr.caminho(&json_rel), serde_json::to_string_pretty(&v)? + "\n")?;
    let csv = crate::motoregis::csv_de(&json_rel);
    if !mr.caminho(&csv).exists() {
        mr.append_record(&csv, &HEADER.map(String::from))?;
    }
    idx.insert(nome.to_string(), json!(json_rel));
    std::fs::write(mr.caminho("configs.json"), serde_json::to_string_pretty(&cfg)? + "\n")?;
    Ok(mr.caminho(&json_rel))
}

pub fn git_pull(git_sync: bool, motor_dir: &Path) -> R<()> {
    if git_on(git_sync, motor_dir) {
        git(motor_dir, &["pull", "--quiet", "--rebase"])?;
    }
    Ok(())
}

pub fn git_push(git_sync: bool, motor_dir: &Path, veiculo: &str, data: &str, message: Option<&str>) -> R<String> {
    if !git_on(git_sync, motor_dir) {
        return Ok("git desligado".into());
    }
    git(motor_dir, &["add", "veiculos/"])?;
    if git(motor_dir, &["diff", "--cached", "--quiet"])?.code == 0 {
        return Ok("nada pra commitar".into());
    }
    let msg = match message.filter(|m| !m.is_empty()) {
        Some(m) => m.to_string(),
        None => format!("sync: argos {veiculo} {data}"),
    };
    git(motor_dir, &["commit", "-m", &msg])?;
    let r = git(motor_dir, &["push", "--quiet"])?;
    Ok(if r.code == 0 { "push ok".into() } else { format!("push falhou: {}", crate::util::corta(r.stderr.trim(), 200)) })
}


/// saída do check_maintenance + as contas prontas (o Haiku erra subtração): quanto passou do prazo e a última vez
pub fn annotate_maintenance(text: &str, data: &[Vec<String>], vehicle: &Value) -> String {
    use std::sync::OnceLock;
    static RE: OnceLock<(regex::Regex, regex::Regex)> = OnceLock::new();
    let (prazo, item) = RE.get_or_init(|| {
        (regex::Regex::new(r"^(\s*)([\d.]+) (km|meses|dias) atrás \(a cada (\d+) (km|meses|dias)\)$").unwrap(),
         regex::Regex::new(r"^\s*⚠️\s+(.+)$").unwrap())
    });
    let mut last = std::collections::HashMap::new();
    for row in data.iter().skip(1) {
        if row.len() > 5 && row[2] == "2" {
            if let Ok(odo) = crate::util::py_float(&row[1]) {
                last.insert(crate::motoregis::display_name(&row[5], Some(vehicle)), format!("{} com {} km", row[0], crate::util::fmt_g(odo)));
            }
        }
    }
    let mut out = Vec::new();
    for line in text.split('\n') {
        let mut line = line.to_string();
        if let Some(m) = prazo.captures(&line.clone()) {
            if m[3] == m[5] {
                let (unit, every) = (&m[3], &m[4]);
                let late = m[2].parse::<f64>().unwrap_or(0.0) - every.parse::<f64>().unwrap_or(0.0);
                let head = if late != 0.0 { format!("atraso: {} {unit}", crate::util::fmt_g(late)) } else { "atraso: venceu agora".into() };
                line = format!("{}{head} (feito há {} {unit}, prazo a cada {every} {unit})", &m[1], &m[2]);
            }
        }
        let name = item.captures(&line).map(|m| m[1].to_string());
        out.push(line);
        if let Some(n) = name {
            let when = last.get(&n).cloned().unwrap_or_else(|| "nunca registrada (conta desde o 1º registro)".into());
            out.push(format!("      última vez: {when}"));
        }
    }
    out.join("\n")
}
