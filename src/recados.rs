//! Recados entre instâncias (duas pessoas, mesma máquina): quem manda anexa uma linha em
//! `<pasta do outro>/dados/recados.jsonl`; o `tick` de quem recebe lê e entrega. Tarefa entra na hora
//! (vence no `quando`); mensagem espera o `quando` na caixa.
use crate::util::{Dt, R};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Recado {
    /// nome de quem mandou
    pub de: String,
    /// quem mandou é "ela" (pro "da Ana"/"do Pedro")
    #[serde(default)]
    pub de_ela: bool,
    pub texto: String,
    /// ISO com fuso; None = agora
    #[serde(default)]
    pub quando: Option<String>,
    /// true = vira tarefa que cobra; false = só mensagem
    pub tarefa: bool,
    pub em: String,
}

impl Recado {
    /// "do Pedro" / "da Ana"
    pub fn do_remetente(&self) -> String {
        format!("{} {}", if self.de_ela { "da" } else { "do" }, self.de)
    }
}

const CAIXA: &str = "recados.jsonl";
/// a caixa vira este arquivo enquanto é lida: o que chegar no meio vai pra uma caixa nova
const LENDO: &str = "recados.lendo";

/// anexa o recado na caixa de `dir_outro` (a pasta ARGOS_DIR da outra instância)
pub fn enviar(dir_outro: &Path, r: &Recado) -> R<()> {
    let dados = dir_outro.join("dados");
    if !dados.is_dir() {
        return Err(crate::util::Erro::Msg(format!("{}: pasta do outro bot não existe", dados.display())));
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(dados.join(CAIXA))?;
    f.write_all(format!("{}\n", serde_json::to_string(r)?).as_bytes())?;
    Ok(())
}

/// tira da caixa de `dados` o que já é pra entregar: toda tarefa e as mensagens com `quando` vencido.
/// O resto volta pra caixa. Linha estragada só vai pro log.
pub fn receber(dados: &Path, now: &Dt) -> R<Vec<Recado>> {
    let (caixa, lendo) = (dados.join(CAIXA), dados.join(LENDO));
    // sobrou de uma volta que caiu no meio: lê de novo antes
    if !lendo.exists() {
        if !caixa.exists() {
            return Ok(vec![]);
        }
        std::fs::rename(&caixa, &lendo)?;
    }
    let mut agora_ = vec![];
    let mut depois = vec![];
    for linha in std::fs::read_to_string(&lendo)?.lines().filter(|l| !l.trim().is_empty()) {
        let r: Recado = match serde_json::from_str(linha) {
            Ok(r) => r,
            Err(e) => {
                println!("recado estragado ({e}): {linha}");
                continue;
            }
        };
        let vence = match &r.quando {
            Some(q) => crate::util::from_iso(q).map(|q| q <= *now).unwrap_or(true),
            None => true,
        };
        if r.tarefa || vence {
            agora_.push(r);
        } else {
            depois.push(r);
        }
    }
    for r in &depois {
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&caixa)?;
        f.write_all(format!("{}\n", serde_json::to_string(r)?).as_bytes())?;
    }
    std::fs::remove_file(&lendo)?;
    Ok(agora_)
}
