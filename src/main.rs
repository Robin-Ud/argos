//! CLI do Argos (a lógica toda está na biblioteca, src/lib.rs).
use argos::*;
use bot::{data_dir, read_secret, Bot};

/// configs.json da instância, e o fuso dele vale pro processo inteiro (`util::fuso`)
fn load_config(dir: &Path) -> util::R<bot::Config> {
    let c = bot::load_config(dir)?;
    util::define_fuso(c.tz());
    Ok(c)
}
use std::path::Path;

// ==========================================
// ZONE 7: CLI
// ==========================================

/// Testa só a leitura: argos --ler bomba.jpg odo.jpg (não grava nada).
fn cli_read(paths: &[String]) -> util::R<()> {
    let config = load_config(&data_dir())?;
    let mr = motoregis::Motoregis::new(&config.motor_dir);
    let ctx = rascunho::vehicle_context(&mr, &mr.load_config()?)?;
    let claude = rede::Claude::new(rede::agente(), &read_secret(&config, "anthropic_key")?);
    let photos: Vec<Vec<u8>> = paths.iter().map(std::fs::read).collect::<Result<_, _>>()?;
    let reading = rede::read_photos(&claude, &config.modelo, &config.esforco_leitura, &photos, &ctx)?;
    println!("{}", serde_json::to_string_pretty(&reading)?);
    let d = rascunho::new_draft(&rascunho::Leitura::de_json(&reading), &ctx, &util::agora().format("%d/%m/%y").to_string(), config.km_max_sem_registro);
    let tags = regex::Regex::new("<[^>]+>").unwrap();
    println!("{}", tags.replace_all(&rascunho::render(&d, &ctx, config.tolerancia_total, config.km_max_sem_registro), ""));
    if rascunho::problems(&d, &ctx, 1.0, 1e9).is_empty() {
        println!("linha: {}", util::repr_lista(&rascunho::csv_row(&d)?));
    } else {
        println!("linha: (incompleta)");
    }
    Ok(())
}

/// argos --anp: baixa a semana nova da ANP e grava dados/postos.json (o bot roda isso sozinho toda
/// manhã, num processo à parte: o xlsx descompactado tem 12 MB). `--anp-arquivo x.xlsx` só lê e
/// imprime os postos do município, sem rede.
fn cli_anp(arquivo: Option<&str>) -> util::R<()> {
    let dir = data_dir();
    let config = load_config(&dir)?;
    let municipio = config.postos_municipio.clone().ok_or_else(|| util::Erro::Msg("postos_municipio não configurado".into()))?;
    if let Some(a) = arquivo {
        let linhas = postos::xlsx_linhas(&std::fs::read(a)?)?;
        let p = postos::postos_da_planilha(&linhas, &municipio, &config.postos_estado)?;
        println!("{}", serde_json::to_string_pretty(&p)?);
        return Ok(());
    }
    let fontes = postos::Fontes { anp: postos::ANP_LPC, nominatim: postos::NOMINATIM, pausa: std::time::Duration::from_millis(1100) };
    let r = postos::atualizar(&rede::agente(), &fontes, &municipio, &config.postos_estado, &dir.join("dados").join("postos.json"))?;
    println!("anp: {r}");
    Ok(())
}

/// argos --temporal: o que o `/temporal` responderia agora (rede real, nada vai pro Telegram)
fn cli_temporal() -> util::R<()> {
    let config = load_config(&data_dir())?;
    let geo = config.alerta_geocode.clone().ok_or_else(|| util::Erro::Msg("alerta_geocode não configurado".into()))?;
    let inmet = temporal::Inmet::new(rede::agente()).avisos(&geo);
    let local = match (config.alerta_lat, config.alerta_lon) {
        (Some(lat), Some(lon)) => clima::Clima::new(rede::agente()).extremos(lat, lon),
        _ => Ok(vec![]),
    };
    let t = temporal::texto_status(&config.alerta_nome, &inmet, &local, util::agora().naive_local(), &config.limites_alerta());
    println!("{}", regex::Regex::new("<[^>]+>").unwrap().replace_all(&t, ""));
    Ok(())
}

/// argos --agenda: próximos 7 dias
fn cli_agenda() -> util::R<()> {
    let config = load_config(&data_dir())?;
    let mut g = gcal::GoogleCalendar::new(rede::agente(), Path::new(&config.cred_dir))?;
    let now = util::agora();
    for e in g.list_events(&now, &(now + chrono::Duration::days(7)), None)? {
        let s = e.get("start").and_then(|s| s.get("dateTime").or_else(|| s.get("date"))).and_then(|v| v.as_str()).unwrap_or("");
        let s: String = s.chars().take(16).collect::<String>().replace('T', " ");
        println!("{s}  {}", e.get("summary").and_then(|v| v.as_str()).unwrap_or("(sem título)"));
    }
    Ok(())
}

