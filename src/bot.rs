//! ZONE 1 (config/credenciais) + ZONE 6 (classe Bot: long polling, fotos, botões, avisos).
use crate::agenda::{event_line, Agenda};
use crate::desfazer::{op_csv, op_memoria_antes, op_tarefa_antes, push_undo, undo_button};
use crate::gcal::GoogleCalendar;
use crate::motoregis::{display_name, Motoregis};
use crate::rascunho::{apply_correction, botao, csv_row, esc, keyboard, new_draft, render, suggest_gap, vehicle_context, Contexto, Leitura, Rascunho};
use crate::rede::{agente, read_photos, Claude, Telegram};
use crate::tarefas::{
    facts_to_review, git_pull, git_push, load_state, maintenance_items, prune_expired, validity_note, overdue_maintenance, record_maintenance, save, save_state, task_done, task_snooze, task_tick, adiar_por_compromisso,
    tasks_overview, Aviso, CfgCobranca, Estado, Tarefa,
};
use crate::util::{agora, corta, de_timestamp, devolve_memoria, dumps, fmt_g, from_iso, iso, repr_lista, Erro, R};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Instant;

// ==========================================
// ZONE 1: CONFIG E CREDENCIAIS
// ==========================================

/// Toda chave é opcional, menos `usuario` (`load_config_de` cobra): o padrão é o `Default` abaixo.
/// Os recursos de fora (chuva, temporal, postos) só ligam com a chave deles.
#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// pasta no formato do MotorCostTracker (configs.json + veiculos/); padrão `<ARGOS_DIR>/dados/motor`
    pub motor_dir: String,
    /// telegram_token, chat_id, anthropic_key, google_*.json; padrão `<ARGOS_DIR>/credenciais`
    pub cred_dir: String,
    pub modelo: String,
    pub modelo_agenda: String,
    pub esforco_leitura: String,
    pub esforco_agenda: String,
    pub agenda_reset_min: f64,
    pub hora_manutencao: String,
    pub manutencao_repetir_dias: f64,
    pub hora_padrao_tarefa: String,
    pub cobrar_a_cada_h: f64,
    pub janela_cobranca: (String, String),
    pub memoria_revisao_dias: f64,
    pub hora_revisao_memoria: String,
    /// git pull/push no motor_dir (só se ele for um repositório)
    pub git_sync: bool,
    pub espera_fotos_s: f64,
    pub tolerancia_total: f64,
    pub km_max_sem_registro: f64,
    /// aviso de chuva (clima.rs): sem lat/lon fica desligado
    pub clima_lat: Option<f64>,
    pub clima_lon: Option<f64>,
    pub chuva_limiar: f64,
    pub hora_chuva_noite: String,
    pub hora_chuva_manha: String,
    /// postos (postos.rs): sem município fica desligado; com ele, `postos_estado` é obrigatório
    pub postos_municipio: Option<String>,
    pub postos_estado: String,
    pub postos_raio_km: f64,
    pub hora_postos: String,
    /// alerta de temporal (temporal.rs): sem `alerta_geocode` fica desligado; com ele, `alerta_nome` é obrigatório
    pub alerta_nome: String,
    /// código IBGE do município (o INMET filtra por ele)
    pub alerta_geocode: Option<String>,
    /// ponto da previsão local (sem lat/lon só o INMET)
    pub alerta_lat: Option<f64>,
    pub alerta_lon: Option<f64>,
    pub alerta_rajada_kmh: f64,
    pub alerta_chuva_mm_h: f64,
    pub alerta_chuva_mm_dia: f64,
    /// sem aviso nessa janela, só vermelho (o resto sai no fim dela)
    pub alerta_silencio: (String, String),
    pub alerta_intervalo_min: f64,
    /// fuso IANA ("America/Manaus"); só os do Brasil e Lisboa vão no binário (.cargo/config.toml)
    pub fuso: String,
    /// quem usa esta instância (nome, pronome, "sobre", cidade, nome do bot): obrigatório
    pub usuario: crate::agenda::Usuario,
    /// agenda que esta instância só consulta (e o bot dela, com `dir`)
    pub agenda_parceiro: Option<crate::agenda::Parceiro>,
    /// parágrafo a mais no system prompt, no meio das regras (antes de CHUVA); aceita os marcadores
    /// `{ele}`, `{dele}`, `{do_nome}`… do `personaliza`
    pub prompt_extra: Option<String>,
    /// escopos a mais no `--google-login` (ex. drive.file pra quem usa o Drive com o mesmo token)
    pub google_escopos_extra: Vec<String>,
    /// aviso automático de chuva (21h/6h30); false deixa só o /chuva e a previsao_chuva
    pub chuva_aviso: bool,
    /// aviso das 06:00 só destes veículos (pedaço do nome, sem acento); sem a chave, todos
    pub manutencao_veiculos: Option<Vec<String>>,
    /// postos.json de outra instância (ranking, coordenadas e apelidos em comum); com ele, esta não baixa a ANP
    pub postos_json: Option<String>,
    /// detectado (`detectar`), não vem do configs.json: tem veículo no motor_dir
    #[serde(skip)]
    pub tem_veiculos: bool,
    /// detectado: google_client.json + google_token.json no cred_dir
    #[serde(skip)]
    pub tem_google: bool,
}

impl Default for Config {
    fn default() -> Config {
        let s = |x: &str| x.to_string();
        Config {
            motor_dir: String::new(),
            cred_dir: String::new(),
            modelo: s("claude-sonnet-5-5"),
            modelo_agenda: s("claude-haiku-5-5"),
            esforco_leitura: s("medium"),
            esforco_agenda: s("medium"),
            agenda_reset_min: 30.0,
            hora_manutencao: s("06:00"),
            manutencao_repetir_dias: 3.0,
            hora_padrao_tarefa: s("09:00"),
            cobrar_a_cada_h: 3.0,
            janela_cobranca: (s("06:00"), s("23:00")),
            memoria_revisao_dias: 14.0,
            hora_revisao_memoria: s("10:00"),
            git_sync: false,
            espera_fotos_s: 12.0,
            tolerancia_total: 0.03,
            km_max_sem_registro: 3000.0,
            clima_lat: None,
            clima_lon: None,
            chuva_limiar: 70.0,
            hora_chuva_noite: s("21:00"),
            hora_chuva_manha: s("06:30"),
            postos_municipio: None,
            postos_estado: String::new(),
            postos_raio_km: 5.0,
            hora_postos: s("05:00"),
            alerta_nome: String::new(),
            alerta_geocode: None,
            alerta_lat: None,
            alerta_lon: None,
            alerta_rajada_kmh: 70.0,
            alerta_chuva_mm_h: 30.0,
            alerta_chuva_mm_dia: 60.0,
            alerta_silencio: (s("23:00"), s("06:00")),
            alerta_intervalo_min: 30.0,
            fuso: s("America/Sao_Paulo"),
            usuario: crate::agenda::Usuario::default(),
            agenda_parceiro: None,
            prompt_extra: None,
            google_escopos_extra: vec![],
            chuva_aviso: true,
            manutencao_veiculos: None,
            postos_json: None,
            tem_veiculos: true,
            tem_google: true,
        }
    }
}

impl Config {
    /// recurso ligado nesta instância: os de fora pela chave, veículo e agenda pelo que tem no disco
    /// (`detectar`); o resto sempre. Recebe também o nome do comando ("/manutencao", "/hoje"…)
    pub fn ligado(&self, recurso: &str) -> bool {
        match recurso {
            "veiculos" | "manutencao" | "custos" => self.tem_veiculos,
            "agenda" | "hoje" | "amanha" | "semana" => self.tem_google,
            "chuva" => self.clima_lat.is_some() && self.clima_lon.is_some(),
            "temporal" => self.alerta_geocode.is_some(),
            "postos" => self.postos_municipio.is_some(),
            _ => true,
        }
    }

    /// veículo cadastrado e Google configurado (rodar de novo depois de trocar motor_dir/cred_dir)
    pub fn detectar(&mut self) {
        let m = crate::motoregis::Motoregis::new(&self.motor_dir).load_config();
        self.tem_veiculos = m.is_ok_and(|m| !m.veiculos_index.is_empty());
        let cred = Path::new(&self.cred_dir);
        self.tem_google = cred.join("google_client.json").exists() && cred.join("google_token.json").exists();
    }

    /// chaves que dependem uma da outra; erro = o bot não sobe (melhor que avisar "temporal em ")
    pub fn validar(&self) -> R<()> {
        let mut falta = vec![];
        if self.usuario.nome.trim().is_empty() {
            falta.push("usuario.nome");
        }
        if self.alerta_geocode.is_some() && self.alerta_nome.trim().is_empty() {
            falta.push("alerta_nome (tem alerta_geocode)");
        }
        if self.postos_municipio.is_some() && self.postos_estado.trim().is_empty() {
            falta.push("postos_estado (tem postos_municipio)");
        }
        if !falta.is_empty() {
            return Err(Erro::Msg(format!("configs.json: falta {}", falta.join(", "))));
        }
        if self.fuso.parse::<chrono_tz::Tz>().is_err() {
            return Err(Erro::Msg(format!("configs.json: fuso {:?} não está no binário (use um do Brasil, ex. America/Sao_Paulo, ou Europe/Lisbon)", self.fuso)));
        }
        Ok(())
    }

    /// o fuso (validado no load)
    pub fn tz(&self) -> chrono_tz::Tz {
        self.fuso.parse().unwrap_or(chrono_tz::Tz::America__Sao_Paulo)
    }
}

impl Config {
    pub fn cobranca(&self) -> CfgCobranca {
        CfgCobranca { cobrar_a_cada_h: self.cobrar_a_cada_h, janela_cobranca: self.janela_cobranca.clone() }
    }

    pub fn limites_alerta(&self) -> crate::temporal::Limites {
        crate::temporal::Limites { rajada_kmh: self.alerta_rajada_kmh, chuva_mm_h: self.alerta_chuva_mm_h, chuva_mm_dia: self.alerta_chuva_mm_dia }
    }
}

/// os.path.expanduser: $HOME, senão a home do usuário no /etc/passwd
pub fn expanduser(p: &str) -> String {
    let home = || -> Option<String> {
        if let Ok(h) = std::env::var("HOME") {
            return Some(h);
        }
        extern "C" {
            fn getuid() -> u32;
        }
        // SAFETY: getuid não falha nem tem pré-condição
        let uid = unsafe { getuid() }.to_string();
        let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
        passwd.lines().map(|l| l.split(':').collect::<Vec<_>>()).find(|c| c.len() > 5 && c[2] == uid).map(|c| c[5].to_string())
    };
    match p.strip_prefix('~') {
        Some(resto) if resto.is_empty() || resto.starts_with('/') => match home() {
            Some(h) => format!("{h}{resto}"),
            None => p.to_string(),
        },
        _ => p.to_string(),
    }
}

