//! Rotina: os horários em que ele costuma aparecer no chat, por dia da semana (a rotina é semanal, não diária).
//! Toda mensagem/botão do dono vira uma linha em `dados/atividade.csv`; `habitos` acha os horários de costume
//! e as cobranças das tarefas caem neles (`tarefas::encaixe`), um pouco antes.
use crate::util::{dia_semana, from_iso, iso, Dt, R};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use std::path::Path;

/// semanas guardadas no atividade.csv
pub const SEMANAS: i64 = 8;
/// ocorrências do mesmo dia da semana que contam (as mais recentes com dado)
const OLHA: usize = 6;
/// mínimo de ocorrências com dado pra aprender aquele dia
const MINIMO: usize = 3;
/// silêncio que separa uma sessão da outra
const SESSAO_MIN: i64 = 30;
/// começos de sessão a até isto um do outro são o mesmo costume…
const VIZINHO_MIN: u32 = 45;
/// …desde que o grupo não passe disto do primeiro ao último
const LARGURA_MIN: u32 = 90;

/// uma linha por interação (`em` ISO), anexada
pub fn registrar(path: &Path, em: &Dt) -> R<()> {
    use std::io::Write;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", iso(em))?;
    Ok(())
}

/// interações gravadas (linha quebrada é pulada); sem arquivo = nenhuma
pub fn ler(path: &Path) -> Vec<NaiveDateTime> {
    let Ok(s) = std::fs::read_to_string(path) else { return vec![] };
    s.lines().filter_map(|l| from_iso(l.trim()).ok()).map(|d| d.naive_local()).collect()
}

/// regrava só as últimas `SEMANAS` semanas
pub fn podar(path: &Path, now: &Dt) -> R<()> {
    let Ok(s) = std::fs::read_to_string(path) else { return Ok(()) };
    let corte = now.naive_local() - Duration::weeks(SEMANAS);
    let fica: Vec<&str> = s.lines().filter(|l| from_iso(l.trim()).is_ok_and(|d| d.naive_local() >= corte)).collect();
    if fica.len() == s.lines().count() {
        return Ok(());
    }
    let tmp = path.with_extension("csv.tmp");
    std::fs::write(&tmp, fica.iter().map(|l| format!("{l}\n")).collect::<String>())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// começo de cada sessão (interações a menos de 30 min da anterior são a mesma sessão)
pub fn sessoes(evs: &[NaiveDateTime]) -> Vec<NaiveDateTime> {
    let mut evs = evs.to_vec();
    evs.sort();
    let mut out = vec![];
    let mut ultima: Option<NaiveDateTime> = None;
    for e in evs {
        if ultima.is_none_or(|u| e - u > Duration::minutes(SESSAO_MIN)) {
            out.push(e);
        }
        ultima = Some(e);
    }
    out
}

/// Horários de costume de cada dia da semana (0 = segunda), já `antecedencia_min` antes. Por dia da semana:
/// as últimas 6 ocorrências dele antes de `hoje` que têm interação (precisa de 3); começos de sessão vizinhos
/// viram um grupo; grupo que aparece em metade dessas ocorrências (no mínimo 2) é costume, na mediana.
pub fn habitos(evs: &[NaiveDateTime], hoje: NaiveDate, antecedencia_min: i64) -> [Vec<NaiveTime>; 7] {
    let ss: Vec<NaiveDateTime> = sessoes(evs).into_iter().filter(|s| s.date() < hoje).collect();
    std::array::from_fn(|w| {
        let mut dias: Vec<NaiveDate> = ss.iter().map(|s| s.date()).filter(|d| d.weekday().num_days_from_monday() as usize == w).collect();
        dias.dedup();
        let dias: Vec<NaiveDate> = dias.into_iter().rev().take(OLHA).collect();
        if dias.len() < MINIMO {
            return vec![];
        }
        let mut pontos: Vec<(u32, NaiveDate)> = ss.iter().filter(|s| dias.contains(&s.date())).map(|s| (s.hour() * 60 + s.minute(), s.date())).collect();
        pontos.sort();
        let mut grupos: Vec<Vec<(u32, NaiveDate)>> = vec![];
        for p in pontos {
            match grupos.last_mut() {
                Some(g) if p.0 - g.last().unwrap().0 <= VIZINHO_MIN && p.0 - g[0].0 <= LARGURA_MIN => g.push(p),
                _ => grupos.push(vec![p]),
            }
        }
        let precisa = dias.len().div_ceil(2).max(2);
        let mut out = vec![];
        for g in grupos {
            // um ponto por dia: o primeiro daquele dia no grupo
            let mut por_dia: Vec<(NaiveDate, u32)> = vec![];
            for (m, d) in &g {
                if !por_dia.iter().any(|(x, _)| x == d) {
                    por_dia.push((*d, *m));
                }
            }
            if por_dia.len() < precisa {
                continue;
            }
            let mut ms: Vec<u32> = por_dia.iter().map(|(_, m)| *m).collect();
            ms.sort();
            let mediana = ms[(ms.len() - 1) / 2] as i64;
            let m = (mediana - antecedencia_min).max(0) as u32;
            out.push(NaiveTime::from_hms_opt(m / 60, m % 60, 0).unwrap());
        }
        out
    })
}

/// fixos do configs.json + aprendidos; fixo a menos de 1 h de um aprendido sai (o aprendido é o de verdade)
pub fn mistura(fixos: &[NaiveTime], aprendidos: &[Vec<NaiveTime>; 7]) -> [Vec<NaiveTime>; 7] {
    std::array::from_fn(|w| {
        let a = &aprendidos[w];
        let mut v: Vec<NaiveTime> = fixos.iter().filter(|f| !a.iter().any(|x| (**f - *x).num_minutes().abs() < 60)).copied().chain(a.iter().copied()).collect();
        v.sort();
        v.dedup();
        v
    })
}

/// resposta do /rotina
pub fn texto_rotina(fixos: &[NaiveTime], aprendidos: &[Vec<NaiveTime>; 7], antecedencia_min: i64) -> String {
    let base = NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(); // uma segunda
    let todos = mistura(fixos, aprendidos);
    let mut linhas = vec!["🗓 <b>Quando eu cobro</b> (horário em que você costuma aparecer, menos ".to_string() + &antecedencia_min.to_string() + " min):"];
    for (w, hs) in todos.iter().enumerate() {
        let dia = dia_semana(&(base + Duration::days(w as i64))).chars().take(3).collect::<String>();
        let hs = if hs.is_empty() {
            "—".to_string()
        } else {
            hs.iter().map(|h| format!("{}{}", h.format("%H:%M"), if aprendidos[w].contains(h) { "" } else { "*" })).collect::<Vec<_>>().join(", ")
        };
        linhas.push(format!("{dia} {hs}"));
    }
    if aprendidos.iter().all(Vec::is_empty) {
        linhas.push(format!("\nAinda aprendendo: preciso de {MINIMO} semanas de conversa pra achar seu costume de cada dia."));
    }
    if !fixos.is_empty() {
        linhas.push("* fixo do configs.json (horarios_cobranca)".into());
    }
    linhas.push("Sem horário no dia: cobro no fim do intervalo, dentro das janelas.".into());
    linhas.join("\n")
}