/// argos --selftest [https://url-neutra]: monta o bot inteiro numa pasta temporária (credenciais
/// falsas, cópia do MotorCostTracker, git desligado), exercita leitura/relatório/tarefas/corpos de
/// requisição sem falar com Telegram/Google/Anthropic, e imprime o pico de memória (VmHWM).
fn selftest(url: Option<&str>) -> util::R<()> {
    use serde_json::json;
    let tmp = util::TempDir::new("argos_selftest.");
    let cred = tmp.0.join("cred");
    std::fs::create_dir_all(&cred)?;
    for (n, c) in [("telegram_token", "0:falso"), ("chat_id", "1"), ("anthropic_key", "sk-falsa"),
                   ("google_client.json", r#"{"installed": {"client_id": "x", "client_secret": "y", "token_uri": "https://oauth2.googleapis.com/token"}}"#),
                   ("google_token.json", r#"{"refresh_token": "z"}"#)] {
        std::fs::write(cred.join(n), c)?;
    }
    let real = load_config(&data_dir())?;
    let motor = tmp.0.join("motor");
    std::fs::create_dir_all(motor.join("veiculos"))?;
    if real.tem_veiculos {
        std::fs::copy(Path::new(&real.motor_dir).join("configs.json"), motor.join("configs.json"))?;
        for e in std::fs::read_dir(Path::new(&real.motor_dir).join("veiculos"))? {
            let e = e?;
            std::fs::copy(e.path(), motor.join("veiculos").join(e.file_name()))?;
        }
    } else {
        // instância sem veículo: um de mentira na cópia, só pra medir o fluxo das fotos
        tarefas::novo_veiculo(&motor, "carro", "Carro de Teste", 2020)?;
    }
    let mut config = real.clone();
    config.cred_dir = cred.to_string_lossy().into();
    config.motor_dir = motor.to_string_lossy().into();
    config.git_sync = false;
    println!("início:          {}", util::memoria());

    let mut bot = Bot::new(config.clone(), &tmp.0)?;
    println!("bot montado:     {}", util::memoria());

    // leitura + rascunho + gravação + relatório (na cópia)
    let mconfig = bot.mr.load_config()?;
    let ctx = rascunho::vehicle_context(&bot.mr, &mconfig)?;
    let last = ctx.veiculos[0].last_odo.unwrap_or(0.0);
    let leitura = rascunho::Leitura { total_reais: Some(50.0), litros: Some(8.0), preco_litro: Some(6.25), combustivel: Some("gasolina_comum".into()),
                                      odometro_km: Some(last + 200.0), veiculo: Some(ctx.veiculos[0].nome.clone()), observacao: String::new(),
                                      cnpj_posto: None };
    let d = rascunho::new_draft(&leitura, &ctx, &util::agora().format("%d/%m/%y").to_string(), config.km_max_sem_registro);
    let _ = rascunho::render(&d, &ctx, config.tolerancia_total, config.km_max_sem_registro);
    let _ = rascunho::keyboard(&d, &ctx, config.tolerancia_total, config.km_max_sem_registro);
    let rel = tarefas::save(&bot.mr, &d, &ctx, &mconfig)?;
    println!("relatório ({} linhas)", rel.lines().count());
    let _ = tarefas::overdue_maintenance(&bot.mr, &mconfig, &util::agora())?;

    // corpo de leitura com 2 "fotos" de 400 KB (o pico real do fluxo de abastecimento)
    let fotos = vec![vec![0x5au8; 400 << 10]; 2];
    let corpo = rede::photos_body(&config.modelo, &config.esforco_leitura, &fotos, &ctx).to_string();
    println!("corpo das fotos: {} KB", corpo.len() >> 10);
    drop(corpo);
    drop(fotos);

    // corpo da agenda com histórico de 20 voltas
    let msgs: Vec<serde_json::Value> = (0..20).map(|i| json!({"role": "user", "content": format!("mensagem {i} {}", "x".repeat(500))})).collect();
    let corpo = json!({"model": config.modelo_agenda, "system": format!("{}{}", agenda::agenda_system(&config), bot.system_extra()?),
                       "tools": agenda::agenda_tools(&config), "messages": msgs}).to_string();
    println!("corpo da agenda: {} KB", corpo.len() >> 10);
    drop(corpo);

    // tarefas: cria, venceu, cobra, salva
    let t = tarefas::new_task(&mut bot.state, "teste", "2026-01-01T09:00", Some(7), false, "09:00")?;
    let _ = tarefas::task_tick(&mut bot.state.tarefas[0], &util::agora(), &config.cobranca())?;
    tarefas::task_done(&mut bot.state, t.id, &util::agora())?;
    tarefas::save_state(&bot.state, &bot.state_path)?;
    let _ = tarefas::tasks_overview(&bot.state)?;
    let mut d2 = d.clone();
    rascunho::apply_correction(&mut d2, "km 3.810, litros 8,52 total R$ 53,40", &ctx.fuel_types, &ctx.nomes())?;

    if let Some(u) = url {
        // handshake TLS de verdade (rustls) contra um host neutro, pra medir a conexão viva
        let r = rede::agente().get(u).timeout(std::time::Duration::from_secs(20)).call();
        println!("GET {u}: {}", match r { Ok(r) => r.status().to_string(), Err(e) => e.to_string() });
    }
    util::devolve_memoria();
    println!("fim:             {}", util::memoria());
    Ok(())
}

/// argos --checar-config: carrega e valida o configs.json e diz o que está ligado (rodar antes de todo restart)
fn cli_checar_config() -> util::R<()> {
    let dir = data_dir();
    let c = load_config(&dir)?;
    let cred = Path::new(&c.cred_dir);
    let tem = |n: &str| if cred.join(n).exists() { "ok" } else { "FALTA" };
    let u = &c.usuario;
    println!("pasta:       {}", dir.display());
    println!("usuário:     {} ({}), bot: {}", u.nome, if u.ela { "ela" } else { "ele" }, u.assistente);
    println!("credenciais: {} · telegram_token {} · anthropic_key {} · chat_id {}", cred.display(), tem("telegram_token"), tem("anthropic_key"),
             if cred.join("chat_id").exists() { "ok" } else { "falta (sobe em modo pareamento)" });
    println!("google:      {}", if c.tem_google { "ligado" } else { "desligado (argos --google-login)" });
    let mr = motoregis::Motoregis::new(&c.motor_dir);
    let veiculos: Vec<String> = mr.load_config().map(|m| m.veiculos_index.into_iter().map(|(n, _)| n).collect()).unwrap_or_default();
    println!("veículos:    {} · {}", c.motor_dir, if veiculos.is_empty() { "nenhum (argos --veiculo)".into() } else { veiculos.join(", ") });
    for r in ["chuva", "temporal", "postos"] {
        println!("{r:<12} {}", if c.ligado(r) { "ligado" } else { "desligado" });
    }
    if let Some(p) = &c.agenda_parceiro {
        println!("parceiro:    {} ({}){}", p.nome, p.calendar_id, if p.dir.is_some() { " · recados" } else { "" });
    }
    Ok(())
}

fn cli_init() -> util::R<()> {
    use std::io::IsTerminal;
    let dir = match std::env::var("ARGOS_DIR") {
        Ok(d) => std::path::PathBuf::from(d),
        Err(_) => std::env::current_dir()?,
    };
    std::fs::create_dir_all(&dir)?;
    let terminal = std::io::stdin().is_terminal();
    instalar::init_em(&dir, &mut std::io::stdin().lock(), terminal)
}

/// argos --veiculo moto|carro "Nome" ano: cadastra um veículo (cria o motor_dir se precisar)
fn cli_veiculo(tipo: &str, nome: &str, ano: &str) -> util::R<()> {
    let config = load_config(&data_dir())?;
    let ano: i32 = ano.parse().map_err(|_| util::Erro::Msg(format!("ano {ano:?} não é número")))?;
    let json = tarefas::novo_veiculo(Path::new(&config.motor_dir), tipo, nome, ano)?;
    println!("✅ {nome} cadastrado. Intervalos de manutenção genéricos em {}: confira com o manual.", json.display());
    println!("O bot em execução pega sozinho no próximo ciclo; o primeiro abastecimento por foto já entra nele.");
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let r = match args.get(1).map(String::as_str) {
        Some("--ler") if args.len() > 2 => cli_read(&args[2..]),
        Some("--google-login") if args.len() == 2 => {
            load_config(&data_dir()).and_then(|c| gcal::google_login(&rede::agente(), Path::new(&c.cred_dir), &c.google_escopos_extra))
        }
        Some("--agenda") if args.len() == 2 => cli_agenda(),
        Some("--temporal") if args.len() == 2 => cli_temporal(),
        Some("--anp") if args.len() == 2 => cli_anp(None),
        Some("--anp-arquivo") if args.len() == 3 => cli_anp(Some(&args[2])),
        Some("--selftest") => selftest(args.get(2).map(String::as_str)),
        Some("--checar-config") if args.len() == 2 => cli_checar_config(),
        Some("--init") if args.len() == 2 => cli_init(),
        Some("--veiculo") if args.len() == 5 => cli_veiculo(&args[2], &args[3], &args[4]),
        _ => {
            let dir = data_dir();
            match load_config(&dir).and_then(|c| Bot::new(c, &dir)) {
                Ok(mut b) => b.run(),
                Err(e) => Err(e),
            }
        }
    };
    if let Err(e) = r {
        eprintln!("❌ {e}");
        std::process::exit(1);
    }
}
