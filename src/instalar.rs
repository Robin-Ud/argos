//! `argos --init`: monta a pasta de uma instância nova.
use std::path::Path;

/// argos --init: pergunta quem usa e as duas chaves (BotFather e Anthropic) e monta a pasta do bot:
/// configs.json mínimo, credenciais/ (0600) e dados/. Pasta que já tem configs.json não é tocada.
pub fn init_em(dir: &Path, entrada: &mut dyn std::io::BufRead, terminal: bool) -> crate::util::R<()> {
    use std::io::Write;
    let cfg = dir.join("configs.json");
    if cfg.exists() {
        return Err(crate::util::Erro::Msg(format!("{} já existe: edite à mão (ou use outra pasta com ARGOS_DIR)", cfg.display())));
    }
    let mut pergunta = |texto: &str, padrao: &str, segredo: bool| -> crate::util::R<String> {
        print!("{texto}{}: ", if padrao.is_empty() { String::new() } else { format!(" [{padrao}]") });
        std::io::stdout().flush()?;
        // chave não aparece na tela (stty, se for terminal)
        let eco = |liga: bool| if terminal && segredo {
            let _ = std::process::Command::new("stty").arg(if liga { "echo" } else { "-echo" }).stdin(std::process::Stdio::inherit()).status();
        };
        eco(false);
        let mut l = String::new();
        let lido = entrada.read_line(&mut l);
        eco(true);
        if terminal && segredo {
            println!();
        }
        if lido? == 0 {
            return Err(crate::util::Erro::Msg("entrada acabou antes das respostas".into()));
        }
        let l = l.trim();
        Ok(if l.is_empty() { padrao.to_string() } else { l.to_string() })
    };
    let nome = pergunta("Seu nome (como o bot te chama)", "", false)?;
    if nome.is_empty() {
        return Err(crate::util::Erro::Msg("nome vazio".into()));
    }
    let ela = pergunta("Pronome: ela ou ele", "ele", false)?.to_lowercase().starts_with("ela");
    let sobre = pergunta("Uma linha sobre você, opcional (ex.: estudante de biologia em Campinas)", "", false)?;
    let cidade = pergunta("Sua cidade, opcional (vai na previsão de chuva)", "", false)?;
    let assistente = pergunta("Nome do bot, com artigo", "o Argos", false)?;
    let token = pergunta("Token do bot (o @BotFather dá: /newbot)", "", true)?;
    if !regex::Regex::new(r"^\d+:[\w-]{20,}$").unwrap().is_match(&token) {
        return Err(crate::util::Erro::Msg("isso não parece token do BotFather (número:letras)".into()));
    }
    let chave = pergunta("Chave da API da Anthropic (console.anthropic.com → API Keys)", "", true)?;
    if !chave.starts_with("sk-ant-") {
        return Err(crate::util::Erro::Msg("a chave da Anthropic começa com sk-ant-".into()));
    }
    let cred = dir.join("credenciais");
    std::fs::create_dir_all(&cred)?;
    std::fs::create_dir_all(dir.join("dados"))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cred, std::fs::Permissions::from_mode(0o700))?;
    }
    crate::util::grava_secreto(&cred.join("telegram_token"), &token)?;
    crate::util::grava_secreto(&cred.join("anthropic_key"), &chave)?;
    let mut usuario = serde_json::json!({"nome": nome, "ela": ela});
    for (k, v) in [("sobre", &sobre), ("cidade", &cidade)] {
        if !v.is_empty() {
            usuario[k] = serde_json::json!(v);
        }
    }
    if assistente != "o Argos" {
        usuario["assistente"] = serde_json::json!(assistente);
    }
    std::fs::write(&cfg, serde_json::to_string_pretty(&serde_json::json!({"usuario": usuario}))? + "\n")?;
    crate::bot::load_config(dir)?; // o que foi gravado tem que subir
    println!("\n✅ Pronto em {}.\n", dir.display());
    println!("Próximos passos:");
    println!("  1. Suba o bot (ARGOS_DIR={} argos). O log mostra um código de 6 dígitos: mande ele pro bot no Telegram, no privado.", dir.display());
    println!("  2. Opcional, veículo: argos --veiculo moto|carro \"Nome\" ano (abastecimento por foto e manutenção).");
    println!("  3. Opcional, Google Agenda: ponha o google_client.json em {} e rode argos --google-login.", cred.display());
    println!("  4. Opcional, chuva/temporal/postos: chaves no configs.json (veja exemplos/configs.exemplo.json).");
    Ok(())
}
