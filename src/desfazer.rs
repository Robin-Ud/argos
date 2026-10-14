//! Desfazer: cada gravação do bot guarda em `estado.desfazer` como reverter (mesmo formato JSON do argos.py).
//! Ops (revertidas de trás pra frente): csv {csv, rows, veiculo} · evento_criado {id} · evento_alterado {id, antes}
//! · evento_apagado {evento} · tarefa_criada {id} · tarefa_antes {tarefa} · memoria_criada {id} · memoria_antes {fato}
//! · tracker {rows} (linhas do dados/tracker.csv, ao lado do estado.json; só Rust).
//! Pra uma op nova: construtor aqui + braço no `match` de `undo`.
use crate::gcal::GoogleCalendar;
use crate::motoregis::{csv_line, Motoregis};
use crate::tarefas::{git_pull, git_push, save_state, Estado, Fato, Tarefa};
use crate::util::{corta, iso, str_num, Dt, Erro, Origem, R};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::Path;

pub const UNDO_KEEP: usize = 20;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Entrada {
    pub id: i64,
    pub desc: String,
    /// ops cruas (JSON): o que o Python gravou volta igual
    pub ops: Vec<Value>,
    pub em: String,
    pub desfeito: bool,
    #[serde(flatten)]
    pub resto: Map<String, Value>,
}

/// Guarda como desfazer um grupo de ações (as ops são revertidas de trás pra frente).
pub fn push_undo(state: &mut Estado, desc: &str, ops: Vec<Value>, now: &Dt) -> Option<i64> {
    if ops.is_empty() {
        return None;
    }
    let id = state.proximo_id;
    state.desfazer.push(Entrada { id, desc: desc.to_string(), ops, em: iso(now), desfeito: false, resto: Map::new() });
    state.proximo_id += 1;
    let n = state.desfazer.len();
    if n > UNDO_KEEP {
        state.desfazer.drain(..n - UNDO_KEEP);
    }
    Some(id)
}

pub fn undo_button(undo_id: Option<i64>) -> Option<Value> {
    undo_id.map(|id| json!([[{"text": "↩️ desfazer", "callback_data": format!("und:{id}")}]]))
}

// --- construtores das ops ---

/// linha do CSV como o Python guarda: a categoria (coluna 2) é inteiro
pub fn row_value(row: &[String]) -> Value {
    Value::Array(
        row.iter()
            .enumerate()
            .map(|(i, c)| match crate::util::py_int(c) {
                Ok(n) if i == 2 => json!(n),
                _ => json!(c),
            })
            .collect(),
    )
}

pub fn op_csv(csv: &str, rows: &[Vec<String>], veiculo: &str) -> Value {
    json!({"op": "csv", "csv": csv, "rows": rows.iter().map(|r| row_value(r)).collect::<Vec<_>>(), "veiculo": veiculo})
}

pub fn op_tarefa_antes(t: &Tarefa) -> Value {
    json!({"op": "tarefa_antes", "tarefa": serde_json::to_value(t).unwrap_or(Value::Null)})
}

/// conclusão de hábito gravada no tracker.csv (células todas texto: a 3ª coluna é o nome da tarefa, não categoria)
pub fn op_tracker(rows: &[Vec<String>]) -> Value {
    json!({"op": "tracker", "rows": rows})
}

pub fn op_memoria_antes(f: &Fato) -> Value {
    json!({"op": "memoria_antes", "fato": serde_json::to_value(f).unwrap_or(Value::Null)})
}

/// célula como o csv.writer escreve (str() do Python; None vira vazio)
fn celula(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        v => str_num(v),
    }
}

/// readlines() com newline='': quebra em \n, \r\n e \r, mantendo o fim de linha
fn linhas_cruas(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut atual = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        atual.push(c);
        if c == '\n' || (c == '\r' && it.peek() != Some(&'\n')) {
            out.push(std::mem::take(&mut atual));
        }
    }
    if !atual.is_empty() {
        out.push(atual);
    }
    out
}

/// Tira do CSV as linhas gravadas (a última ocorrência de cada). Se alguma sumiu, não mexe em nada.
pub fn remove_rows(path: &Path, rows: &[Value]) -> R<bool> {
    let texto = std::fs::read_to_string(path).map_err(|e| Erro::Msg(format!("{}: {e}", path.display())))?;
    let mut lines = linhas_cruas(&texto);
    for row in rows.iter().rev() {
        let cells: Vec<String> = row.as_array().ok_or_else(|| Erro::Chave("rows".into()))?.iter().map(celula).collect();
        let target = csv_line(&cells);
        let Some(idx) = (0..lines.len()).rev().find(|&i| lines[i].trim_end_matches(['\r', '\n']) == target) else {
            return Ok(false);
        };
        lines.remove(idx);
    }
    std::fs::write(path, lines.concat())?;
    Ok(true)
}