/// pasta do bot (onde ficam configs.json e dados/): $ARGOS_DIR, senão a pasta do binário
/// se tiver configs.json, senão o diretório atual (WorkingDirectory do systemd)
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("ARGOS_DIR") {
        return PathBuf::from(d);
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        if d.join("configs.json").exists() {
            return d;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

pub fn load_config(dir: &Path) -> R<Config> {
    load_config_de(&dir.join("configs.json"))
}

pub fn load_config_de(arquivo: &Path) -> R<Config> {
    let b = std::fs::read(arquivo).map_err(|e| Erro::Msg(format!("{}: {e}", arquivo.display())))?;
    let v: Value = serde_json::from_slice(&b)?;
    if v.get("usuario").is_none() {
        return Err(Erro::Msg(format!("{}: falta \"usuario\" (rode argos --init)", arquivo.display())));
    }
    let mut c: Config = serde_json::from_value(v)?;
    // variáveis de ambiente sobrescrevem (usado nos testes)
    if let Ok(m) = std::env::var("ARGOS_MOTOR_DIR") {
        if !m.is_empty() {
            c.motor_dir = m;
        }
    }
    if std::env::var("ARGOS_GIT_SYNC").as_deref() == Ok("0") {
        c.git_sync = false;
    }
    let dir = arquivo.parent().unwrap_or(Path::new("."));
    if c.motor_dir.is_empty() {
        c.motor_dir = "dados/motor".into();
    }
    if c.cred_dir.is_empty() {
        c.cred_dir = "credenciais".into();
    }
    // relativo = a partir da pasta do bot (não do diretório de onde rodou)
    let absoluto = |p: &str| {
        let p = expanduser(p);
        if Path::new(&p).is_absolute() { p } else { dir.join(p).to_string_lossy().into_owned() }
    };
    c.motor_dir = absoluto(&c.motor_dir);
    c.cred_dir = absoluto(&c.cred_dir);
    c.validar()?;
    c.detectar();
    Ok(c)
}

pub fn read_secret(config: &Config, name: &str) -> R<String> {
    let p = Path::new(&config.cred_dir).join(name);
    Ok(std::fs::read_to_string(&p).map_err(|e| Erro::Msg(format!("{}: {e}", p.display())))?.trim().to_string())
}

// ==========================================
// ZONE 6: INTERFACE
// ==========================================

pub struct Bot {
    pub config: Config,
    pub state_path: PathBuf,
    pub tg: Telegram,
    pub chat_id: i64,
    pub claude: Claude,
    pub mr: Motoregis,
    pub photos: Vec<Vec<u8>>,
    pub last_photo_at: Instant,
    pub draft: Option<Rascunho>,
    /// leitura bruta do rascunho aberto (e quando chegou): foto que chega depois junta nela (`juntar`)
    pub leitura: Option<(Leitura, Instant)>,
    /// já perguntou o posto (📍) pra esse rascunho
    pub perguntou_posto: bool,
    /// texto "posto X" que não achei: vira apelido do posto que ele escolher em seguida
    pub apelido_posto: Option<String>,
    /// já lembrou que o rascunho está aberto sem salvar (uma vez por rascunho)
    pub lembrou_rascunho: bool,
    pub context: Option<Contexto>,
    pub draft_msg: Option<i64>,
    pub msg_date: String,
    pub state: Estado,
    pub agenda: Agenda,
    /// sem o arquivo chat_id: o código que a pessoa manda no privado pra virar a dona do bot
    pub pareamento: Option<String>,
    /// None = sem google_client.json/google_token.json (agenda desligada); `google()` dá o erro claro
    pub gcal: Option<GoogleCalendar>,
    /// op de desfazer da última manutenção gravada (register_maintenance)
    pub last_op: Option<Value>,
    /// dados/postos.json (gravado pelo `argos --anp`)
    pub postos_path: PathBuf,
    /// última localização mandada no chat (e quando)
    pub loc: Option<((f64, f64), Instant)>,
    pub clima: crate::clima::Clima,
    /// avisos do INMET (alerta de temporal)
    pub inmet: crate::temporal::Inmet,
    /// tarefa que espera um horário depois do botão 🕒 (só em memória)
    pub esperando_hora: Option<i64>,
}

impl Bot {
    pub fn new(mut config: Config, data_dir: &Path) -> R<Bot> {
        config.detectar();
        let agent = agente();
        let mut tg = Telegram::new(agent.clone(), &read_secret(&config, "telegram_token")?);
        tg.conversa = Some(data_dir.join("dados").join("conversa.jsonl"));
        let (chat_id, pareamento) = if Path::new(&config.cred_dir).join("chat_id").exists() {
            (crate::util::py_int(&read_secret(&config, "chat_id")?)?, None)
        } else {
            let codigo = crate::util::codigo_pareamento()?;
            println!("pareamento: manda {codigo} pro bot no Telegram, no privado (vale até reiniciar)");
            (0, Some(codigo))
        };
        let claude = Claude::new(agent.clone(), &read_secret(&config, "anthropic_key")?);
        let mr = Motoregis::new(&config.motor_dir);
        let state_path = data_dir.join("dados").join("estado.json");
        let postos_path = match &config.postos_json {
            Some(p) => PathBuf::from(expanduser(p)),
            None => data_dir.join("dados").join("postos.json"),
        };
        let state = load_state(&state_path)?;
        let clima = crate::clima::Clima::new(agent.clone());
        let inmet = crate::temporal::Inmet::new(crate::rede::agente_sem_pool());
        let gcal = if config.tem_google {
            Some(GoogleCalendar::new(agent, Path::new(&config.cred_dir))?)
        } else {
            println!("google: desligado (argos --google-login)");
            None
        };
        Ok(Bot {
            tg,
            chat_id,
            pareamento,
            claude,
            mr,
            photos: vec![],
            last_photo_at: Instant::now(),
            draft: None,
            leitura: None,
            perguntou_posto: false,
            apelido_posto: None,
            lembrou_rascunho: false,
            context: None,
            draft_msg: None,
            msg_date: agora().format("%d/%m/%y").to_string(),
            state,
            state_path,
            postos_path,
            loc: None,
            agenda: Agenda::default(),
            gcal,
            config,
            last_op: None,
            clima,
            inmet,
            esperando_hora: None,
        })
    }

    fn send(&self, text: &str, kb: Option<Value>) -> R<Value> {
        self.tg.send(self.chat_id, text, kb, true)
    }

    /// entrega os recados do bot do parceiro (`recados::receber`): tarefa entra na lista (a de agora cobra já
    /// neste tick, a de depois avisa que chegou); mensagem vai como veio. Devolve quantos entraram.
    pub fn recados_tick(&mut self, now: &crate::util::Dt) -> R<usize> {
        let Some(dados) = self.state_path.parent().map(Path::to_path_buf) else { return Ok(0) };
        let recados = crate::recados::receber(&dados, now)?;
        for r in &recados {
            println!("recado: {}", serde_json::to_string(r).unwrap_or_default());
            if !r.tarefa {
                if let Err(e) = self.send(&format!("📨 Recado {}: {}", r.do_remetente(), esc(&r.texto)), None) {
                    println!("recado não entregue: {e}");
                }
                continue;
            }
            let quando = match r.quando.as_deref().map(crate::util::from_iso).transpose()?.filter(|q| q > now) {
                Some(q) => q,
                None => *now,
            };
            let hora = self.config.hora_padrao_tarefa.clone();
            let texto = format!("{} (pedido {})", r.texto, r.do_remetente());
            let t = crate::tarefas::new_task(&mut self.state, &texto, &quando.format("%Y-%m-%dT%H:%M").to_string(), None, false, &hora)?;
            if quando > *now {
                let aviso = format!("📨 Tarefa {}: {} — {}", r.do_remetente(), esc(&r.texto), crate::tarefas::fmt_when(&t.proximo)?);
                if let Err(e) = self.send(&aviso, None) {
                    println!("aviso de tarefa recebida não foi: {e}");
                }
            }
        }
        if !recados.is_empty() {
            self.save_state()?;
        }
        Ok(recados.len())
    }

    fn save_state(&self) -> R<()> {
        save_state(&self.state, &self.state_path)
    }

    fn git_pull(&self) -> R<()> {
        git_pull(self.config.git_sync, Path::new(&self.config.motor_dir))
    }

    fn git_push(&self, veiculo: &str, data: &str) -> R<String> {
        git_push(self.config.git_sync, Path::new(&self.config.motor_dir), veiculo, data, None)
    }

    /// Reverte um grupo de ações (o último, se undo_id for None).
    pub fn undo(&mut self, undo_id: Option<i64>) -> R<String> {
        crate::desfazer::undo(
            &mut self.state,
            &self.mr,
            self.config.git_sync,
            Path::new(&self.config.motor_dir),
            self.gcal.as_mut(),
            &self.state_path,
            undo_id,
        )
    }

    /// a Google Agenda, ou o erro que vai pro Haiku/usuário quando ela não está configurada
    pub fn google(&mut self) -> R<&mut GoogleCalendar> {
        self.gcal.as_mut().ok_or_else(|| Erro::Msg("Google Agenda não configurada neste bot (argos --google-login)".into()))
    }

    /// nomes dos veículos cadastrados (vazio = sem veículo: foto, manutenção e custos desligados)
    pub fn veiculos(&self) -> Vec<String> {
        self.mr.load_config().map(|m| m.veiculos_index.into_iter().map(|(n, _)| n).collect()).unwrap_or_default()
    }

    /// apelidos dos veículos e lugar do temporal, pros atalhos
    pub fn nomes_atalho(&self) -> crate::atalhos::Nomes {
        let mut v = vec![];
        if let Ok(m) = self.mr.load_config() {
            for (nome, json_path) in m.veiculos_index {
                let moto = self.mr.load_json(&json_path).ok().flatten().as_ref().is_some_and(crate::rascunho::e_moto);
                v.push((nome, moto));
            }
        }
        let lugar = self.config.ligado("temporal").then_some(self.config.alerta_nome.as_str());
        crate::atalhos::Nomes::de(&v, lugar)
    }

    /// o /ajuda (e a resposta de "/" que não é comando): só o que está ligado nesta instância
    pub fn ajuda(&self) -> String {
        let veiculos = self.veiculos();
        let mut t = if self.config.tem_google {
            String::from("📅 <b>Agenda:</b> escreve normal — <i>marca dentista quinta 15h</i>, <i>o que tenho amanhã?</i>, <i>empurra a reunião pra 16h</i>, <i>apaga o date de sexta</i>.\n\n")
        } else {
            String::new()
        };
        t += "✅ <b>Tarefas</b> (lista própria, fora da agenda — eu cobro até você fazer): <i>toda sexta varrer a casa</i>, <i>a cada 15 dias desde a última vez: lavar o capacete</i>, <i>varri a casa</i>. Lista: /tarefas\n";
        t += &format!("🧠 <b>Memória:</b> me conta coisas que valem pra sempre (<i>a Carla é minha chefe</i>), eu guardo e te peço pra revisar a cada {} dias. Ver: /memoria\n", fmt_g(self.config.memoria_revisao_dias));
        if veiculos.is_empty() {
            return t + "\n⛽ Abastecimento e manutenção de veículo: desligados (sem veículo cadastrado).";
        }
        t += &format!("🔧 Manutenção vencida eu aviso sozinho às {}. Fez alguma? <i>calibrei os pneus</i>.\n\n", self.config.hora_manutencao.trim_start_matches('0').replace(":00", "h"));
        t += "⛽ <b>Abastecimento:</b> manda a foto da <b>bomba</b> e a do <b>odômetro</b> (juntas ou em sequência) que eu leio e registro na planilha do veículo.\n\n";
        t + &crate::rascunho::texto_corrigir(&veiculos, "Pra corrigir uma leitura")
    }

    // --- loop ---
    pub fn run(&mut self) -> ! {
        let mut offset: Option<i64> = None;
        println!("argos: rodando");
        let cmds: Vec<Value> = crate::atalhos::COMANDOS.iter().filter(|(c, _)| self.config.ligado(c))
            .map(|(c, d)| match *c {
                "temporal" => json!({"command": c, "description": format!("temporal em {}?", self.config.alerta_nome)}),
                _ => json!({"command": c, "description": d}),
            })
            .collect();
        if let Err(e) = self.tg.call("setMyCommands", 20, json!({"commands": cmds})) {
            println!("setMyCommands: {e}");
        }
        loop {
            let waiting = !self.photos.is_empty();
            let params = json!({"offset": offset, "timeout": if waiting { 3 } else { 30 }, "allowed_updates": ["message", "callback_query"]});
            let updates = match self.tg.call("getUpdates", 40, params) {
                Ok(u) => u.as_array().cloned().unwrap_or_default(),
                Err(e) => {
                    println!("getUpdates: {e}");
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    continue;
                }
            };
            for u in &updates {
                offset = u.get("update_id").and_then(Value::as_i64).map(|i| i + 1).or(offset);
                if let Err(e) = self.handle(u) {
                    println!("erro: {e:?}");
                    if let Err(e2) = self.send(&format!("💥 erro: <code>{}</code>", corta(&esc(&e.to_string()), 300)), None) {
                        println!("erro mandando o erro: {e2}");
                    }
                }
            }
            let quieto = self.last_photo_at.elapsed().as_secs_f64();
            if !self.photos.is_empty() && (self.photos.len() >= 3 || (self.photos.len() == 2 && quieto > 3.0) || quieto > self.config.espera_fotos_s) {
                if let Err(e) = self.process_photos() {
                    println!("erro lendo fotos: {e:?}");
                    let _ = self.send(&format!("💥 não consegui ler: <code>{}</code>", corta(&esc(&e.to_string()), 300)), None);
                }
                self.photos = vec![];
                devolve_memoria();
            }
            if let Err(e) = self.tick() {
                println!("erro nos avisos: {e:?}");
            }
        }
    }

    /// sem dono ainda: só a mensagem privada com o código vale; ela grava o chat_id e o bot passa a ser dela
    fn parear(&mut self, u: &Value) -> R<()> {
        let m = u.get("message").cloned().unwrap_or_default();
        let chat = m.pointer("/chat/id").and_then(Value::as_i64);
        let privado = m.pointer("/chat/type").and_then(Value::as_str) == Some("private");
        let texto = m.get("text").and_then(Value::as_str).unwrap_or_default().trim();
        match chat {
            Some(c) if privado && Some(texto) == self.pareamento.as_deref() => {
                crate::util::grava_secreto(&Path::new(&self.config.cred_dir).join("chat_id"), &c.to_string())?;
                self.chat_id = c;
                self.pareamento = None;
                println!("pareado com o chat {c}");
                self.send("✅ Pareado: agora só respondo pra você. Manda /ajuda pra ver o que eu faço.", None)?;
            }
            _ => println!("ignorado chat {} (esperando o código de pareamento)", chat.map(|c| c.to_string()).unwrap_or_else(|| "None".into())),
        }
        Ok(())
    }

    pub fn handle(&mut self, u: &Value) -> R<()> {
        if self.pareamento.is_some() {
            return self.parear(u);
        }
        if let Some(q) = u.get("callback_query") {
            if q.pointer("/from/id").and_then(Value::as_i64) != Some(self.chat_id) {
                return Ok(());
            }
            // botão tocado enquanto o bot pensava (Haiku de 2 min, 07/10) fica velho: só some o reloginho, a ação vale
            if let Err(e) = self.tg.call("answerCallbackQuery", 30, json!({"callback_query_id": q.get("id")})) {
                println!("botão: answerCallbackQuery falhou ({e:?}), sigo com a ação");
            }
            let data = q.get("data").and_then(Value::as_str).ok_or_else(|| Erro::Chave("'data'".into()))?.to_string();
            self.tg.registra("eu", &crate::conversa::do_botao(q, &data));
            return self.on_button(&data);
        }
        let empty = json!({});
        let m = u.get("message").unwrap_or(&empty);
        let chat = m.pointer("/chat/id").and_then(Value::as_i64);
        if chat != Some(self.chat_id) {
            println!("ignorado chat {}", chat.map(|c| c.to_string()).unwrap_or_else(|| "None".into()));
            return Ok(());
        }
        if let Some(t) = crate::conversa::da_mensagem(m) {
            self.tg.registra("eu", &t);
        }
        if m.get("photo").is_some() && !self.config.tem_veiculos {
            self.send("Sem veículo cadastrado neste bot, então não leio foto de abastecimento. Cadastro: <code>argos --veiculo moto|carro \"Nome\" ano</code> na máquina do bot.", None)?;
        } else if let Some(photo) = m.get("photo") {
            let fid = photo.as_array().and_then(|a| a.last()).and_then(|p| p.get("file_id")).and_then(Value::as_str).ok_or_else(|| Erro::Chave("'file_id'".into()))?;
            let bytes = self.tg.download_photo(fid)?;
            self.photos.push(bytes);
            self.last_photo_at = Instant::now();
            if self.photos.len() == 1 {
                self.msg_date = de_timestamp(m.get("date").and_then(Value::as_i64).unwrap_or(0)).format("%d/%m/%y").to_string();
            }
        } else if let Some(l) = m.get("location") {
            let num = |k: &str| l.get(k).and_then(Value::as_f64).ok_or_else(|| Erro::Chave(format!("'{k}'")));
            self.on_location((num("latitude")?, num("longitude")?))?;
        } else if let Some(t) = m.get("text").and_then(Value::as_str) {
            self.on_text(t)?;
        }
        Ok(())
    }

    // --- fotos ---
    fn process_photos(&mut self) -> R<()> {
        self.tg.call("sendChatAction", 30, json!({"chat_id": self.chat_id, "action": "typing"}))?;
        self.git_pull()?;
        let mconfig = self.mr.load_config()?;
        let ctx = vehicle_context(&self.mr, &mconfig)?;
        let reading = read_photos(&self.claude, &self.config.modelo, &self.config.esforco_leitura, &self.photos, &ctx)?;
        println!("leitura: {reading}");
        self.apply_reading(Leitura::de_json(&reading), ctx)
    }

    /// leitura nova → rascunho. Com rascunho aberto há < 15 min e leitura que completa a dele (bomba numa foto,
    /// painel na outra), junta no mesmo rascunho em vez de começar outro.
    pub fn apply_reading(&mut self, nova: Leitura, ctx: Contexto) -> R<()> {
        let anterior = match (&self.draft, &self.leitura) {
            (Some(d), Some((l, t))) if t.elapsed().as_secs() < 900 => crate::rascunho::juntar(l, &nova).map(|j| (j, d.posto.clone(), d.data.clone())),
            _ => None,
        };
        let (leitura, posto, data) = match anterior {
            Some((j, posto, data)) => {
                println!("leitura: juntei com o rascunho aberto");
                if let Some(id) = self.draft_msg {
                    // a mensagem velha perde os botões: o rascunho continua na nova
                    let _ = self.tg.edit(self.chat_id, id, "↪ juntei com a foto seguinte (rascunho abaixo)", None);
                }
                (j, posto, data)
            }
            None => {
                self.perguntou_posto = false;
                self.apelido_posto = None;
                self.lembrou_rascunho = false;
                (nova, None, self.msg_date.clone())
            }
        };
        let mut d = new_draft(&leitura, &ctx, &data, self.config.km_max_sem_registro);
        if d.posto.is_none() {
            d.posto = posto;
        }
        let postos = crate::postos::carregar(&self.postos_path).unwrap_or_default();
        d.posto_nome = d.posto.as_ref().and_then(|c| postos.postos.get(c)).map(crate::postos::nome_posto);
        let sem_posto = d.posto.is_none();
        self.draft = Some(d);
        self.leitura = Some((leitura, Instant::now()));
        self.context = Some(ctx);
        self.show_draft(true)?;
        if sem_posto && !postos.postos.is_empty() && !self.perguntou_posto {
            self.perguntou_posto = true;
            // botão nativo do Telegram que manda a localização num toque (só existe em teclado de resposta)
            self.tg.call("sendMessage", 30, json!({
                "chat_id": self.chat_id,
                "text": "📍 Sem cupom não sei o posto. Manda a localização que eu acho (ou ignora).",
                "reply_markup": {"keyboard": [[{"text": "📍 mandar localização", "request_location": true}]], "one_time_keyboard": true, "resize_keyboard": true},
            }))?;
        }
        Ok(())
    }

    // --- postos ---
    /// localização no chat: com rascunho sem posto, pergunta qual dos mais próximos; senão, ranking por perto
    fn on_location(&mut self, loc: (f64, f64)) -> R<()> {
        use crate::postos::{carregar, dist_fmt, mais_proximos, nome_posto};
        self.loc = Some((loc, Instant::now()));
        if self.draft.as_ref().map(|d| d.posto.is_none()) == Some(true) {
            let postos = carregar(&self.postos_path)?;
            let perto = mais_proximos(&postos, loc, 3);
            if perto.is_empty() {
                self.send("Ainda não tenho a localização dos postos (o bot baixa da ANP toda manhã).", None)?;
                return Ok(());
            }
            let mut kb: Vec<Value> = perto.iter().map(|(c, km)| json!([botao(&format!("{} · {}", nome_posto(&postos.postos[c]), dist_fmt(*km)), &format!("pst:{c}"))])).collect();
            kb.push(json!([botao("nenhum desses", "pst:-")]));
            self.send("⛽ Qual desses?", Some(Value::Array(kb)))?;
            return Ok(());
        }
        let t = self.texto_postos(Some(loc))?;
        self.send(&t, None)?;
        Ok(())
    }

    /// ranking de onde abastecer (veículo e combustível mais usados), opcionalmente só perto de `perto`
    pub fn texto_postos(&self, perto: Option<(f64, f64)>) -> R<String> {
        use crate::postos::{carregar, mais_usado, ranking, texto_ranking};
        let mconfig = self.mr.load_config()?;
        let mut por = vec![];
        for (nome, json_path) in &mconfig.veiculos_index {
            por.push((nome.clone(), self.mr.read_csv(&crate::motoregis::csv_de(json_path))?));
        }
        let Some((veic, comb, _)) = mais_usado(&por) else { return Ok("Ainda não tem abastecimento registrado.".into()) };
        let linhas = &por.iter().find(|(n, _)| *n == veic).unwrap().1;
        let ops = ranking(linhas, &carregar(&self.postos_path)?, &comb, perto, self.config.postos_raio_km);
        Ok(texto_ranking(&ops, &comb, 5, perto.is_some(), self.config.postos_raio_km))
    }

    /// localização de até 30 min atrás
    fn loc_recente(&self) -> Option<(f64, f64)> {
        self.loc.filter(|(_, t)| t.elapsed().as_secs() < 1800).map(|(l, _)| l)
    }

    fn show_draft(&mut self, new: bool) -> R<()> {
        let (d, ctx) = (self.draft.as_ref().unwrap(), self.context.as_ref().unwrap());
        let (tol, km) = (self.config.tolerancia_total, self.config.km_max_sem_registro);
        let text = render(d, ctx, tol, km);
        let kb = keyboard(d, ctx, tol, km);
        match self.draft_msg {
            Some(id) if !new => self.tg.edit(self.chat_id, id, &text, Some(kb)),
            _ => {
                let r = self.send(&text, Some(kb))?;
                self.draft_msg = r.get("message_id").and_then(Value::as_i64);
                Ok(())
            }
        }
    }

    // --- texto ---
    fn on_text(&mut self, text: &str) -> R<()> {
        self.agenda.foco_hora = None;
        if let Some(id) = self.esperando_hora.take() {
            if self.on_task_time(id, text)? {
                return Ok(());
            }
        }
        if text.starts_with("/memoria") {
            if !self.state.memoria.is_empty() {
                self.send_memory_review(&agora(), true)?;
            } else {
                self.send("Memória vazia. Me conta algo tipo <i>a Carla é minha chefe</i>.", None)?;
            }
            return Ok(());
        }
        if matches!(crate::agenda::fold(text.trim()).as_str(), "/atalhos" | "/comandos" | "atalhos") {
            let texto = crate::atalhos::texto_atalhos(&self.nomes_atalho(), &self.config.alerta_nome);
            let linhas: Vec<&str> = texto.lines().filter(|l| crate::atalhos::comando_da_linha(l).is_none_or(|c| self.config.ligado(c))).collect();
            self.send(&linhas.join("\n"), None)?;
            return Ok(());
        }
        if text.starts_with("/tarefas") {
            self.send(&tasks_overview(&self.state)?, None)?;
            return Ok(());
        }
        // o foco do 🕒 só vale se a mensagem for pro Haiku (atalho não leva a linha pro histórico)
        let foco = self.agenda.foco_hora.take();
        if self.shortcut(text)? {
            return Ok(());
        }
        if text.starts_with('/') {
            self.send(&self.ajuda(), None)?;
            return Ok(());
        }
        self.agenda.foco_hora = foco;
        let mut changed = vec![];
        if let (Some(d), Some(ctx)) = (self.draft.as_mut(), self.context.as_ref()) {
            changed = apply_correction(d, text, &ctx.fuel_types, &ctx.nomes())?;
        }
        if changed.is_empty() {
            if self.on_draft_question(text)? || self.on_posto_text(text)? {
                return Ok(());
            }
            return self.on_agenda(text);
        }
        let ctx = self.context.as_ref().unwrap();
        let d = self.draft.as_mut().unwrap();
        if changed.iter().any(|c| c == "veiculo") {
            d.cheio = ctx.v(&d.veiculo).last_was_full;
        }
        d.gap = suggest_gap(d, ctx);
        self.show_draft(true)
    }

    /// "é o posto perto do cena" com rascunho aberto sem posto: resolve aqui (nunca vai pro Haiku, que gravaria
    /// isso na memória). Achou pelo nome/apelido → botões `pst:`; não achou → pede a localização e guarda o
    /// texto pra virar apelido do posto escolhido.
    fn on_posto_text(&mut self, text: &str) -> R<bool> {
        use crate::postos::{carregar, nome_posto, por_nome};
        if self.draft.as_ref().map(|d| d.posto.is_none()) != Some(true) || !crate::agenda::fold(text).split(|c: char| !c.is_alphanumeric()).any(|w| w == "posto") {
            return Ok(false);
        }
        let postos = carregar(&self.postos_path)?;
        let achados = por_nome(&postos, text);
        if achados.is_empty() && self.loc_recente().is_some() {
            // já mandou a localização e nenhum dos mais próximos é: posto fica em aberto (cadastro depois, com a nota)
            self.apelido_posto = None;
            self.send("👍 posto fica em aberto — dá pra salvar assim:", None)?;
            self.show_draft(true)?;
            return Ok(true);
        }
        if achados.is_empty() {
            // só texto curto vira apelido ("posto perto do cena"); explicação longa não
            self.apelido_posto = (crate::postos::termos(text).len() <= 4).then(|| text.trim().to_string());
            self.tg.call("sendMessage", 30, json!({
                "chat_id": self.chat_id,
                "text": "🤔 Não sei qual posto é esse. Manda a localização que eu acho — e da próxima vez já sei pelo nome.",
                "reply_markup": {"keyboard": [[{"text": "📍 mandar localização", "request_location": true}]], "one_time_keyboard": true, "resize_keyboard": true},
            }))?;
            return Ok(true);
        }
        let mut kb: Vec<Value> = achados.iter().map(|c| json!([botao(&nome_posto(&postos.postos[c]), &format!("pst:{c}"))])).collect();
        kb.push(json!([botao("nenhum desses", "pst:-")]));
        self.send("⛽ É esse?", Some(Value::Array(kb)))?;
        Ok(true)
    }

    /// "o abastecimento foi registrado?" com rascunho aberto: responde aqui (o Haiku não sabe do rascunho)
    fn on_draft_question(&mut self, text: &str) -> R<bool> {
        if self.draft.is_none() || !pergunta_do_rascunho(text) {
            return Ok(false);
        }
        self.send("Ainda não — falta tocar ✅ Salvar. Tá aqui:", None)?;
        self.show_draft(true)?;
        Ok(true)
    }

    /// rascunho aberto há 30 min sem salvar nem cancelar: lembra uma vez e reenvia
    pub fn draft_reminder(&mut self) -> R<()> {
        let velho = self.leitura.as_ref().map(|(_, t)| t.elapsed().as_secs() >= 1800) == Some(true);
        if self.draft.is_none() || self.lembrou_rascunho || !velho {
            return Ok(());
        }
        self.lembrou_rascunho = true;
        let data = self.draft.as_ref().unwrap().data.clone();
        self.send(&format!("⛽ O abastecimento de {data} ainda não foi salvo:"), None)?;
        self.show_draft(true)
    }

    // --- botões ---
    fn on_button(&mut self, data: &str) -> R<()> {
        if data.starts_with("del:") {
            return self.on_delete(data == "del:yes");
        }
        if data.starts_with("mem:") {
            return self.on_memory_button(data);
        }
        if data.starts_with("tar:") {
            return self.on_task_button(data);
        }
        if data.starts_with("mnt:") || data.starts_with("mnk:") {
            return self.on_maintenance_button(data);
        }
        if data.starts_with("mnc:") {
            return self.on_maint_confirm(data == "mnc:yes");
        }
        if data == "mnx" {
            self.send("👍 nada gravado", None)?;
            return Ok(());
        }
        if let Some(id) = data.strip_prefix("und:") {
            let id = crate::util::py_int(id.split(':').next().unwrap_or(""))?;
            let r = self.undo(Some(id))?;
            self.send(&esc(&r), None)?;
            return Ok(());
        }
        if self.draft.is_none() {
            self.send("Esse registro já foi fechado.", None)?;
            return Ok(());
        }
        let ctx = self.context.as_ref().unwrap();
        let d = self.draft.as_mut().unwrap();
        match data {
            "tank" => {
                d.cheio = !d.cheio;
                d.gap = d.gap && d.cheio;
            }
            "gap" => d.gap = !d.gap,
            t if t.starts_with("trip:") => {
                let tag = t.split_once(':').unwrap().1.to_string();
                d.trecho = if d.trecho.as_deref() == Some(tag.as_str()) { None } else { Some(tag) };
            }
            "veh" => {
                let names = ctx.nomes();
                let i = names.iter().position(|n| *n == d.veiculo).unwrap_or(0);
                d.veiculo = names[(i + 1) % names.len()].clone();
                d.cheio = ctx.v(&d.veiculo).last_was_full;
                d.gap = suggest_gap(d, ctx);
            }
            "fix" => {
                self.send(&crate::rascunho::texto_corrigir(&ctx.nomes(), "Pra corrigir"), None)?;
                return Ok(());
            }
            "cancel" => {
                let text = render(d, ctx, self.config.tolerancia_total, self.config.km_max_sem_registro) + "\n\n❌ cancelado";
                let msg = self.draft_msg.ok_or_else(|| Erro::Chave("draft_msg".into()))?;
                self.tg.edit(self.chat_id, msg, &text, None)?;
                self.draft = None;
                self.leitura = None;
                return Ok(());
            }
            "save" => return self.on_save(),
            t if t.starts_with("pst:") => {
                let c = &t[4..];
                if c == "-" {
                    self.apelido_posto = None;
                    self.send("👍 fica sem posto — dá pra salvar assim:", None)?;
                    return self.show_draft(true);
                }
                if let Some(a) = self.apelido_posto.take() {
                    crate::postos::fixar_apelido(&self.postos_path, c, &a)?;
                }
                let postos = crate::postos::carregar(&self.postos_path)?;
                d.posto = Some(c.to_string());
                d.posto_nome = postos.postos.get(c).map(crate::postos::nome_posto);
                // você está no posto: a sua localização vale mais que a da rua (Nominatim)
                if let Some(loc) = self.loc_recente() {
                    crate::postos::fixar_coord(&self.postos_path, c, loc)?;
                }
                return self.show_draft(true);
            }
            _ => {}
        }
        self.show_draft(false)
    }

    // --- avisos ativos ---
    /// cada aviso falha sozinho (só log): antes um `?` no da manutenção (git pull, Telegram fora) pulava
    /// chuva e temporal daquela volta e não gravava o estado
    pub fn tick(&mut self) -> R<()> {
        if self.pareamento.is_some() {
            return Ok(()); // sem dono, nada de aviso
        }
        // veículo cadastrado (argos --veiculo) ou login do Google com o bot rodando: vale sem reiniciar
        self.config.detectar();
        if self.config.tem_google && self.gcal.is_none() {
            match GoogleCalendar::new(crate::rede::agente(), Path::new(&self.config.cred_dir)) {
                Ok(g) => {
                    println!("google: ligado");
                    self.gcal = Some(g);
                }
                Err(e) => println!("google: {e}"),
            }
        }
        if let Err(e) = self.draft_reminder() {
            println!("lembrete do rascunho: {e}");
        }
        let now = agora();
        let mut changed = false;
        match self.recados_tick(&now) {
            Ok(n) => changed = n > 0,
            Err(e) => println!("erro nos recados: {e}"),
        }
        let cfg = self.config.cobranca();
        let mut ocupado = None; // agenda consultada no máximo uma vez por volta, só se alguma tarefa for avisar
        let mut avisos = Vec::new();
        for i in 0..self.state.tarefas.len() {
            let r = self.task_alert(i, &now, &cfg, &mut ocupado);
            changed = changed || self.state.tarefas[i].pendente || r.as_ref().is_ok_and(|k| k.is_some());
            match r {
                Ok(Some(k)) => avisos.push((i, k)),
                Ok(None) => {}
                Err(e) => println!("erro na tarefa {}: {e}", self.state.tarefas[i].texto),
            }
        }
        if let Err(e) = self.send_task_alerts(&avisos) {
            println!("erro no aviso de tarefas: {e}");
        }
        let hoje = now.format("%Y-%m-%d").to_string();
        if self.config.tem_veiculos && now.format("%H:%M").to_string() >= self.config.hora_manutencao && self.state.manutencao_checada.as_deref() != Some(hoje.as_str()) {
            self.state.manutencao_checada = Some(hoje.clone());
            if let Err(e) = self.check_maintenance_alerts(&now, false) {
                println!("erro no aviso de manutenção: {e}");
            }
            changed = true;
        }
        if !prune_expired(&mut self.state, &now).is_empty() {
            changed = true;
        }
        if now.format("%H:%M").to_string() >= self.config.hora_revisao_memoria && self.state.memoria_checada.as_deref() != Some(hoje.as_str()) {
            self.state.memoria_checada = Some(hoje.clone());
            if let Err(e) = self.send_memory_review(&now, false) {
                println!("erro na revisão da memória: {e}");
            }
            changed = true;
        }
        if self.anp_tick(&now) {
            changed = true;
        }
        if self.rain_tick(&now) {
            changed = true;
        }
        if self.storm_tick(&now) {
            changed = true;
        }
        if changed {
            self.save_state()?;
        }
        Ok(())
    }

    /// avança a tarefa `i`; Some = é hora do 🔔/⏰ (quem manda é o `send_task_alerts`).
    /// Em evento da agenda, segura até o fim dele e devolve None
    pub(crate) fn task_alert(&mut self, i: usize, now: &crate::util::Dt, cfg: &CfgCobranca, ocupado: &mut Option<Option<crate::util::Dt>>) -> R<Option<Aviso>> {
        let Some(k) = task_tick(&mut self.state.tarefas[i], now, cfg)? else { return Ok(None) };
        if let Some(fim) = *ocupado.get_or_insert_with(|| self.ocupado_ate(now)) {
            let t = &mut self.state.tarefas[i];
            adiar_por_compromisso(t, k, &fim, cfg)?;
            println!("tarefa: em evento até {}, segurei {}", fim.format("%H:%M"), t.texto);
            return Ok(None);
        }
        Ok(Some(k))
    }

    /// uma tarefa: a mensagem com ✅/⏰ 1h/⏰ amanhã/🕒. Várias na mesma volta (06:30 com 4 cobrando, 14/10):
    /// uma mensagem só, como a da manutenção — um ✅ por tarefa e o adiar de todas embaixo
    pub(crate) fn send_task_alerts(&mut self, avisos: &[(usize, Aviso)]) -> R<()> {
        for &(i, k) in avisos {
            println!("tarefa: {} {}", if k == Aviso::Vence { "vence" } else { "cobra" }, self.state.tarefas[i].texto);
        }
        let (texto, kb) = match avisos {
            [] => return Ok(()),
            [(i, k)] => {
                let t = &self.state.tarefas[*i];
                let head = if *k == Aviso::Vence { "🔔".to_string() } else { format!("⏰ cobrança {}:", t.cobrancas) };
                (format!("{head} <b>{}</b>", esc(&t.texto)),
                 json!([[botao("✅ feito", &format!("tar:ok:{}", t.id)), botao("⏰ 1h", &format!("tar:1h:{}", t.id)),
                         botao("⏰ amanhã", &format!("tar:am:{}", t.id))],
                        [botao("🕒 outra hora", &format!("tar:qq:{}", t.id))]]))
            }
            _ => {
                let ts: Vec<(&Tarefa, Aviso)> = avisos.iter().map(|&(i, k)| (&self.state.tarefas[i], k)).collect();
                let lines = ts.iter().map(|(t, k)| match k {
                    Aviso::Vence => format!("• {} 🔔", esc(&t.texto)),
                    Aviso::Cobra if t.cobrancas > 1 => format!("• {} <i>({}ª vez)</i>", esc(&t.texto), t.cobrancas),
                    Aviso::Cobra => format!("• {}", esc(&t.texto)),
                }).collect::<Vec<_>>().join("\n");
                let mut kb: Vec<Value> = ts.iter().map(|(t, _)| {
                    let rotulo = if t.texto.chars().count() > 28 { format!("{}…", corta(&t.texto, 27).trim_end()) } else { t.texto.clone() };
                    json!([botao(&format!("✅ {rotulo}"), &format!("tar:ok:{}", t.id)), botao("🕒", &format!("tar:qq:{}", t.id))])
                }).collect();
                let ids = ts.iter().map(|(t, _)| t.id.to_string()).collect::<Vec<_>>().join(",");
                // callback_data passa de 64 bytes → o Telegram recusa a mensagem inteira
                if ids.len() <= 64 - "tar:am:".len() {
                    kb.push(json!([botao("⏰ 1h todas", &format!("tar:1h:{ids}")), botao("⏰ amanhã todas", &format!("tar:am:{ids}"))]));
                }
                (format!("⏰ <b>{} tarefas cobrando</b>:\n{lines}\n\nToca no que você já fez:", ts.len()), Value::Array(kb))
            }
        };
        self.send(&texto, Some(kb))?;
        self.anota_aviso(&texto);
        Ok(())
    }

    /// o que o bot mandou de tarefa sem passar pelo modelo vai junto na próxima mensagem pra agenda
    fn anota_aviso(&mut self, texto: &str) {
        let t = crate::util::tira_tags(texto).replace("\n\nToca no que você já fez:", "");
        self.agenda.aviso = Some((agora(), t));
    }

    /// fim do evento da agenda em que você está agora (Google fora → None: avisa mesmo assim)
    fn ocupado_ate(&mut self, now: &crate::util::Dt) -> Option<crate::util::Dt> {
        use chrono::TimeZone;
        // timeMin filtra pelo fim do evento: o que já começou e não acabou vem junto
        let eventos = self.gcal.as_mut()?.list_events(now, &(*now + chrono::Duration::days(1)), None)
            .map_err(|e| println!("agenda pra segurar cobrança: {e}")).ok()?;
        let fim = crate::agenda::ocupado_ate(&eventos, now.naive_local())?;
        crate::util::fuso().from_local_datetime(&fim).earliest().map(|d| d.fixed_offset())
    }

    /// uma vez por dia (hora_postos) roda `argos --anp` num processo à parte: o xlsx da ANP tem 12 MB
    /// descompactado e não pode passar pela memória do bot. true = estado mudou
    pub fn anp_tick(&mut self, now: &crate::util::Dt) -> bool {
        // postos_json de outra instância: quem baixa a ANP é ela
        if self.config.postos_municipio.is_none() || self.config.postos_json.is_some() {
            return false;
        }
        let hoje = now.format("%Y-%m-%d").to_string();
        if now.format("%H:%M").to_string() < self.config.hora_postos || self.state.postos_checado.as_deref() == Some(hoje.as_str()) {
            return false;
        }
        self.state.postos_checado = Some(hoje);
        match std::env::current_exe() {
            Ok(exe) => {
                std::thread::spawn(move || match std::process::Command::new(exe).arg("--anp").output() {
                    Ok(o) => println!("{}{}", String::from_utf8_lossy(&o.stdout).trim(), String::from_utf8_lossy(&o.stderr).trim()),
                    Err(e) => println!("anp: {e}"),
                });
            }
            Err(e) => println!("anp: {e}"),
        }
        true
    }

    /// aviso de chuva: à noite sobre amanhã, de manhã sobre o resto de hoje (uma vez por dia cada).
    /// Falha da Open-Meteo só vai pro log; o dia fica marcado pra não tentar a cada volta. true = estado mudou
    pub fn rain_tick(&mut self, now: &crate::util::Dt) -> bool {
        let (Some(lat), Some(lon)) = (self.config.clima_lat, self.config.clima_lon) else { return false };
        if !self.config.chuva_aviso {
            return false;
        }
        let hoje = now.format("%Y-%m-%d").to_string();
        let hm = now.format("%H:%M").to_string();
        let noite = hm >= self.config.hora_chuva_noite && self.state.chuva_noite_checada.as_deref() != Some(hoje.as_str());
        let manha = hm >= self.config.hora_chuva_manha && self.state.chuva_manha_checada.as_deref() != Some(hoje.as_str());
        if !noite && !manha {
            return false;
        }
        if noite {
            self.state.chuva_noite_checada = Some(hoje.clone());
        }
        if manha {
            self.state.chuva_manha_checada = Some(hoje);
        }
        let horas = match self.clima.previsao(lat, lon) {
            Ok(h) => h,
            Err(e) => {
                println!("erro na previsão: {e}");
                return true;
            }
        };
        let dia = now.date_naive();
        // o bot subindo às 22h: a manhã de hoje já passou, só vale o aviso da noite
        let mut avisos = vec![];
        if manha && !noite {
            avisos.push((dia, chrono::Timelike::hour(now), "hoje"));
        }
        if noite {
            avisos.push((dia + chrono::Duration::days(1), 0, "amanhã"));
        }
        for (d, desde, rotulo) in avisos {
            let periodos = crate::clima::periodos_chuva(&horas, d, desde, self.config.chuva_limiar);
            if !periodos.is_empty() {
                if let Err(e) = self.send(&crate::clima::texto_chuva(rotulo, &periodos), None) {
                    println!("erro mandando aviso de chuva: {e}");
                }
                println!("chuva: aviso {rotulo}");
            }
        }
        true
    }

    /// INMET do município + previsão local no ponto (cada uma pode falhar sozinha)
    pub fn temporal_dados(&self) -> Option<(R<Vec<crate::temporal::Aviso>>, R<Vec<crate::clima::HoraExtrema>>)> {
        let geo = self.config.alerta_geocode.as_deref()?;
        let inmet = self.inmet.avisos(geo);
        devolve_memoria();
        let local = match (self.config.alerta_lat, self.config.alerta_lon) {
            (Some(lat), Some(lon)) => self.clima.extremos(lat, lon),
            _ => Ok(vec![]),
        };
        Some((inmet, local))
    }

    /// alerta de temporal, a cada `alerta_intervalo_min`: avisa o que é novo (INMET vermelho ou extremo
    /// local nas próximas 36 h). Na janela de silêncio só vermelho sai; o resto não é marcado e sai quando ela
    /// acabar. As duas fontes falhando juntas por 12 h avisam uma vez. true = estado mudou
    pub fn storm_tick(&mut self, now: &crate::util::Dt) -> bool {
        use crate::temporal::{chave_inmet, criticos, em_silencio, extremos_locais, nivel, texto_alerta, JANELA_H};
        if self.config.alerta_geocode.is_none() {
            return false;
        }
        let devido = match self.state.alerta_checado.as_deref().and_then(|s| crate::util::from_iso(s).ok()) {
            Some(t) => (*now - t).num_seconds() as f64 >= self.config.alerta_intervalo_min * 60.0,
            None => true,
        };
        if !devido {
            return false;
        }
        self.state.alerta_checado = Some(now.format("%Y-%m-%dT%H:%M").to_string());
        let Some((inmet, local)) = self.temporal_dados() else { return true };
        let hoje = now.date_naive();
        // falha: só log, até as duas passarem 12 h
        match (&inmet, &local) {
            (Ok(_), _) | (_, Ok(_)) => {
                self.state.alerta_falha_desde = None;
            }
            (Err(a), Err(b)) => {
                println!("temporal: INMET {a} · Open-Meteo {b}");
                let desde = self.state.alerta_falha_desde.get_or_insert_with(|| now.format("%Y-%m-%dT%H:%M").to_string()).clone();
                let horas = crate::util::from_iso(&desde).map(|d| (*now - d).num_hours()).unwrap_or(0);
                if horas >= 12 && self.state.alerta_falha_avisada.as_deref() != Some(desde.as_str()) {
                    self.state.alerta_falha_avisada = Some(desde.clone());
                    let quando = desde.get(8..10).zip(desde.get(5..7)).zip(desde.get(11..13)).map(|((d, m), h)| format!("{d}/{m} {h}h")).unwrap_or_default();
                    let t = format!("⚠️ Alerta de temporal em {} sem dados desde {quando} (INMET e Open-Meteo falhando). Fica de olho por conta própria.", esc(&self.config.alerta_nome));
                    if let Err(e) = self.send(&t, None) {
                        println!("erro mandando aviso de falha do temporal: {e}");
                    }
                }
                return true;
            }
        }
        if let Err(e) = &inmet {
            println!("temporal: INMET {e}");
        }
        if let Err(e) = &local {
            println!("temporal: Open-Meteo {e}");
        }
        let avisos = inmet.unwrap_or_default();
        let ext = local.map(|h| extremos_locais(&h, now.naive_local(), JANELA_H, &self.config.limites_alerta())).unwrap_or_default();
        let enviados = &self.state.alerta_enviados;
        let novos_inmet: Vec<&crate::temporal::Aviso> = criticos(&avisos).into_iter().filter(|a| !enviados.contains_key(&chave_inmet(a))).collect();
        let novos_locais: Vec<&crate::temporal::Extremo> = ext.iter().filter(|e| !enviados.contains_key(&e.chave())).collect();
        if novos_inmet.is_empty() && novos_locais.is_empty() {
            return true;
        }
        let vermelho = novos_inmet.iter().any(|a| nivel(a) == 2) || novos_locais.iter().any(|e| e.vermelho());
        if !vermelho && em_silencio(&now.format("%H:%M").to_string(), &self.config.alerta_silencio) {
            println!("temporal: segurando {} aviso(s) até o fim do silêncio", novos_inmet.len() + novos_locais.len());
            return true;
        }
        // com algo novo, o bloco local vai inteiro (contexto); só os avisos do INMET novos
        let texto = texto_alerta(&self.config.alerta_nome, &novos_inmet, &ext, hoje);
        match self.send(&texto, None) {
            Ok(_) => {
                println!("temporal: aviso ({} INMET, {} local)", novos_inmet.len(), novos_locais.len());
                let hoje_s = hoje.format("%Y-%m-%d").to_string();
                let chaves: Vec<String> = novos_inmet.iter().map(|a| chave_inmet(a)).chain(ext.iter().map(|e| e.chave())).collect();
                for k in chaves {
                    self.state.alerta_enviados.insert(k, json!(hoje_s));
                }
            }
            Err(e) => println!("erro mandando alerta de temporal: {e}"),
        }
        // esquece o que tem mais de 3 dias
        let limite = (hoje - chrono::Duration::days(3)).format("%Y-%m-%d").to_string();
        self.state.alerta_enviados.retain(|_, d| d.as_str().map(|d| d >= limite.as_str()).unwrap_or(false));
        true
    }

    pub fn send_memory_review(&self, now: &crate::util::Dt, force: bool) -> R<()> {
        let facts = if force { self.state.memoria.clone() } else { facts_to_review(&self.state, now, self.config.memoria_revisao_dias)? };
        if facts.is_empty() {
            return Ok(());
        }
        let lines = facts.iter().enumerate().map(|(i, f)| format!("{}. {}{}", i + 1, esc(&f.texto), validity_note(f))).collect::<Vec<_>>().join("\n");
        let mut kb: Vec<Value> = facts
            .iter()
            .enumerate()
            .map(|(i, f)| json!([botao(&format!("✅ {}", i + 1), &format!("mem:ok:{}", f.id)), botao(&format!("🗑 {}", i + 1), &format!("mem:del:{}", f.id))]))
            .collect();
        let ids = facts.iter().map(|f| f.id.to_string()).collect::<Vec<_>>().join(",");
        kb.push(json!([botao("✅ tudo certo", &format!("mem:all:{ids}"))]));
        self.send(&format!("🧠 <b>Revisão da memória</b> — ainda vale?\n{lines}\n\nPra corrigir algum, me escreve o certo."), Some(Value::Array(kb)))?;
        Ok(())
    }

    fn on_memory_button(&mut self, data: &str) -> R<()> {
        let mut p = data.splitn(3, ':');
        let (_, kind, ids) = (p.next(), p.next().unwrap_or(""), p.next().ok_or_else(|| Erro::Valor("not enough values to unpack (expected 3, got 2)".into()))?);
        let now = iso(&agora());
        let wanted: Vec<i64> = ids.split(',').map(crate::util::py_int).collect::<R<_>>()?;
        let facts: Vec<_> = self.state.memoria.iter().filter(|f| wanted.contains(&f.id)).cloned().collect();
        if facts.is_empty() {
            self.send("Já resolvido.", None)?;
            return Ok(());
        }
        let mut ops = vec![];
        if kind == "del" {
            ops = facts.iter().map(op_memoria_antes).collect();
            self.state.memoria.retain(|f| !wanted.contains(&f.id));
        } else {
            for f in self.state.memoria.iter_mut().filter(|f| wanted.contains(&f.id)) {
                f.revisado = now.clone();
            }
        }
        let undo_id = push_undo(&mut self.state, "esquecer da memória", ops, &agora());
        self.save_state()?;
        let verb = if kind == "del" { "🗑 esqueci" } else { "✅ mantido" };
        self.send(&format!("{verb}: {}", facts.iter().map(|f| esc(&f.texto)).collect::<Vec<_>>().join("; ")), undo_button(undo_id))?;
        Ok(())
    }

    pub fn check_maintenance_alerts(&mut self, now: &crate::util::Dt, force: bool) -> R<()> {
        self.git_pull()?;
        let mconfig = self.mr.load_config()?;
        // por dia de calendário: o aviso de ontem 06:00:11 não pode segurar o de hoje 06:00:05
        let repeat = self.config.manutencao_repetir_dias.ceil() as i64;
        for (vehicle, items) in overdue_maintenance(&self.mr, &mconfig, now)? {
            if self.config.manutencao_veiculos.as_ref().is_some_and(|vs| !vs.iter().any(|v| crate::agenda::fold(&vehicle).contains(&crate::agenda::fold(v)))) {
                continue;
            }
            let mut keys: Vec<String> = items.iter().map(|(k, _)| k.clone()).collect();
            keys.sort();
            if !force {
                if let Some(last) = self.state.avisos_manutencao.get(&vehicle) {
                    if last.get("itens") == Some(&json!(keys)) {
                        let em = last.get("em").and_then(Value::as_str).ok_or_else(|| Erro::Chave("'em'".into()))?;
                        if (now.date_naive() - from_iso(em)?.date_naive()).num_days() < repeat {
                            continue;
                        }
                    }
                }
            }
            self.send_maintenance(&mconfig, &vehicle, &items, now)?;
        }
        Ok(())
    }

    /// "🔧 <veículo> — manutenção vencida" com um botão ✅ por item; conta como o aviso do dia (`avisos_manutencao`)
    fn send_maintenance(&mut self, mconfig: &crate::motoregis::MotorConfig, vehicle: &str, items: &[(String, String)], now: &crate::util::Dt) -> R<()> {
        let mut keys: Vec<String> = items.iter().map(|(k, _)| k.clone()).collect();
        keys.sort();
        let idx = mconfig.veiculos_index.iter().position(|(n, _)| *n == vehicle).unwrap();
        let kb: Vec<Value> = items.iter().map(|(k, label)| json!([botao(&format!("✅ {label}"), &format!("mnt:{idx}:{k}"))])).collect();
        let lines = items.iter().map(|(_, l)| format!("• {}", esc(l))).collect::<Vec<_>>().join("\n");
        self.send(
            &format!("🔧 <b>{}</b> — manutenção vencida:\n{lines}\n\nToca no que você já fez (grava com a data de hoje):", esc(vehicle)),
            Some(Value::Array(kb)),
        )?;
        self.state.avisos_manutencao.insert(vehicle.to_string(), json!({"itens": keys, "em": iso(now)}));
        Ok(())
    }

    pub fn maintenance_catalog(&self) -> R<String> {
        if !self.config.tem_veiculos {
            return Ok(String::new());
        }
        let items = maintenance_items(&self.mr, &self.mr.load_config()?)?;
        Ok(format!(
            "\n\nItens de manutenção por veículo (chaves exatas):\n{}",
            items.iter().map(|(v, ks)| format!("- {v}: {}", ks.join(", "))).collect::<Vec<_>>().join("\n")
        ))
    }

    /// saída do motoregis.check_maintenance de cada veículo, com o último odômetro e a data de hoje
    pub fn maintenance_overview(&self) -> R<String> {
        self.git_pull()?;
        let today = agora().format("%d/%m/%y").to_string();
        let mut out = Vec::new();
        for (name, json_path) in &self.mr.load_config()?.veiculos_index {
            let csv_path = crate::motoregis::csv_de(json_path);
            let data = self.mr.read_csv(&csv_path)?;
            if data.len() <= 1 {
                continue;
            }
            let odo = self.mr.get_last_odometer(&csv_path)?;
            let report = self.mr.check_maintenance(&csv_path, json_path, odo, &today)?;
            let vehicle = self.mr.load_json(json_path)?.unwrap_or_else(|| json!({}));
            let text = crate::tarefas::annotate_maintenance(report.trim_end(), &data, &vehicle);
            let text = if text.is_empty() { "nada vencido".to_string() } else { text };
            out.push(format!("{name} (último registro {} km, hoje {today}):\n{text}", crate::util::fmt_g(odo)));
        }
        Ok(if out.is_empty() { "nenhum veículo com registros".into() } else { out.join("\n\n") })
    }

    /// confere veículo e itens contra a lista do MotorCostTracker; devolve o CSV do veículo
    pub fn maintenance_csv(&mut self, vehicle: &str, items: &[String]) -> R<String> {
        self.git_pull()?;
        let mconfig = self.mr.load_config()?;
        let valid = maintenance_items(&self.mr, &mconfig)?;
        let Some((_, keys)) = valid.iter().find(|(v, _)| v == vehicle) else {
            let nomes: Vec<String> = valid.iter().map(|(v, _)| v.clone()).collect();
            return Err(Erro::Valor(format!("veículo desconhecido: {vehicle}. Opções: {}", repr_lista(&nomes))));
        };
        let bad: Vec<String> = items.iter().filter(|i| !keys.contains(i)).cloned().collect();
        if !bad.is_empty() {
            return Err(Erro::Valor(format!("itens desconhecidos {}. Opções: {}", repr_lista(&bad), repr_lista(keys))));
        }
        Ok(crate::motoregis::csv_de(mconfig.json_de(vehicle)?))
    }

    /// grava em `date` (dd/mm/aa); `odo` None = estimada pela média km/dia
    pub fn register_maintenance(&mut self, vehicle: &str, items: &[String], cost: f64, date: &str, odo: Option<f64>) -> R<String> {
        let csv = self.maintenance_csv(vehicle, items)?;
        let mconfig = self.mr.load_config()?;
        let (odo, estimado, rows) = record_maintenance(&self.mr, &mconfig, vehicle, items, cost, date, odo)?;
        let sync = self.git_push(vehicle, date)?;
        self.state.avisos_manutencao.remove(vehicle);
        self.save_state()?;
        println!("manutenção: {vehicle} {} {date} {} km R$ {} ({sync})", repr_lista(items), fmt_g(odo), crate::util::fmt_f(cost, 2));
        self.last_op = Some(op_csv(&csv, &rows, vehicle));
        Ok(dumps(&json!({"ok": true, "odometro_usado": odo, "odometro_estimado": estimado, "git": sync})))
    }

    /// botão da manutenção pedida pela agenda com km estimada
    fn on_maint_confirm(&mut self, yes: bool) -> R<()> {
        let Some(p) = self.agenda.pending_maint.take() else {
            self.send("Esse pedido já foi resolvido.", None)?;
            return Ok(());
        };
        if !yes {
            self.send("👍 nada gravado", None)?;
            return Ok(());
        }
        let s = |k: &str| p[k].as_str().unwrap_or("").to_string();
        let itens: Vec<String> = p["itens"].as_array().map(|a| a.iter().map(crate::util::str_num).collect()).unwrap_or_default();
        let odo = p["odometro"].as_f64().ok_or_else(|| Erro::Chave("odometro".into()))?;
        let result: Value = serde_json::from_str(&self.register_maintenance(&s("veiculo"), &itens, p["custo"].as_f64().unwrap_or(0.0), &s("data"), Some(odo))?)?;
        let ops = self.last_op.clone().into_iter().collect();
        let undo_id = push_undo(&mut self.state, &format!("manutenção {}", s("rotulo")), ops, &agora());
        self.save_state()?;
        self.send(
            &format!("🔧 registrado: {} em {} (~{} km) <i>{}</i>", esc(&s("rotulo")), s("data"), fmt_g(odo), esc(result["git"].as_str().unwrap_or(""))),
            undo_button(undo_id),
        )?;
        Ok(())
    }

    fn on_task_button(&mut self, data: &str) -> R<()> {
        let p: Vec<&str> = data.split(':').collect();
        let [_, kind, task_id] = p[..] else {
            return Err(Erro::Valor(format!("too many values to unpack (expected 3, got {})", p.len())));
        };
        if task_id.contains(',') {
            return self.on_tasks_snooze(kind, task_id);
        }
        let id = crate::util::py_int(task_id)?;
        let Some(pos) = self.state.tarefas.iter().position(|t| t.id == id) else {
            self.send("Essa tarefa não existe mais.", None)?;
            return Ok(());
        };
        let now = agora();
        let texto = self.state.tarefas[pos].texto.clone();
        if kind == "no" {
            self.send(&format!("{}: deixei como estava.", esc(&texto)), None)?;
            return Ok(());
        }
        if kind == "qq" {
            self.esperando_hora = Some(id);
            self.send(&format!("{}: pra quando? (ex.: <i>18</i>, <i>18h30</i>, <i>amanhã 9</i>)", esc(&texto)), None)?;
            return Ok(());
        }
        let mut undo_id = None;
        let msg = if kind == "ok" {
            let (msg, ops) = self.conclui_tarefa(id, &now)?;
            undo_id = push_undo(&mut self.state, &format!("tarefa {texto}"), ops, &now);
            msg
        } else {
            task_snooze(&mut self.state.tarefas[pos], &now, kind, &self.config.cobranca())?
        };
        self.save_state()?;
        self.send(&format!("{}: {msg}", esc(&texto)), undo_button(undo_id))?;
        self.anota_aviso(&format!("{texto}: {msg}"));
        Ok(())
    }

    /// "⏰ 1h todas" / "⏰ amanhã todas" da mensagem agrupada: só as que ainda estão cobrando
    /// (a que já levou ✅ não volta a cobrar); um ↩️ desfaz todas
    fn on_tasks_snooze(&mut self, kind: &str, ids: &str) -> R<()> {
        let ids = ids.split(',').map(crate::util::py_int).collect::<R<Vec<i64>>>()?;
        let now = agora();
        let cfg = self.config.cobranca();
        let (mut ops, mut linhas) = (Vec::new(), Vec::new());
        for t in self.state.tarefas.iter_mut().filter(|t| ids.contains(&t.id) && t.pendente) {
            ops.push(op_tarefa_antes(t));
            linhas.push(format!("{}: {}", esc(&t.texto), task_snooze(t, &now, kind, &cfg)?));
        }
        if linhas.is_empty() {
            self.send("Já resolvido.", None)?;
            return Ok(());
        }
        let undo_id = push_undo(&mut self.state, &format!("adiar {} tarefas", linhas.len()), ops, &now);
        self.save_state()?;
        self.send(&linhas.join("\n"), undo_button(undo_id))?;
        self.anota_aviso(&linhas.join("\n"));
        Ok(())
    }

    /// conclui a tarefa (botão ✅ ou agenda); hábito ganha uma linha no dados/tracker.csv.
    /// Devolve a mensagem e as ops de desfazer (o estado não é salvo aqui)
    pub fn conclui_tarefa(&mut self, id: i64, now: &crate::util::Dt) -> R<(String, Vec<Value>)> {
        let pos = self.state.tarefas.iter().position(|t| t.id == id).ok_or_else(|| Erro::Valor(format!("tarefa {id} não existe")))?;
        let t = &self.state.tarefas[pos];
        let mut ops = vec![op_tarefa_antes(t)];
        let habito = crate::tarefas::e_habito(t);
        if habito {
            let row = crate::tarefas::tracker_row(t, now);
            crate::tarefas::append_tracker(&self.state_path.with_file_name("tracker.csv"), &row)?;
            println!("tracker: {}", t.texto);
            ops.push(crate::desfazer::op_tracker(&[row]));
        }
        let msg = task_done(&mut self.state, id, now)?;
        Ok((if habito { format!("{msg} · 📊 no tracker") } else { msg }, ops))
    }

    /// resposta ao "pra quando?" do botão 🕒. false = não era horário (o texto segue o fluxo normal)
    fn on_task_time(&mut self, id: i64, text: &str) -> R<bool> {
        let now = agora();
        let Some(pos) = self.state.tarefas.iter().position(|t| t.id == id) else {
            self.send("Essa tarefa não existe mais.", None)?;
            return Ok(true);
        };
        let texto = self.state.tarefas[pos].texto.clone();
        let Some(alvo) = crate::tarefas::parse_hora_livre(text, &now, &self.state.tarefas[pos].hora) else {
            // o Haiku recebe junto de qual tarefa é a resposta (com 3 cobrando, perguntava "qual?")
            self.agenda.foco_hora = Some(format!("#{id} {texto}"));
            return Ok(false);
        };
        let before = op_tarefa_antes(&self.state.tarefas[pos]);
        let novo = crate::tarefas::task_move(&mut self.state.tarefas[pos], &alvo, &self.config.cobranca())?;
        let undo_id = push_undo(&mut self.state, &format!("tarefa {texto}"), vec![before], &now);
        self.save_state()?;
        self.send(&format!("{}: ⏰ te cobro {}", esc(&texto), crate::tarefas::fmt_when(&novo)?), undo_button(undo_id))?;
        Ok(true)
    }

    fn on_maintenance_button(&mut self, data: &str) -> R<()> {
        let mut p = data.splitn(3, ':');
        let (kind, idx) = (p.next().unwrap_or(""), p.next().unwrap_or(""));
        let key = p.next().ok_or_else(|| Erro::Valor("not enough values to unpack (expected 3, got 2)".into()))?;
        let mconfig = self.mr.load_config()?;
        let i = crate::util::py_int(idx)?;
        let vehicle = mconfig.veiculos_index.get(i as usize).filter(|_| i >= 0).ok_or_else(|| Erro::Chave("list index out of range".into()))?.0.clone();
        let json_path = mconfig.json_de(&vehicle)?.to_string();
        let label = display_name(key, self.mr.load_json(&json_path)?.as_ref());
        if kind == "mnt" {
            // 1º toque: só pergunta
            let (odo, estimado) = crate::tarefas::maintenance_odometer(&self.mr, &crate::motoregis::csv_de(&json_path), agora().date_naive())?;
            self.send(
                &format!("Gravar <b>{}</b> feito hoje ({}{} km)?", esc(&label), if estimado { "~" } else { "" }, fmt_g(odo)),
                Some(json!([[botao("✅ gravar", &format!("mnk:{idx}:{key}")), botao("cancelar", "mnx")]])),
            )?;
            return Ok(());
        }
        let hoje = agora().format("%d/%m/%y").to_string();
        let result: Value = serde_json::from_str(&self.register_maintenance(&vehicle, &[key.to_string()], 0.0, &hoje, None)?)?;
        let ops = self.last_op.clone().into_iter().collect();
        let undo_id = push_undo(&mut self.state, &format!("manutenção {label}"), ops, &agora());
        self.save_state()?;
        let odo = result["odometro_usado"].as_f64().unwrap_or(0.0);
        let til = if result["odometro_estimado"].as_bool().unwrap_or(false) { "~" } else { "" };
        self.send(
            &format!("🔧 registrado: {} ({til}{} km) <i>{}</i>", esc(&label), fmt_g(odo), esc(result["git"].as_str().unwrap_or(""))),
            undo_button(undo_id),
        )?;
        Ok(())
    }

    // --- agenda ---
    /// responde sem o Claude quando dá (crate::atalhos); true = respondido
    fn shortcut(&mut self, text: &str) -> R<bool> {
        use crate::atalhos::{atalho, manutencao_chat, Atalho};
        let now = agora();
        let Some(a) = atalho(text, now.date_naive(), &self.nomes_atalho()) else { return Ok(false) };
        let recurso = match a {
            Atalho::Chuva { .. } => "chuva",
            Atalho::Temporal => "temporal",
            Atalho::Postos => "postos",
            Atalho::Manutencao { .. } => "manutencao",
            Atalho::Custos { .. } => "custos",
            Atalho::Agenda { .. } => "agenda",
            _ => "",
        };
        if !self.config.ligado(recurso) {
            let r = match recurso {
                "manutencao" | "custos" => "Sem veículo cadastrado neste bot.".to_string(),
                "agenda" => "Google Agenda não configurada neste bot.".to_string(),
                r => format!("/{r}: desligado neste bot."),
            };
            self.send(&r, None)?;
            return Ok(true);
        }
        let (reply, keyboard) = match a {
            Atalho::Desfazer => {
                let r = self.undo(None)?;
                (esc(&r), None)
            }
            Atalho::Tarefas => (tasks_overview(&self.state)?, None),
            Atalho::Agenda { inicio, dias, rotulo } => {
                let start = crate::util::no_fuso(inicio.and_hms_opt(0, 0, 0).unwrap());
                let end = crate::util::no_fuso((inicio + chrono::Duration::days(dias)).and_hms_opt(0, 0, 0).unwrap());
                let events = self.google()?.list_events(&start, &end, None)?;
                let dia = inicio.format("%d/%m").to_string();
                let mut lines = vec![];
                for e in events.iter().filter(|e| crate::agenda::fold(&crate::util::str_num(e.get("summary").unwrap_or(&Value::Null))) != "sono") {
                    let l = crate::agenda::agenda_line(e)?;
                    // dia só: tira o "sex 02/10 · " da frente quando é o próprio dia
                    let l = match l.split_once(" · ") {
                        Some((pre, rest)) if dias == 1 && pre.ends_with(&dia) => rest.to_string(),
                        _ => l,
                    };
                    lines.push(format!("• {}", esc(&l)));
                }
                let quando = if dias == 1 { format!("{rotulo} ({} {dia})", crate::util::dia_semana(&inicio).chars().take(3).collect::<String>()) } else { rotulo };
                let r = if lines.is_empty() { format!("Nada na agenda pra {quando}.") } else { format!("📅 <b>{}</b>\n{}", esc(&quando), lines.join("\n")) };
                (r, None)
            }
            Atalho::Manutencao { filtro } => {
                let anotado = self.maintenance_overview()?;
                let nomes = if filtro.is_empty() {
                    None
                } else {
                    let mut n = vec![];
                    for (_, json_path) in &self.mr.load_config()?.veiculos_index {
                        let v = self.mr.load_json(json_path)?.unwrap_or(Value::Null);
                        let keys = v.get("manutencoes").and_then(Value::as_object).map(|m| {
                            m.values().filter_map(Value::as_object).flat_map(|o| o.keys().cloned()).collect::<Vec<_>>()
                        });
                        for k in keys.unwrap_or_default() {
                            if filtro.iter().any(|f| k.contains(f)) {
                                n.push(display_name(&k, Some(&v)));
                            }
                        }
                    }
                    Some(n)
                };
                let (r, achou) = manutencao_chat(&anotado, nomes.as_deref());
                if nomes.is_some() && !achou {
                    return Ok(false); // item em dia: o modelo explica (precisa da data da última vez)
                }
                (r, None)
            }
            Atalho::Postos => {
                if self.config.postos_municipio.is_none() {
                    return Ok(false);
                }
                (self.texto_postos(self.loc_recente())?, None)
            }
            Atalho::Temporal => {
                let Some((inmet, local)) = self.temporal_dados() else { return Ok(false) };
                (crate::temporal::texto_status(&self.config.alerta_nome, &inmet, &local, now.naive_local(), &self.config.limites_alerta()), None)
            }
            Atalho::Custos { veiculo } => {
                let cfg = self.mr.load_config()?;
                let mut blocos = vec![];
                for (nome, json_path) in &cfg.veiculos_index {
                    if veiculo.as_deref().is_some_and(|v| !crate::agenda::fold(nome).contains(v)) {
                        continue;
                    }
                    let rel = self.mr.show_cost_analysis(&crate::motoregis::csv_de(json_path), nome, &cfg)?;
                    let resumo = rel.split_once("--- Resumo ---\n").map_or(rel.as_str(), |(_, r)| r).trim_end();
                    blocos.push(format!("🧾 <b>{}</b>\n<pre>{}</pre>", esc(nome), esc(resumo)));
                }
                if blocos.is_empty() {
                    return Ok(false);
                }
                (blocos.join("\n\n") + "\n<i>(u) último trecho cheio-a-cheio · (g) média geral</i>", None)
            }
            Atalho::Chuva { dia, rotulo } => {
                let (Some(lat), Some(lon)) = (self.config.clima_lat, self.config.clima_lon) else { return Ok(false) };
                let horas = self.clima.previsao(lat, lon)?;
                let desde = if dia == now.date_naive() { chrono::Timelike::hour(&now) } else { 0 };
                let periodos = crate::clima::periodos_chuva(&horas, dia, desde, self.config.chuva_limiar);
                let r = if periodos.is_empty() {
                    crate::clima::resumo_sem_chuva(&rotulo, &horas, dia, desde)
                } else {
                    crate::clima::texto_chuva(&rotulo, &periodos)
                };
                (r, None)
            }
        };
        println!("atalho: {}", corta(text, 60));
        self.tg.send(self.chat_id, &reply, keyboard, true)?;
        // entra no histórico da conversa pra o "empurra ele" seguinte ter contexto
        self.agenda_note(text, &crate::util::tira_tags(&reply));
        Ok(true)
    }

    fn on_agenda(&mut self, text: &str) -> R<()> {
        self.tg.call("sendChatAction", 30, json!({"chat_id": self.chat_id, "action": "typing"}))?;
        let r = self.agenda_ask(text);
        devolve_memoria();
        let mut reply = r?;
        if !self.agenda.actions.is_empty() {
            reply += &format!("\n\n{}", self.agenda.actions.iter().map(|a| format!("✓ {a}")).collect::<Vec<_>>().join("\n"));
        }
        let desc = crate::util::corta(&self.agenda.actions.join("; "), 120);
        let ops = std::mem::take(&mut self.agenda.undo_ops);
        let undo_id = push_undo(&mut self.state, &desc, ops, &agora());
        if undo_id.is_some() {
            self.save_state()?;
        }
        self.tg.send(self.chat_id, &reply, undo_button(undo_id), false)?;
        if let Some(e) = &self.agenda.pending_delete {
            self.send(
                &format!("🗑 Apagar <b>{}</b>?", esc(&event_line(e)?)),
                Some(json!([[botao("🗑 Apagar", "del:yes"), botao("Manter", "del:no")]])),
            )?;
        }
        if let Some(p) = &self.agenda.pending_maint {
            let txt = format!("🔧 Gravar <b>{}</b> em {} com ~{} km (estimada pela média)?",
                              esc(p["rotulo"].as_str().unwrap_or("")), p["data"].as_str().unwrap_or(""), fmt_g(p["odometro"].as_f64().unwrap_or(0.0)));
            self.send(&txt, Some(json!([[botao("✅ gravar", "mnc:yes"), botao("cancelar", "mnc:no")]])))?;
        }
        for (id, desc) in std::mem::take(&mut self.agenda.pending_done) {
            self.send(
                &format!("✅ Concluir <b>{}</b>?", esc(&desc)),
                Some(json!([[botao("✅ Concluir", &format!("tar:ok:{id}")), botao("Não", &format!("tar:no:{id}"))]])),
            )?;
        }
        Ok(())
    }

    fn on_delete(&mut self, confirm: bool) -> R<()> {
        let e = self.agenda.pending_delete.take();
        match e {
            None => {
                self.send("Esse pedido já foi resolvido.", None)?;
            }
            Some(e) if confirm => {
                let id = e.get("id").and_then(Value::as_str).ok_or_else(|| Erro::Chave("'id'".into()))?;
                self.google()?.delete(id)?;
                println!("agenda: apagado {}", event_line(&e)?);
                let mut keep = serde_json::Map::new();
                for k in ["summary", "start", "end", "location", "description", "reminders"] {
                    if let Some(v) = e.get(k) {
                        keep.insert(k.into(), v.clone());
                    }
                }
                let op = json!({"op": "evento_apagado", "evento": keep});
                let undo_id = push_undo(&mut self.state, &format!("apagar {}", event_line(&e)?), vec![op], &agora());
                self.save_state()?;
                self.send(&format!("🗑 apagado: {}", esc(&event_line(&e)?)), undo_button(undo_id))?;
            }
            Some(e) => {
                self.send(&format!("👍 mantido: {}", esc(&event_line(&e)?)), None)?;
            }
        }
        Ok(())
    }

    fn on_save(&mut self) -> R<()> {
        self.git_pull()?;
        let (d, ctx) = (self.draft.as_ref().unwrap(), self.context.as_ref().unwrap());
        let mconfig = self.mr.load_config()?;
        let report = save(&self.mr, d, ctx, &mconfig)?;
        let sync = self.git_push(&d.veiculo, &d.data)?;
        let text = render(d, ctx, self.config.tolerancia_total, self.config.km_max_sem_registro) + "\n\n✅ salvo";
        let msg_id = self.draft_msg.ok_or_else(|| Erro::Chave("draft_msg".into()))?;
        self.tg.edit(self.chat_id, msg_id, &text, None)?;
        // manutenção vai com botões logo abaixo (send_maintenance); aqui só o resumo de consumo/custo
        let resumo = sem_manutencao(&report);
        let msg = if resumo.is_empty() { "✅ salvo".to_string() } else { format!("✅ salvo\n<pre>{}</pre>", esc(resumo)) };
        let op = op_csv(&ctx.v(&d.veiculo).csv, &[csv_row(d)?], &d.veiculo);
        let desc = format!("abastecimento {} {}", d.veiculo, d.data);
        let undo_id = push_undo(&mut self.state, &desc, vec![op], &agora());
        self.save_state()?;
        self.send(&format!("{msg}\n<i>{}</i>", esc(&sync)), undo_button(undo_id))?;
        println!("salvo: {} ({sync})", repr_lista(&csv_row(d)?));
        let vehicle = d.veiculo.clone();
        self.draft = None;
        self.leitura = None;
        let now = agora();
        if let Some((_, items)) = overdue_maintenance(&self.mr, &mconfig, &now)?.into_iter().find(|(v, _)| *v == vehicle) {
            self.send_maintenance(&mconfig, &vehicle, &items, &now)?;
            self.save_state()?;
        }
        Ok(())
    }
}

/// texto que pergunta se o abastecimento foi salvo ("o abastecimento foi registrado?", "salvou?")
pub fn pergunta_do_rascunho(text: &str) -> bool {
    let f = crate::agenda::fold(text.trim());
    let verbo = ["registr", "salv", "grav", "anot", "lancou", "lancad"].iter().any(|v| f.contains(v));
    verbo && (f.contains("abastec") || (f.ends_with('?') && f.split_whitespace().count() <= 3))
}

/// relatório do motoregis sem o bloco "--- Manutenções pendentes ---" (vai com botões numa mensagem à parte)
pub fn sem_manutencao(report: &str) -> &str {
    if !report.starts_with("--- Manutenções pendentes ---") {
        return report;
    }
    report.find("--- Resumo ---").map(|i| &report[i..]).unwrap_or("").trim()
}