/// troca o item de mesmo id pelo snapshot, ou acrescenta no fim
pub fn restore_by_id<T: Serialize + serde::de::DeserializeOwned>(items: &mut Vec<T>, snapshot: &Value) -> R<()> {
    let id = snapshot.get("id").cloned().unwrap_or(Value::Null);
    let novo: T = serde_json::from_value(snapshot.clone())?;
    for item in items.iter_mut() {
        if serde_json::to_value(&*item).ok().and_then(|v| v.get("id").cloned()) == Some(id.clone()) {
            *item = novo;
            return Ok(());
        }
    }
    items.push(novo);
    Ok(())
}

/// self.agenda.gcal (None no FakeUndoBot dos testes)
fn cal<'a>(g: &'a mut Option<&mut GoogleCalendar>) -> R<&'a mut GoogleCalendar> {
    g.as_deref_mut().ok_or_else(|| Erro::Chave("'NoneType' object has no attribute 'gcal'".into()))
}

fn campo<'a>(op: &'a Value, k: &str) -> R<&'a Value> {
    op.get(k).ok_or_else(|| Erro::Chave(format!("'{k}'")))
}

/// Reverte um grupo de ações (o último pendente, se undo_id for None). É o Bot.undo do Python:
/// salva o estado em `state_path` quando dá certo.
pub fn undo(
    state: &mut Estado,
    mr: &Motoregis,
    git_sync: bool,
    motor_dir: &Path,
    mut gcal: Option<&mut GoogleCalendar>,
    state_path: &Path,
    undo_id: Option<i64>,
) -> R<String> {
    let pending: Vec<usize> = (0..state.desfazer.len()).filter(|&i| !state.desfazer[i].desfeito).collect();
    let pos = match undo_id.filter(|id| *id != 0) {
        Some(id) => pending.iter().copied().find(|&i| state.desfazer[i].id == id),
        None => pending.last().copied(),
    };
    let Some(pos) = pos else { return Ok("Nada pra desfazer (ou já foi desfeito).".into()) };
    let entry = state.desfazer[pos].clone();
    let tem_git = entry.ops.iter().any(|op| op.get("op").and_then(Value::as_str) == Some("csv"));
    if tem_git {
        git_pull(git_sync, motor_dir)?;
    }
    for op in entry.ops.iter().rev() {
        let id = || campo(op, "id");
        match op.get("op").and_then(Value::as_str).unwrap_or("") {
            "csv" => {
                let csv = str_num(campo(op, "csv")?);
                let rows = campo(op, "rows")?.as_array().cloned().unwrap_or_default();
                if !remove_rows(&mr.caminho(&csv), &rows)? {
                    return Ok(format!("Não achei mais a linha no CSV ({}), não mexi em nada.", entry.desc));
                }
            }
            "evento_criado" => match cal(&mut gcal)?.delete(&str_num(id()?)) {
                Err(Erro::Http { code: 404 | 410, origem: Origem::Google, .. }) | Ok(()) => {}
                Err(e) => return Err(e),
            },
            "evento_alterado" => {
                cal(&mut gcal)?.patch(&str_num(id()?), campo(op, "antes")?)?;
            }
            "evento_apagado" => {
                cal(&mut gcal)?.create(campo(op, "evento")?)?;
            }
            "tarefa_criada" => {
                let alvo = id()?.clone();
                state.tarefas.retain(|t| json!(t.id) != alvo);
            }
            "tarefa_antes" => restore_by_id(&mut state.tarefas, campo(op, "tarefa")?)?,
            "memoria_criada" => {
                let alvo = id()?.clone();
                state.memoria.retain(|f| json!(f.id) != alvo);
            }
            "memoria_antes" => restore_by_id(&mut state.memoria, campo(op, "fato")?)?,
            "tracker" => {
                let rows = campo(op, "rows")?.as_array().cloned().unwrap_or_default();
                if !remove_rows(&state_path.with_file_name("tracker.csv"), &rows)? {
                    return Ok(format!("Não achei mais a linha no tracker.csv ({}), não mexi em nada.", entry.desc));
                }
            }
            _ => {}
        }
    }
    let mut sync = String::new();
    if tem_git {
        let msg = corta(&format!("desfaz: {}", entry.desc), 72);
        sync = format!(" · {}", git_push(git_sync, motor_dir, "", "", Some(&msg))?);
    }
    state.desfazer[pos].desfeito = true;
    save_state(state, state_path)?;
    println!("desfeito: {}{sync}", entry.desc);
    Ok(format!("↩️ desfeito: {}{sync}", entry.desc))
}
