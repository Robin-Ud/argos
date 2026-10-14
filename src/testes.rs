//! Testes das regras puras do Argos — sem rede, sem Claude, numa cópia do MotorCostTracker.
//! Port 1:1 do argos_test.py (27 testes) + comparação do relatório/CSV com o motoregis.py.
use crate::agenda::{agenda_tools, event_line, when_to_google};
use crate::motoregis::{MotorConfig, Motoregis};
use crate::rascunho::*;
use crate::rede::model_params;
use crate::tarefas::*;
use crate::util::{iso, Dt, TempDir};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const MOTO: &str = "Honda CG 160 Titan";
const CARRO: &str = "Chevrolet Onix";

/// python3 sem gravar __pycache__ (as pastas de referência são só leitura)
fn python() -> std::process::Command {
    let mut c = std::process::Command::new("python3");
    c.env("PYTHONDONTWRITEBYTECODE", "1");
    c
}

/// o MotorCostTracker de verdade (só o teste ao vivo contra o motoregis.py usa; sem ele, pula).
/// `ARGOS_MOTOR_REAL` troca o caminho (ex.: um que não existe, pra simular um clone limpo)
fn real_motor() -> PathBuf {
    match std::env::var("ARGOS_MOTOR_REAL") {
        Ok(p) => PathBuf::from(p),
        Err(_) => PathBuf::from(std::env::var("HOME").unwrap()).join("deving/veiculos-saude/MotorCostTracker"),
    }
}

/// os veículos e o lugar do temporal do fixture (o que o `Bot::nomes_atalho` monta com ele)
fn nomes_teste() -> crate::atalhos::Nomes {
    crate::atalhos::Nomes::de(&[("Honda CG 160 Titan".into(), true), ("Chevrolet Onix".into(), false)], Some("Ponta Grossa"))
}

/// `atalhos::atalho` com os nomes do fixture
fn atalho(t: &str, hoje: chrono::NaiveDate) -> Option<crate::atalhos::Atalho> {
    crate::atalhos::atalho(t, hoje, &nomes_teste())
}

/// pasta em tests/fixtures
fn fixture(nome: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(nome)
}

fn copia(origem: &Path, dest: &Path) {
    std::fs::create_dir_all(dest.join("veiculos")).unwrap();
    std::fs::copy(origem.join("configs.json"), dest.join("configs.json")).unwrap();
    for e in std::fs::read_dir(origem.join("veiculos")).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), dest.join("veiculos").join(e.file_name())).unwrap();
    }
}

/// o `Base` do argos_test.py: cópia do MotorCostTracker numa pasta temporária
struct Base {
    _tmp: TempDir,
    mr: Motoregis,
    mconfig: MotorConfig,
    ctx: Contexto,
    last: f64,
}

impl Base {
    fn new() -> Base {
        Base::de(&fixture("motor"))
    }

    fn de(origem: &Path) -> Base {
        let tmp = TempDir::new("argos_test.");
        copia(origem, &tmp.0);
        let mr = Motoregis::new(&tmp.0);
        assert_eq!(mr.dir, tmp.0);
        let mconfig = mr.load_config().unwrap();
        let ctx = vehicle_context(&mr, &mconfig).unwrap();
        let last = ctx.v(MOTO).last_odo.unwrap();
        Base { _tmp: tmp, mr, mconfig, ctx, last }
    }

    fn reading(&self) -> Leitura {
        Leitura {
            total_reais: Some(50.0),
            litros: Some(8.0),
            preco_litro: Some(6.25),
            combustivel: Some("gasolina_comum".into()),
            odometro_km: Some(self.last + 200.0),
            veiculo: Some(MOTO.into()),
            observacao: String::new(),
            cnpj_posto: None,
        }
    }

    fn draft(&self, r: Leitura) -> Rascunho {
        new_draft(&r, &self.ctx, "01/10/26", 3000.0)
    }
}

// --- TestVeiculo ---

#[test]
fn odometro_perto_da_moto() {
    let b = Base::new();
    assert_eq!(pick_vehicle(Some(b.last + 150.0), None, &b.ctx.veiculos, 3000.0), MOTO);
}

#[test]
fn odometro_longe_vai_pro_onix_sem_historico() {
    let b = Base::new();
    assert_eq!(pick_vehicle(Some(98765.0), None, &b.ctx.veiculos, 3000.0), CARRO);
}

#[test]
fn palpite_implausivel_e_ignorado() {
    let b = Base::new();
    assert_eq!(pick_vehicle(Some(b.last + 100.0), Some(CARRO), &b.ctx.veiculos, 3000.0), CARRO);
    // Onix sem histórico é sempre plausível; a moto com odômetro menor não é
    assert_eq!(pick_vehicle(Some(b.last - 500.0), Some(MOTO), &b.ctx.veiculos, 3000.0), CARRO);
}

// --- TestRascunho ---

#[test]
fn completa_total_pela_conta() {
    let b = Base::new();
    let d = b.draft(Leitura { total_reais: None, ..b.reading() });
    assert!((d.total.unwrap() - 50.0).abs() < 1e-7);
}

#[test]
fn sem_problemas() {
    let b = Base::new();
    let d = b.draft(b.reading());
    assert!(problems(&d, &b.ctx, 0.03, 3000.0).is_empty());
}

#[test]
fn conta_nao_bate() {
    let b = Base::new();
    let d = b.draft(Leitura { total_reais: Some(70.0), ..b.reading() });
    assert!(problems(&d, &b.ctx, 0.03, 3000.0).iter().any(|p| p.contains("litros × preço")));
}

#[test]
fn odometro_menor() {
    let b = Base::new();
    let mut d = b.draft(Leitura { odometro_km: Some(b.last - 10.0), veiculo: Some(MOTO.into()), ..b.reading() });
    d.veiculo = MOTO.into();
    assert!(problems(&d, &b.ctx, 0.03, 3000.0).iter().any(|p| p.contains("menor")));
}

#[test]
fn gap_sugerido_em_distancia_absurda() {
    let b = Base::new();
    let d = b.draft(Leitura { odometro_km: Some(b.last + 2500.0), ..b.reading() });
    assert!(d.gap);
}

#[test]
fn detalhes_no_formato_do_motoregis() {
    let b = Base::new();
    let mut d = b.draft(b.reading());
    (d.cheio, d.gap, d.trecho) = (true, true, Some("viagem".into()));
    assert_eq!(details_field(&d), "gasolina_comum|cheio|gap|viagem");
    (d.cheio, d.gap, d.trecho) = (false, false, None);
    assert_eq!(details_field(&d), "gasolina_comum|parcial");
}

// --- TestCorrecao ---

#[test]
fn varios_campos() {
    let b = Base::new();
    let mut d = b.draft(b.reading());
    let changed = apply_correction(&mut d, "km 3.810, litros 8,52 total R$ 53,40", &b.ctx.fuel_types, &b.ctx.nomes()).unwrap();
    assert_eq!((d.odometro, d.litros, d.total), (Some(3810.0), Some(8.52), Some(53.40)));
    let mut c = changed.clone();
    c.sort();
    assert_eq!(c, vec!["litros", "odometro", "total"]);
}

#[test]
fn combustivel_e_veiculo() {
    let b = Base::new();
    let mut d = b.draft(b.reading());
    apply_correction(&mut d, "foi etanol no onix", &b.ctx.fuel_types, &b.ctx.nomes()).unwrap();
    assert_eq!((d.combustivel.as_deref(), d.veiculo.as_str()), (Some("etanol"), CARRO));
}

#[test]
fn aditivada() {
    let b = Base::new();
    let mut d = b.draft(b.reading());
    apply_correction(&mut d, "aditivada", &b.ctx.fuel_types, &b.ctx.nomes()).unwrap();
    assert_eq!(d.combustivel.as_deref(), Some("gasolina_aditivada"));
}

#[test]
fn texto_sem_correcao() {
    let b = Base::new();
    let mut d = b.draft(b.reading());
    assert!(apply_correction(&mut d, "beleza", &b.ctx.fuel_types, &b.ctx.nomes()).unwrap().is_empty());
}

// --- TestGravar ---

#[test]
fn grava_linha_e_relatorio() {
    let b = Base::new();
    let d = b.draft(b.reading());
    let report = save(&b.mr, &d, &b.ctx, &b.mconfig).unwrap();
    let rows = b.mr.read_csv(&b.ctx.v(MOTO).csv).unwrap();
    assert_eq!(rows.last().unwrap(), &csv_row(&d).unwrap());
    assert!(!report.is_empty());
}

#[test]
fn onix_ganha_header() {
    let b = Base::new();
    let d = b.draft(Leitura { odometro_km: Some(98765.0), veiculo: Some(CARRO.into()), ..b.reading() });
    save(&b.mr, &d, &b.ctx, &b.mconfig).unwrap();
    let rows = b.mr.read_csv(&b.ctx.v(CARRO).csv).unwrap();
    assert_eq!(rows[0][0], "data");
    assert_eq!(rows[1][1], "98765");
}

// --- TestSchema ---

#[test]
fn render_e_teclado_sem_quebrar() {
    let ctx = Contexto {
        veiculos: vec![Veiculo { nome: MOTO.into(), moto: true, csv: String::new(), json: String::new(), last_odo: None, last_fuel_odo: None, last_was_full: true, history: vec![] }],
        fuel_types: vec!["etanol".into()],
    };
    let d = new_draft(&Leitura { observacao: "reflexo".into(), ..Default::default() }, &ctx, "01/10/26", 3000.0);
    assert!(render(&d, &ctx, 0.03, 3000.0).contains('❓'));
    let kb = keyboard(&d, &ctx, 0.03, 3000.0);
    assert!(!kb[0].as_array().unwrap().iter().any(|b| b["callback_data"] == "save"));
}

// --- TestAgenda ---

#[test]
fn datas_pro_google() {
    assert_eq!(when_to_google("2026-10-03").unwrap(), json!({"date": "2026-10-03"}));
    assert_eq!(when_to_google("2026-10-03T15:00").unwrap(), json!({"dateTime": "2026-10-03T15:00:00", "timeZone": "America/Sao_Paulo"}));
}

#[test]
fn aware_assume_sao_paulo() {
    assert_eq!(when_to_aware("2026-10-03T15:00").unwrap().offset().local_minus_utc(), -3 * 3600);
}

/// instância com tudo ligado (tests/fixtures/config/completo.json, pessoa inventada)
fn config_completo() -> crate::bot::Config {
    let mut c = crate::bot::load_config_de(&fixture("config").join("completo.json")).unwrap();
    // motor_dir e cred_dir não existem no fixture: veículo e Google ligados na mão
    c.motor_dir = fixture("motor").to_string_lossy().into();
    c.tem_veiculos = true;
    c.tem_google = true;
    c
}

/// outra pessoa, com o Pedro como parceiro: agenda do parceiro, chuva desligada
fn config_dela() -> crate::bot::Config {
    let mut c = config_completo();
    c.usuario = serde_json::from_value(json!({"nome": "Ana", "ela": true, "sobre": "estudante de biologia", "cidade": "Campinas"})).unwrap();
    c.agenda_parceiro = Some(serde_json::from_value(json!({"nome": "Pedro", "calendar_id": "m@exemplo.com"})).unwrap());
    c.prompt_extra = None;
    c.clima_lat = None;
    c
}

/// config_dela com a pasta do bot do parceiro: liga consultar_parceiro, mandar_recado e o `privado` da memória
fn config_dela_com_dir(dir: &str) -> crate::bot::Config {
    let mut c = config_dela();
    c.agenda_parceiro.as_mut().unwrap().dir = Some(dir.into());
    c
}

#[test]
fn anulaveis_no_limite_da_api() {
    // a API recusa (400) o pedido inteiro se as ferramentas strict somarem mais de 16 parâmetros com anyOf/union
    for c in [config_completo(), config_dela(), config_dela_com_dir("/tmp")] {
        let n: usize = agenda_tools(&c).as_array().unwrap().iter()
            .filter(|t| t["strict"] == json!(true))
            .map(|t| t["input_schema"]["properties"].as_object().unwrap().values().filter(|p| p.get("anyOf").is_some()).count())
            .sum();
        assert!(n <= 16, "{n} parâmetros anuláveis em ferramentas strict (limite 16)");
    }
}

#[test]
fn linha_do_evento() {
    let e = json!({"id": "x", "summary": "Dentista", "start": {"dateTime": "2026-10-08T15:00:00-03:00"}, "end": {"dateTime": "2026-10-08T16:00:00-03:00"}});
    assert_eq!(event_line(&e).unwrap(), "Dentista — quinta 08/10 15:00");
    let e = json!({"id": "y", "summary": "Feriado", "start": {"date": "2026-10-12"}, "end": {"date": "2026-10-13"}});
    assert_eq!(event_line(&e).unwrap(), "Feriado — 12/10 (dia inteiro)");
}

#[test]
fn schemas_estritos() {
    for t in agenda_tools(&config_completo()).as_array().unwrap().iter().chain(agenda_tools(&config_dela()).as_array().unwrap()) {
        if ["previsao_chuva", "registrar_manutencao", "adiar_tarefa"].contains(&t["name"].as_str().unwrap()) {
            assert!(t.get("strict").is_none());
            continue;
        }
        let sch = &t["input_schema"];
        let mut req: Vec<&str> = sch["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        let mut props: Vec<&str> = sch["properties"].as_object().unwrap().keys().map(String::as_str).collect();
        req.sort();
        props.sort();
        assert_eq!(req, props);
        assert_eq!(sch["additionalProperties"], json!(false));
        assert_eq!(t["strict"], json!(true));
    }
}

// --- TestTarefas ---

fn cfg() -> CfgCobranca {
    CfgCobranca { cobrar_a_cada_h: 3.0, janela_cobranca: ("08:00".into(), "22:00".into()) }
}

fn at(s: &str) -> Dt {
    when_to_aware(s).unwrap()
}

fn estado() -> Estado {
    load_state(Path::new("/nonexistent/estado.json")).unwrap()
}

#[test]
fn semanal_fixa_vence_cobra_e_anda() {
    let mut st = estado();
    let id = new_task(&mut st, "varrer a casa", "2026-10-02T18:00", Some(7), false, "09:00").unwrap().id;
    let c = cfg();
    let tick = |st: &mut Estado, s: &str| task_tick(&mut st.tarefas[0], &at(s), &c).unwrap();
    assert_eq!(tick(&mut st, "2026-10-02T17:59"), None);
    assert_eq!(tick(&mut st, "2026-10-02T18:00"), Some(Aviso::Vence));
    assert_eq!(&st.tarefas[0].proximo[..16], "2026-10-09T18:00"); // já marcou a próxima sexta
    assert_eq!(tick(&mut st, "2026-10-02T20:00"), None);
    assert_eq!(tick(&mut st, "2026-10-02T21:00"), Some(Aviso::Cobra));
    // próxima cobrança cairia 00:00 → empurra pras 08:00
    assert_eq!(&st.tarefas[0].cobrar_em.as_ref().unwrap()[..16], "2026-10-03T08:00");
    assert_eq!(tick(&mut st, "2026-10-03T07:00"), None);
    assert_eq!(tick(&mut st, "2026-10-03T08:00"), Some(Aviso::Cobra));
    task_done(&mut st, id, &at("2026-10-03T10:00")).unwrap();
    assert!(!st.tarefas[0].pendente);
    assert_eq!(&st.tarefas[0].proximo[..16], "2026-10-09T18:00"); // agenda fixa não muda
}

#[test]
fn feita_adiantada_pula_a_proxima() {
    let mut st = estado();
    let id = new_task(&mut st, "varrer", "2026-10-02T18:00", Some(7), false, "09:00").unwrap().id;
    task_done(&mut st, id, &at("2026-10-01T15:00")).unwrap();
    assert_eq!(&st.tarefas[0].proximo[..16], "2026-10-09T18:00");
}

#[test]
fn desde_feito_conta_da_conclusao() {
    let mut st = estado();
    let id = new_task(&mut st, "lavar capacete", "2026-10-01", Some(15), true, "09:00").unwrap().id;
    assert_eq!(&st.tarefas[0].proximo[..16], "2026-10-01T09:00");
    assert_eq!(task_tick(&mut st.tarefas[0], &at("2026-10-01T09:00"), &cfg()).unwrap(), Some(Aviso::Vence));
    task_done(&mut st, id, &at("2026-10-04T19:30")).unwrap();
    assert_eq!(&st.tarefas[0].proximo[..16], "2026-10-19T09:00");
}

#[test]
fn uma_vez_some_ao_concluir() {
    let mut st = estado();
    let id = new_task(&mut st, "pagar boleto", "2026-10-05T10:00", None, false, "09:00").unwrap().id;
    task_tick(&mut st.tarefas[0], &at("2026-10-05T10:00"), &cfg()).unwrap();
    task_done(&mut st, id, &at("2026-10-05T11:00")).unwrap();
    assert!(st.tarefas.is_empty());
}

#[test]
fn adiar_uma_hora() {
    let mut st = estado();
    new_task(&mut st, "x", "2026-10-05T10:00", None, false, "09:00").unwrap();
    let t = &mut st.tarefas[0];
    task_tick(t, &at("2026-10-05T10:00"), &cfg()).unwrap();
    task_snooze(t, &at("2026-10-05T10:05"), "1h", &cfg()).unwrap();
    assert_eq!(task_tick(t, &at("2026-10-05T11:00"), &cfg()).unwrap(), None);
    assert_eq!(task_tick(t, &at("2026-10-05T11:05"), &cfg()).unwrap(), Some(Aviso::Cobra));
}

#[test]
fn model_params_por_modelo() {
    assert_eq!(model_params("claude-haiku-4-5", "medium"), (json!({"model": "claude-haiku-4-5"}), None));
    let (p, beta) = model_params("claude-sonnet-5-5", "medium");
    assert!(p.get("fallbacks").is_some());
    assert_eq!(beta, Some("server-side-fallback-2026-07-01"));
    let (p, beta) = model_params("claude-haiku-5-5", "medium");
    assert!(p.get("fallbacks").is_none());
    assert_eq!(p["output_config"]["effort"], "medium");
    assert_eq!(p["thinking"]["block_binding"]["prefix_mismatch_behavior"], "drop_block");
    assert_eq!(beta, Some("thinking-binding-controls-2026-08-01"));
}

#[test]
fn odometro_estimado_pela_media_diaria() {
    use crate::tarefas::estimate_odometer;
    let l = |d: &str, o: &str, c: &str| vec![d.to_string(), o.to_string(), c.to_string(), "0".into(), "0".into(), "x".into()];
    let dia = |s: &str| crate::util::parse_ddmmyy(s).unwrap();
    // fora de ordem de propósito, e manutenção (cat 2) com km repetida não conta
    let data = vec![l("data", "odometro", "categoria"), l("01/09/26", "1000", "1"), l("21/09/26", "1400", "1"),
                    l("11/09/26", "1400", "2"), l("11/09/26", "1100", "1")];
    // entre dois abastecimentos: km/dia do trecho (11/09 1100 → 21/09 1400 = 30/dia)
    assert_eq!(estimate_odometer(&data, dia("15/09/26")), Some(1220.0));
    // depois do último: média do histórico (400 km / 20 d = 20/dia)
    assert_eq!(estimate_odometer(&data, dia("24/09/26")), Some(1460.0));
    assert_eq!(estimate_odometer(&data, dia("21/09/26")), Some(1400.0));
    // antes do primeiro, ou sem dois abastecimentos em dias diferentes: sem estimativa
    assert_eq!(estimate_odometer(&data, dia("31/08/26")), None);
    assert_eq!(estimate_odometer(&data[..2].to_vec(), dia("24/09/26")), None);
    // manutenção 3 dias depois do último abastecimento (14/04 1210 → 05/10 5130 = 3920 km em 174 d = 22,53 km/dia)
    let longo = vec![l("data", "odometro", "categoria"), l("14/04/26", "1210", "1"), l("05/10/26", "5130", "1")];
    assert_eq!(estimate_odometer(&longo, dia("08/10/26")), Some(5197.6));
}

// --- TestManutencao ---

#[test]
fn vencidas_e_registro() {
    let b = Base::new();
    let today = at("2026-10-01T08:00");
    let keys = |b: &Base| -> Vec<String> {
        overdue_maintenance(&b.mr, &b.mconfig, &today).unwrap().into_iter().find(|(v, _)| v == MOTO).map(|(_, i)| i.into_iter().map(|(k, _)| k).collect()).unwrap_or_default()
    };
    assert!(keys(&b).contains(&"limpar_lubrificar_corrente".to_string())); // última vez: 13/09
    record_maintenance(&b.mr, &b.mconfig, MOTO, &["limpar_lubrificar_corrente".into()], 0.0, "01/10/26", None).unwrap();
    assert!(!keys(&b).contains(&"limpar_lubrificar_corrente".to_string()));
    let rows = b.mr.read_csv(&b.ctx.v(MOTO).csv).unwrap();
    assert_eq!(&rows.last().unwrap()[2..], &["2", "0.00", "0.00", "limpar_lubrificar_corrente"]);
}

// ==========================================
// Extras: estado.json e paridade com o motoregis.py
// ==========================================

#[test]
fn estado_json_mantem_formato_e_chaves_desconhecidas() {
    let tmp = TempDir::new("argos_estado.");
    let p = tmp.0.join("dados/estado.json");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let orig = "{\n  \"tarefas\": [],\n  \"avisos_manutencao\": {},\n  \"proximo_id\": 2,\n  \"manutencao_checada\": \"2026-10-01\",\n  \"memoria\": [\n    {\n      \"id\": 1,\n      \"texto\": \"ç\",\n      \"criado\": \"2026-10-01T10:00:00-03:00\",\n      \"revisado\": \"2026-10-01T10:00:00-03:00\",\n      \"ate\": null\n    }\n  ],\n  \"desfazer\": [],\n  \"memoria_checada\": \"2026-10-01\",\n  \"futuro\": {\n    \"x\": [\n      1\n    ]\n  }\n}";
    std::fs::write(&p, orig).unwrap();
    let mut st = load_state(&p).unwrap();
    save_state(&st, &p).unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), orig); // igual ao json.dump(indent=2, ensure_ascii=False)
    new_task(&mut st, "varrer", "2026-10-02T18:00", Some(7), false, "09:00").unwrap();
    save_state(&st, &p).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(v["tarefas"][0], json!({"id": 2, "texto": "varrer", "proximo": "2026-10-02T18:00:00-03:00", "hora": "18:00",
        "repetir_dias": 7, "desde_feito": false, "pendente": false, "cobrar_em": null, "cobrancas": 0}));
    assert_eq!(v["memoria"][0]["texto"], "ç");
    assert_eq!(v["futuro"], json!({"x": [1]}));
}

#[test]
fn isoformat_com_microssegundos() {
    let dt = at("2026-10-02T21:00:00.000250-03:00");
    assert_eq!(iso(&dt), "2026-10-02T21:00:00.000250-03:00");
}

/// relatório (check_maintenance + show_cost_analysis) do CSV congelado em tests/fixtures,
/// texto capturado do motoregis.py em 2026-10-01
#[test]
fn relatorio_igual_ao_python_fixture() {
    let mr = Motoregis::new(fixture("motor"));
    let cfg = mr.load_config().unwrap();
    let c = "veiculos/titan_22.csv";
    let mut out = mr.check_maintenance(c, "veiculos/titan_22.json", mr.get_last_odometer(c).unwrap(), "01/10/26").unwrap();
    out += &mr.show_cost_analysis(c, MOTO, &cfg).unwrap();
    let esperado = "
--- Manutenções pendentes ---

  ⚠️  Calibrar pneus
      99 dias atrás (a cada 15 dias)
  ⚠️  Lubrif. corrente
      19 dias atrás (a cada 15 dias)


--- Resumo ---
  14/04/26 → 10/09/26  (3531.2 km)

  (u)Km/L: 38.08 km/L (-1%)
  (u)Dias rodados: 11 dias (+27% - (g)8.6 dias)
  (u)Km rodados: 243.3 km (+10% - (g)220 km)

  (g)Custo/dia: R$6.19/dia (-43% - (u)R$3.54/dia)
  (g)Custo/10km: R$1.58/10km (+1%)
";
    assert_eq!(out, esperado);
}

/// roda o motoregis.py de verdade (cópia numa pasta temporária) e compara relatório e bytes do CSV
/// em vários cenários. Pulado se não houver python3 ou o MotorCostTracker.
#[test]
fn relatorio_e_csv_iguais_ao_python_ao_vivo() {
    let py = real_motor().join("motoregis.py");
    if !py.exists() || python().arg("--version").output().is_err() {
        eprintln!("(pulado: sem python3 ou motoregis.py)");
        return;
    }
    // só o motoregis.py vem do MotorCostTracker; os dados são os do fixture
    for origem in [fixture("motor")] {
        let b = Base::de(&origem);
        let pydir = TempDir::new("argos_py.");
        copia(&b.mr.dir, &pydir.0);
        std::fs::copy(&py, pydir.0.join("motoregis.py")).unwrap();
        let pymr = Motoregis::new(&pydir.0);

        // cenários: cheio normal, parcial, gap + viagem, cidade, Onix do zero (header + 1 e 2 linhas), manutenção com vírgula
        let mut drafts = vec![];
        let mut d = b.draft(b.reading());
        drafts.push(d.clone());
        d.odometro = Some(b.last + 333.3);
        d.cheio = false;
        d.litros = Some(3.456);
        drafts.push(d.clone());
        let mut d = b.draft(Leitura { odometro_km: Some(b.last + 2500.0), total_reais: Some(61.35), ..b.reading() });
        d.trecho = Some("viagem".into());
        drafts.push(d.clone());
        let mut d = b.draft(Leitura { odometro_km: Some(b.last + 2700.5), ..b.reading() });
        d.trecho = Some("cidade".into());
        drafts.push(d);
        // tag posto= (postos.rs) não pode mudar nada no relatório do motoregis.py
        let mut d = b.draft(Leitura { odometro_km: Some(b.last + 2911.0), cnpj_posto: Some("09308519000106".into()), ..b.reading() });
        d.trecho = Some("cidade".into());
        drafts.push(d);
        let e1 = b.draft(Leitura { odometro_km: Some(98765.0), veiculo: Some(CARRO.into()), ..b.reading() });
        let mut e2 = e1.clone();
        e2.odometro = Some(99210.0);
        e2.data = "15/10/26".into();
        drafts.push(e1);
        drafts.push(e2);

        for d in &drafts {
            // Rust grava e gera o relatório na cópia dele
            let ctx = vehicle_context(&b.mr, &b.mconfig).unwrap();
            let rust = save(&b.mr, d, &ctx, &b.mconfig).unwrap();
            // Python grava a mesma linha e imprime o relatório na cópia dele
            let v = ctx.v(&d.veiculo);
            let row = csv_row(d).unwrap();
            let script = format!(
                "import io, contextlib, os, motoregis as mr\n\
                 cfg = mr.load_config()\n\
                 if not os.path.exists(os.path.join(mr.DATA_DIR, {csv:?})):\n    mr.append_record({csv:?}, ['data','odometro','categoria','custo','litros','detalhes'])\n\
                 mr.append_record({csv:?}, [{r0:?}, {r1:?}, 1, {r3:?}, {r4:?}, {r5:?}])\n\
                 buf = io.StringIO()\n\
                 with contextlib.redirect_stdout(buf):\n    mr.check_maintenance({csv:?}, {json:?}, {odo}, {data:?})\n    mr.show_cost_analysis({csv:?}, {nome:?}, cfg)\n\
                 print(buf.getvalue().strip(), end='')\n",
                csv = v.csv, json = v.json, r0 = row[0], r1 = row[1], r3 = row[3], r4 = row[4], r5 = row[5],
                odo = crate::util::repr_f64(d.odometro.unwrap()), data = d.data, nome = d.veiculo
            );
            let out = python().arg("-c").arg(&script).current_dir(&pydir.0).output().unwrap();
            assert!(out.status.success(), "python falhou: {}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(rust, String::from_utf8(out.stdout).unwrap(), "relatório diferente pro rascunho {d:?}");
            assert_eq!(std::fs::read(b.mr.caminho(&v.csv)).unwrap(), std::fs::read(pymr.caminho(&v.csv)).unwrap(), "CSV diferente");
        }

        // manutenção (e uma linha que precisa de aspas no csv.writer)
        // a km é a estimativa do Rust (o motoregis.py repete a última); aqui só o formato do CSV é comparado
        let (odo, _, _) = record_maintenance(&b.mr, &b.mconfig, MOTO, &["calibrar_pneus".into(), "a,b \"x\"".into()], 10.0, "02/10/26", None).unwrap();
        let script = format!("import motoregis as mr\nodo = {}\n", crate::util::repr_f64(odo)) + "\
                      [mr.append_record('veiculos/titan_22.csv', ['02/10/26', f'{odo:g}', 2, f'{10.0/2:.2f}', '0.00', i]) for i in ['calibrar_pneus', 'a,b \"x\"']]\n";
        let out = python().arg("-c").arg(&script).current_dir(&pydir.0).output().unwrap();
        assert!(out.status.success());
        let c = "veiculos/titan_22.csv";
        assert_eq!(std::fs::read(b.mr.caminho(c)).unwrap(), std::fs::read(pymr.caminho(c)).unwrap());

        // urgências (régua do _compute_urgency) pra todos os itens, em várias datas
        for dia in ["2026-10-01", "2026-12-25", "2027-06-30"] {
            let today = at(&format!("{dia}T08:00"));
            let rust: Vec<(String, Vec<String>)> = overdue_maintenance(&b.mr, &b.mconfig, &today)
                .unwrap()
                .into_iter()
                .map(|(v, i)| {
                    let mut k: Vec<String> = i.into_iter().map(|(k, d)| format!("{k}={d}")).collect();
                    k.sort();
                    (v, k)
                })
                .collect();
            let script = format!(
                "import sys; sys.path.insert(0, {argos:?}); sys.argv=['x']\n\
                 import motoregis as mr\nfrom datetime import datetime\n\
                 sys.modules['motoregis'] = mr\nimport argos\n\
                 r = argos.overdue_maintenance(mr, mr.load_config(), datetime.fromisoformat({dia:?}+'T08:00').replace(tzinfo=argos.TZ))\n\
                 for v, i in r.items(): print(v + '|' + ';'.join(sorted(f'{{k}}={{d}}' for k, d in i)))\n",
                argos = PathBuf::from(std::env::var("HOME").unwrap()).join("deving/argos/py").to_string_lossy(),
            );
            let out = python().arg("-c").arg(&script).current_dir(&pydir.0).output().unwrap();
            if !out.status.success() {
                // argos.py depende do SDK anthropic só dentro do Bot; se o import falhar por outro motivo, pula
                eprintln!("(urgências puladas: {})", String::from_utf8_lossy(&out.stderr));
                continue;
            }
            let py: Vec<(String, Vec<String>)> = String::from_utf8(out.stdout)
                .unwrap()
                .lines()
                .map(|l| {
                    let (v, i) = l.split_once('|').unwrap();
                    (v.to_string(), i.split(';').map(String::from).collect())
                })
                .collect();
            assert_eq!(rust, py, "urgências diferentes em {dia}");
        }
    }
}

// ==========================================
// Claude falso em 127.0.0.1: loop de ferramentas, histórico intacto e novas tentativas
// ==========================================

struct Pedido {
    headers: String,
    corpo: Value,
}

/// servidor HTTP mínimo: responde a fila (status, corpo) em ordem e grava cada pedido
fn claude_falso(respostas: Vec<(u16, Value)>) -> (String, std::sync::mpsc::Receiver<Pedido>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1/messages", l.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for (status, corpo) in respostas {
            let (mut s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut headers = String::new();
            loop {
                let mut linha = String::new();
                r.read_line(&mut linha).unwrap();
                if linha == "\r\n" {
                    break;
                }
                headers.push_str(&linha.to_lowercase());
            }
            let n: usize = headers.lines().find_map(|h| h.strip_prefix("content-length: ")).map(|n| n.trim().parse().unwrap()).unwrap_or(0);
            let mut b = vec![0; n];
            r.read_exact(&mut b).unwrap();
            let corpo_pedido = serde_json::from_slice(&b).unwrap_or_else(|_| json!(String::from_utf8_lossy(&b)));
            tx.send(Pedido { headers, corpo: corpo_pedido }).unwrap();
            let c = corpo.to_string();
            let extra = if status == 429 { "retry-after: 0.1\r\n" } else { "" };
            write!(s, "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{c}", c.len()).unwrap();
        }
    });
    (url, rx)
}

fn bot_de_teste(tmp: &Path, motor: &Path) -> crate::bot::Bot {
    let cred = tmp.join("cred");
    std::fs::create_dir_all(&cred).unwrap();
    for (n, c) in [("telegram_token", "0:falso"), ("chat_id", "1"), ("anthropic_key", "sk-falsa"),
                   ("google_client.json", r#"{"installed": {"client_id": "x", "client_secret": "y", "token_uri": "http://127.0.0.1:9/token"}}"#),
                   ("google_token.json", r#"{"refresh_token": "z"}"#)] {
        std::fs::write(cred.join(n), c).unwrap();
    }
    std::fs::write(tmp.join("configs.json"), std::fs::read(fixture("config").join("completo.json")).unwrap()).unwrap();
    let mut config = crate::bot::load_config(tmp).unwrap();
    config.cred_dir = cred.to_string_lossy().into();
    config.motor_dir = motor.to_string_lossy().into();
    config.git_sync = false;
    crate::bot::Bot::new(config, tmp).unwrap()
}

#[test]
fn agenda_devolve_thinking_intacto_e_anexa_recibo() {
    let tmp = TempDir::new("argos_agenda.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    // 1ª volta: thinking (com assinatura) + texto + 2 ferramentas; 2ª: resposta final
    let volta1 = json!({"stop_reason": "tool_use", "content": [
        {"type": "thinking", "thinking": "", "signature": "c2lnbmF0dXJh=="},
        {"type": "text", "text": "vou criar", "citations": null},
        {"type": "tool_use", "id": "tu_1", "name": "criar_tarefa",
         "input": {"texto": "varrer a casa", "quando": "2026-10-02", "repetir_dias": 7, "contar_desde_feito": false}},
        {"type": "tool_use", "id": "tu_2", "name": "concluir_tarefa", "input": {"id": 99}}]});
    let volta2 = json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "Feito: toda sexta."}]});
    let (url, rx) = claude_falso(vec![(200, volta1.clone()), (200, volta2)]);
    bot.claude.url = url;

    let r = bot.agenda_ask("toda sexta varrer a casa").unwrap();
    assert_eq!(r, "Feito: toda sexta.");
    assert_eq!(bot.agenda.actions, vec!["✅ tarefa criada: varrer a casa — sexta 02/10 09:00 · a cada 7d"]);
    assert_eq!(bot.state.tarefas.len(), 1);

    let p1 = rx.recv().unwrap();
    let p2 = rx.recv().unwrap();
    // Haiku 5.5: effort + thinking com drop_block (beta de binding), sem fallbacks nem tool_choice
    assert!(p1.headers.contains("anthropic-beta: thinking-binding-controls-2026-08-01"));
    assert!(p1.headers.contains("anthropic-version: 2023-06-01") && p1.headers.contains("x-api-key: sk-falsa"));
    for k in ["fallbacks", "tool_choice"] {
        assert!(p1.corpo.get(k).is_none(), "{k}");
    }
    assert_eq!(p1.corpo["output_config"], json!({"effort": "medium"}));
    assert_eq!(p1.corpo["thinking"]["block_binding"]["prefix_mismatch_behavior"], "drop_block");
    assert_eq!(p1.corpo["model"], "claude-haiku-5-5");
    assert_eq!(p1.corpo["tools"].as_array().unwrap().len(), 18);
    let sys = p1.corpo["system"][0]["text"].as_str().unwrap();
    assert!(sys.ends_with("\n\nMemória permanente: vazia."));
    assert!(sys.contains("Itens de manutenção por veículo (chaves exatas):\n- Honda CG"));
    // cache: tools+system e o fim da conversa
    assert_eq!(p1.corpo["system"][0]["cache_control"], json!({"type": "ephemeral"}));
    let ultimo = p1.corpo["messages"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(ultimo["content"].as_array().unwrap().last().unwrap()["cache_control"], json!({"type": "ephemeral"}));
    // append-only: a 2ª chamada repete a 1ª e acrescenta o assistant (byte a byte) + os tool_results
    let m2 = p2.corpo["messages"].as_array().unwrap();
    // igual tirando o marcador de cache (que só vai no fim de cada request)
    let mut m1 = p1.corpo["messages"][0].clone();
    m1["content"][0].as_object_mut().unwrap().remove("cache_control");
    assert_eq!(m2[0], m1);
    assert_eq!(m2[1], json!({"role": "assistant", "content": volta1["content"]}));
    assert_eq!(m2[2]["content"][0]["tool_use_id"], "tu_1");
    assert_eq!(m2[2]["content"][0]["is_error"], false);
    assert_eq!(m2[2]["content"][1], json!({"type": "tool_result", "tool_use_id": "tu_2",
                                           "content": "ValueError: tarefa 99 não existe", "is_error": true,
                                           "cache_control": {"type": "ephemeral"}}));
    assert!(m2[0]["content"][0]["text"].as_str().unwrap().starts_with("[agora: "));
    // a tarefa criada vira op de desfazer (a que falhou não)
    assert_eq!(bot.agenda.undo_ops, vec![json!({"op": "tarefa_criada", "id": 1})]);
}

#[test]
fn claude_tenta_de_novo_em_529_e_429_mas_nao_em_400() {
    let tmp = TempDir::new("argos_retry.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let bot = bot_de_teste(&tmp.0, &motor);
    let ok = json!({"stop_reason": "end_turn", "content": []});
    let erro = json!({"type": "error", "error": {"type": "overloaded_error"}});
    let (url, rx) = claude_falso(vec![(529, erro.clone()), (429, erro.clone()), (200, ok.clone())]);
    let mut c = crate::rede::Claude::new(crate::rede::agente(), "k");
    c.url = url;
    assert_eq!(c.create(&json!({"x": 1}), None).unwrap(), ok);
    assert_eq!(rx.try_iter().count(), 3);

    let (url, rx) = claude_falso(vec![(500, erro.clone()), (500, erro.clone()), (500, erro.clone())]);
    let mut c = crate::rede::Claude::new(crate::rede::agente(), "k");
    c.url = url;
    assert!(c.create(&json!({}), None).unwrap_err().to_string().starts_with("Error code: 500"));
    assert_eq!(rx.try_iter().count(), 3); // 1 + 2 novas tentativas

    let (url, rx) = claude_falso(vec![(400, erro)]);
    let mut c = crate::rede::Claude::new(crate::rede::agente(), "k");
    c.url = url;
    assert!(c.create(&json!({}), Some("b")).is_err());
    let p = rx.recv().unwrap();
    assert!(p.headers.contains("anthropic-beta: b"));
    drop(bot);
}

#[test]
fn corpo_das_fotos() {
    let b = Base::de(&fixture("motor"));
    let body = crate::rede::photos_body("claude-sonnet-5-5", "medium", &[vec![1, 2, 3]], &b.ctx);
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(body["output_config"]["effort"], "medium");
    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    let c = &body["messages"][0]["content"];
    assert_eq!(c[0], json!({"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "AQID"}}));
    assert!(c[1]["text"].as_str().unwrap().contains("- Honda CG 160 Titan: último odômetro registrado 4741.2 km\n- Chevrolet Onix: sem registros ainda"));
    assert!(body.get("thinking").is_none() && body.get("tool_choice").is_none());
}

// ==========================================
// Memória, linhas prontas e busca sem acento (port do argos_test.py novo)
// ==========================================

#[test]
fn linha_pronta() {
    use crate::agenda::agenda_line;
    let e = json!({"summary": "Estágio", "start": {"dateTime": "2026-10-05T10:30:00-03:00"}, "end": {"dateTime": "2026-10-09T12:30:00-03:00"}});
    assert_eq!(agenda_line(&e).unwrap(), "seg 05/10 · 10h30 Estágio (até 09/10)");
    let e = json!({"summary": "Entomologia", "start": {"dateTime": "2026-10-05T08:00:00-03:00"}, "end": {"dateTime": "2026-10-05T10:00:00-03:00"}});
    assert_eq!(agenda_line(&e).unwrap(), "seg 05/10 · 08h Entomologia");
}

#[test]
fn lembrar_revisar() {
    let mut st = estado();
    let now = at("2026-10-01T10:00");
    let f = remember(&mut st, " Zé = apelido do chefe ", &now, None);
    assert_eq!(f.texto, "Zé = apelido do chefe");
    assert!(memory_block(&st, "dele").contains("[1] Zé"));
    assert_eq!(facts_to_review(&st, &at("2026-10-14T10:00"), 14.0).unwrap(), vec![]);
    assert_eq!(facts_to_review(&st, &at("2026-10-15T10:00"), 14.0).unwrap(), vec![f]);
}

#[test]
fn validade() {
    let mut st = estado();
    let now = at("2026-10-01T10:00");
    remember(&mut st, "Estágio é evento na facul", &now, Some("2026-10-09".into()));
    remember(&mut st, "Zé = chefe", &now, None);
    assert!(memory_block(&st, "dele").contains("(até 09/10)"));
    assert!(prune_expired(&mut st, &at("2026-10-09T23:59")).is_empty());
    let gone = prune_expired(&mut st, &at("2026-10-10T00:01"));
    assert_eq!(gone.iter().map(|f| f.texto.as_str()).collect::<Vec<_>>(), ["Estágio é evento na facul"]);
    assert_eq!(st.memoria.len(), 1);
    // temporário não entra na revisão quinzenal
    remember(&mut st, "x", &now, Some("2026-12-01".into()));
    let r = facts_to_review(&st, &at("2026-11-01T10:00"), 14.0).unwrap();
    assert_eq!(r.iter().map(|f| f.texto.as_str()).collect::<Vec<_>>(), ["Zé = chefe"]);
}

#[test]
fn busca_sem_acento() {
    use crate::agenda::fold;
    assert!(fold("Estágio na empresa").contains(&fold("estagio")));
    assert_eq!(fold("ÁÇÃO"), "acao");
}

/// listar_eventos (intervalo, busca local sem acento, linhas_prontas sem 'Sono') e lembrar com evento_ref,
/// contra um Google falso em 127.0.0.1
#[test]
fn listar_eventos_e_lembrar_com_evento() {
    let tmp = TempDir::new("argos_gcal.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let itens = json!({"items": [
        {"id": "1", "summary": "Estágio", "start": {"dateTime": "2026-10-05T10:30:00-03:00"}, "end": {"dateTime": "2026-10-09T12:30:00-03:00"}},
        {"id": "2", "summary": "Sono", "start": {"dateTime": "2026-10-05T23:00:00-03:00"}, "end": {"dateTime": "2026-10-06T07:00:00-03:00"}, "description": "estagio"},
        {"id": "3", "summary": "Feriado", "start": {"date": "2026-10-12"}, "end": {"date": "2026-10-14"}, "location": "casa do ESTAGIO"},
        {"id": "4", "summary": "Dentista", "start": {"dateTime": "2026-10-07T15:00:00-03:00"}, "end": {"dateTime": "2026-10-07T16:00:00-03:00"}}]});
    let (url, rx) = claude_falso(vec![
        (200, json!({"access_token": "tok", "expires_in": 3600})),
        (200, itens.clone()),
        (200, itens.clone()),
        (200, itens),
    ]);
    let base = url.trim_end_matches("/v1/messages").to_string();
    let cred = tmp.0.join("cred");
    std::fs::write(cred.join("google_client.json"), json!({"installed": {"client_id": "x", "client_secret": "y", "token_uri": format!("{base}/token")}}).to_string()).unwrap();
    bot.gcal = Some(crate::gcal::GoogleCalendar::new(crate::rede::agente(), &cred).unwrap());
    bot.gcal.as_mut().unwrap().base = base;

    let r: Value = serde_json::from_str(&bot.run_tool("listar_eventos", &json!({"inicio": "2026-10-01", "fim": "2026-10-20", "busca": "ESTAGIO"})).unwrap()).unwrap();
    assert_eq!(r["linhas_prontas"], json!(["seg 05/10 · 10h30 Estágio (até 09/10)", "12/10 · dia inteiro: Feriado (até 13/10)"]));
    assert_eq!(r["eventos"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect::<Vec<_>>(), ["1", "2", "3"]);
    let token = rx.recv().unwrap();
    assert!(token.headers.starts_with("post /token"));
    let lista = rx.recv().unwrap();
    assert!(lista.headers.contains("maxresults=250"), "{}", lista.headers.lines().next().unwrap());
    assert!(lista.headers.contains("timemax=2026-10-21t00%3a00%3a00-03%3a00")); // 'fim' só data → +1 dia
    assert!(!lista.headers.lines().next().unwrap().contains("&q="));
    assert!(lista.headers.contains("authorization: bearer tok"));

    // evento com hora: validade = dia do fim; dia inteiro: fim exclusivo menos 1
    bot.run_tool("lembrar", &json!({"texto": "Zé é o chefe no estágio", "valido_ate": null, "evento_ref": "estagio"})).unwrap();
    bot.run_tool("lembrar", &json!({"texto": "feriado na casa", "valido_ate": "2026-01-01", "evento_ref": "feriado"})).unwrap();
    assert_eq!(bot.state.memoria.iter().map(|f| f.ate.clone().unwrap()).collect::<Vec<_>>(), ["2026-10-09", "2026-10-13"]);
    assert_eq!(bot.agenda.actions, ["🧠 lembrei: Zé é o chefe no estágio (até 09/10)", "🧠 lembrei: feriado na casa (até 13/10)"]);
    // corrigir e esquecer
    bot.run_tool("corrigir_memoria", &json!({"id": 1, "texto": "Zé = chefe", "valido_ate": null})).unwrap();
    bot.run_tool("esquecer", &json!({"id": 2})).unwrap();
    assert_eq!(bot.state.memoria.len(), 1);
    assert_eq!((bot.state.memoria[0].texto.as_str(), bot.state.memoria[0].ate.clone()), ("Zé = chefe", None));
    assert_eq!(bot.agenda.actions[2..], ["🧠 corrigi: Zé = chefe", "🧠 esqueci: feriado na casa"]);
    let erro = bot.run_tool("esquecer", &json!({"id": 9})).unwrap_err();
    assert_eq!(format!("{}: {erro}", erro.tipo()), "ValueError: fato 9 não existe na memória");
    let salvo: Value = serde_json::from_str(&std::fs::read_to_string(&bot.state_path).unwrap()).unwrap();
    assert_eq!(salvo["memoria"][0]["texto"], "Zé = chefe");
}

// ==========================================
// Desfazer (port do TestDesfazer + fluxo real dos botões)
// ==========================================

use crate::desfazer::{push_undo, remove_rows, row_value, UNDO_KEEP};

/// cópia do MotorCostTracker + o CSV original em bytes
fn base_desfazer() -> (Base, PathBuf, Vec<u8>) {
    let b = Base::new();
    let csv = b.mr.caminho(&b.ctx.v(MOTO).csv);
    let original = std::fs::read(&csv).unwrap();
    (b, csv, original)
}

#[test]
fn abastecimento_volta_byte_a_byte() {
    let (b, csv, original) = base_desfazer();
    let d = b.draft(b.reading());
    save(&b.mr, &d, &b.ctx, &b.mconfig).unwrap();
    assert_ne!(std::fs::read(&csv).unwrap(), original);
    assert!(remove_rows(&csv, &[row_value(&csv_row(&d).unwrap())]).unwrap());
    assert_eq!(std::fs::read(&csv).unwrap(), original);
}

#[test]
fn manutencao_via_bot_undo() {
    let (b, csv, original) = base_desfazer();
    let mut st = estado();
    let (_, _, rows) = record_maintenance(&b.mr, &b.mconfig, MOTO, &["calibrar_pneus".into(), "limpar_lubrificar_corrente".into()], 20.0, "01/10/26", None).unwrap();
    let op = json!({"op": "csv", "csv": b.ctx.v(MOTO).csv, "rows": rows.iter().map(|r| row_value(r)).collect::<Vec<_>>()});
    let uid = push_undo(&mut st, "manutenção", vec![op], &at("2026-10-01T10:00"));
    let sp = b.mr.dir.join("estado_teste.json");
    let undo = |st: &mut Estado, id| crate::desfazer::undo(st, &b.mr, false, &b.mr.dir, None, &sp, id).unwrap();
    assert!(undo(&mut st, uid).contains("desfeito"));
    assert_eq!(std::fs::read(&csv).unwrap(), original);
    assert!(undo(&mut st, uid).contains("Nada pra desfazer")); // não desfaz duas vezes
}

#[test]
fn linha_sumiu_nao_mexe() {
    let (_b, csv, original) = base_desfazer();
    assert!(!remove_rows(&csv, &[json!(["01/01/99", "1", 1, "1.00", "1.00", "x"])]).unwrap());
    assert_eq!(std::fs::read(&csv).unwrap(), original);
}

#[test]
fn desfaz_tarefas_e_memoria() {
    let b = Base::new();
    let mut st = estado();
    let now = at("2026-10-01T10:00");
    let t = new_task(&mut st, "varrer", "2026-10-02T18:00", Some(7), false, "09:00").unwrap();
    let before = crate::desfazer::op_tarefa_antes(&t);
    task_done(&mut st, t.id, &now).unwrap();
    assert_ne!(st.tarefas[0].proximo, t.proximo);
    let f = remember(&mut st, "x", &now, None);
    push_undo(&mut st, "grupo", vec![before, json!({"op": "memoria_criada", "id": f.id})], &now);
    let sp = b.mr.dir.join("estado_teste.json");
    crate::desfazer::undo(&mut st, &b.mr, false, &b.mr.dir, None, &sp, None).unwrap(); // sem id = o último
    assert_eq!(st.tarefas[0].proximo, t.proximo);
    assert!(st.memoria.is_empty());
    assert!(st.desfazer.last().unwrap().desfeito);
}

#[test]
fn guarda_so_os_ultimos() {
    let mut st = estado();
    let now = at("2026-10-01T10:00");
    for i in 0..25 {
        push_undo(&mut st, &i.to_string(), vec![json!({"op": "tarefa_criada", "id": 0})], &now);
    }
    assert_eq!(st.desfazer.len(), UNDO_KEEP);
    assert_eq!(st.desfazer[0].desc, "5");
    assert_eq!(push_undo(&mut st, "vazio", vec![], &now), None);
}

/// fluxo de verdade: botão de manutenção em 2 passos → grava → ↩️ desfazer → CSV volta byte a byte
#[test]
fn botoes_manutencao_e_desfazer_ponta_a_ponta() {
    let tmp = TempDir::new("argos_botoes.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let csv = motor.join("veiculos/titan_22.csv");
    let original = std::fs::read(&csv).unwrap();
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let ok = json!({"ok": true, "result": {"message_id": 7}});
    let (url, rx) = claude_falso(vec![(200, ok.clone()); 12]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    let toque = |bot: &mut crate::bot::Bot, data: &str| {
        bot.handle(&json!({"update_id": 1, "callback_query": {"id": "q", "from": {"id": 1}, "data": data}})).unwrap();
        let answer = rx.recv().unwrap();
        assert!(answer.headers.starts_with("post /answercallbackquery"));
        rx.recv().unwrap().corpo
    };
    // 1º toque: só pergunta
    let m = toque(&mut bot, "mnt:0:calibrar_pneus");
    // km estimada pra hoje (data real): a fixture para em 22/09, então sempre sai acima da última leitura
    let (km, estimado) = crate::tarefas::maintenance_odometer(&bot.mr, "veiculos/titan_22.csv", crate::util::agora().date_naive()).unwrap();
    assert!(estimado && km > 4741.2);
    let km = crate::util::fmt_g(km);
    assert_eq!(m["text"], format!("Gravar <b>Calibrar pneus</b> feito hoje (~{km} km)?"));
    assert_eq!(m["reply_markup"]["inline_keyboard"], json!([[{"text": "✅ gravar", "callback_data": "mnk:0:calibrar_pneus"}, {"text": "cancelar", "callback_data": "mnx"}]]));
    assert_eq!(std::fs::read(&csv).unwrap(), original);
    assert_eq!(toque(&mut bot, "mnx")["text"], "👍 nada gravado");
    // confirma: grava e oferece desfazer
    let m = toque(&mut bot, "mnk:0:calibrar_pneus");
    assert_eq!(m["text"], format!("🔧 registrado: Calibrar pneus (~{km} km) <i>git desligado</i>"));
    let und = m["reply_markup"]["inline_keyboard"][0][0]["callback_data"].as_str().unwrap().to_string();
    assert_eq!(und, "und:1");
    assert!(std::fs::read_to_string(&csv).unwrap().ends_with(&format!(",{km},2,0.00,0.00,calibrar_pneus\r\n")));
    let salvo: Value = serde_json::from_str(&std::fs::read_to_string(&bot.state_path).unwrap()).unwrap();
    assert_eq!(salvo["desfazer"][0]["ops"][0]["rows"][0][2], json!(2));
    assert_eq!(salvo["desfazer"][0]["ops"][0]["veiculo"], MOTO);
    // desfaz
    assert_eq!(toque(&mut bot, &und)["text"], "↩️ desfeito: manutenção Calibrar pneus · git desligado");
    assert_eq!(std::fs::read(&csv).unwrap(), original);
    assert_eq!(toque(&mut bot, &und)["text"], "Nada pra desfazer (ou já foi desfeito).");
}

/// manutenção pela agenda: sem km dita, só grava com o botão; data no passado pode ficar abaixo da última leitura
#[test]
fn manutencao_da_agenda_com_km_estimada_pede_botao() {
    let tmp = TempDir::new("argos_mnc.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let csv = motor.join("veiculos/titan_22.csv");
    let original = std::fs::read(&csv).unwrap();
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let ok = json!({"ok": true, "result": {"message_id": 7}});
    let (url, rx) = claude_falso(vec![(200, ok.clone()); 12]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    let toque = |bot: &mut crate::bot::Bot, data: &str| {
        bot.handle(&json!({"update_id": 1, "callback_query": {"id": "q", "from": {"id": 1}, "data": data}})).unwrap();
        assert!(rx.recv().unwrap().headers.starts_with("post /answercallbackquery"));
        rx.recv().unwrap().corpo
    };
    // 15/09: entre 10/09 (4497.9) e 21/09 (4741.2) → 4608.5, abaixo da última leitura (4741.2)
    let pede = json!({"veiculo": MOTO, "itens": ["limpar_lubrificar_corrente"], "data": "2026-09-15"});
    let r = bot.run_tool("registrar_manutencao", &pede).unwrap();
    assert!(r.starts_with("Botão de confirmação mostrado ao usuário: Lubrif. corrente em 15/09/26 com ~4608.5 km"), "{r}");
    assert_eq!(std::fs::read(&csv).unwrap(), original); // nada gravado ainda
    assert_eq!(toque(&mut bot, "mnc:no")["text"], "👍 nada gravado");
    assert_eq!(toque(&mut bot, "mnc:yes")["text"], "Esse pedido já foi resolvido.");
    bot.run_tool("registrar_manutencao", &pede).unwrap();
    let m = toque(&mut bot, "mnc:yes");
    assert_eq!(m["text"], "🔧 registrado: Lubrif. corrente em 15/09/26 (~4608.5 km) <i>git desligado</i>");
    assert!(std::fs::read_to_string(&csv).unwrap().ends_with("15/09/26,4608.5,2,0.00,0.00,limpar_lubrificar_corrente\r\n"));
    let und = m["reply_markup"]["inline_keyboard"][0][0]["callback_data"].as_str().unwrap().to_string();
    toque(&mut bot, &und);
    assert_eq!(std::fs::read(&csv).unwrap(), original);
    // km dita: grava direto, sem botão
    let r = bot.run_tool("registrar_manutencao", &json!({"veiculo": MOTO, "itens": ["calibrar_pneus"], "custo_total": 5, "odometro": 3800})).unwrap();
    assert!(r.contains("\"odometro_usado\": 3800.0") && r.contains("\"odometro_estimado\": false"), "{r}");
    assert!(bot.agenda.pending_maint.is_none());
    assert!(std::fs::read_to_string(&csv).unwrap().ends_with(",3800,2,5.00,0.00,calibrar_pneus\r\n"));
    // data no futuro e item fora da lista: erro pro modelo, nada pendente
    let amanha = (crate::util::agora().date_naive() + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
    assert!(bot.run_tool("registrar_manutencao", &json!({"veiculo": MOTO, "itens": ["calibrar_pneus"], "data": amanha})).is_err());
    assert!(bot.run_tool("registrar_manutencao", &json!({"veiculo": MOTO, "itens": ["trocar_retrovisor"]})).is_err());
    assert!(bot.agenda.pending_maint.is_none());
}

/// abastecimento de verdade: botão Salvar → relatório com ↩️ → desfazer → CSV volta byte a byte
#[test]
fn salvar_abastecimento_e_desfazer_ponta_a_ponta() {
    let tmp = TempDir::new("argos_salvar.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let csv = motor.join("veiculos/titan_22.csv");
    let original = std::fs::read(&csv).unwrap();
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 8]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    let b = Base::de(&motor);
    bot.draft = Some(b.draft(b.reading()));
    bot.context = Some(vehicle_context(&bot.mr, &bot.mr.load_config().unwrap()).unwrap());
    bot.draft_msg = Some(5);
    let toque = |bot: &mut crate::bot::Bot, data: &str| bot.handle(&json!({"update_id": 1, "callback_query": {"id": "q", "from": {"id": 1}, "data": data}})).unwrap();

    toque(&mut bot, "save");
    let pedidos: Vec<Pedido> = (0..4).map(|_| rx.recv().unwrap()).collect();
    assert!(pedidos[1].headers.starts_with("post /editmessagetext"));
    assert!(pedidos[1].corpo["text"].as_str().unwrap().ends_with("\n\n✅ salvo"));
    let m = &pedidos[2].corpo;
    // resumo de consumo/custo, sem o bloco de manutenção (que vem com botões logo depois)
    let t = m["text"].as_str().unwrap();
    assert!(t.starts_with("✅ salvo\n<pre>--- Resumo ---"), "{t}");
    assert!(!t.contains("Manutenções pendentes"), "{t}");
    assert_eq!(m["reply_markup"]["inline_keyboard"][0][0], json!({"text": "↩️ desfazer", "callback_data": "und:1"}));
    // manutenção vencida igual ao aviso das 06:00: um ✅ por item
    let mnt = &pedidos[3].corpo;
    assert!(mnt["text"].as_str().unwrap().starts_with("🔧 <b>Honda CG 160 Titan</b> — manutenção vencida:\n• "));
    assert!(mnt["reply_markup"]["inline_keyboard"][0][0]["callback_data"].as_str().unwrap().starts_with("mnt:"));
    assert!(bot.state.avisos_manutencao.contains_key("Honda CG 160 Titan"));
    assert_ne!(std::fs::read(&csv).unwrap(), original);
    assert!(bot.draft.is_none());

    toque(&mut bot, "und:1");
    let _ = rx.recv().unwrap();
    assert_eq!(rx.recv().unwrap().corpo["text"], "↩️ desfeito: abastecimento Honda CG 160 Titan 01/10/26 · git desligado");
    assert_eq!(std::fs::read(&csv).unwrap(), original);
}

/// ver_manutencao: check_maintenance anotado com atraso e última vez
#[test]
fn manutencao_anotada() {
    let fix = fixture("motor");
    let mr = Motoregis::new(&fix);
    let c = "veiculos/titan_22.csv";
    let j = "veiculos/titan_22.json";
    let report = mr.check_maintenance(c, j, mr.get_last_odometer(c).unwrap(), "01/10/26").unwrap();
    let ours = crate::tarefas::annotate_maintenance(report.trim_end(), &mr.read_csv(c).unwrap(), &mr.load_json(j).unwrap().unwrap());
    assert!(ours.contains("      atraso: 84 dias (feito há 99 dias, prazo a cada 15 dias)"), "{ours}");
    assert!(ours.contains("      última vez: 24/06/26 com 2689.5 km"), "{ours}");
    // 23/04 → 01/10 são 5 meses completos: filtro e óleo (6 meses) ainda não venceram
    assert!(!ours.contains("Filtro de ar") && !ours.contains("Óleo e filtro"), "{ours}");
}

#[test]
fn atalhos_sem_modelo() {
    use crate::atalhos::Atalho;
    use chrono::NaiveDate;
    let qui = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(); // quinta
    let d = |m, dd| NaiveDate::from_ymd_opt(2026, m, dd).unwrap();
    let ag = |t: &str| match atalho(t, qui) { Some(Atalho::Agenda { inicio, dias, .. }) => Some((inicio, dias)), _ => None };
    assert_eq!(ag("o que tenho amanhã?"), Some((d(10, 2), 1)));
    assert_eq!(ag("Oq tenho hoje"), Some((qui, 1)));
    assert_eq!(ag("o que tem na agenda pra sexta?"), Some((d(10, 2), 1)));
    assert_eq!(ag("o que eu tenho quinta"), Some((qui, 1)));
    assert_eq!(ag("o que tenho segunda-feira"), Some((d(10, 5), 1)));
    assert_eq!(ag("agenda da semana"), Some((qui, 4)));
    assert_eq!(ag("o que tenho semana que vem"), Some((d(10, 5), 7)));
    assert_eq!(ag("/amanha"), Some((d(10, 2), 1)));
    assert_eq!(ag("/agenda"), Some((qui, 7)));
    let manut = |t: &str| match atalho(t, qui) { Some(Atalho::Manutencao { filtro }) => Some(filtro), _ => None };
    assert_eq!(manut("A quanto tempo tava atrasada a calibragem?"), Some(vec!["calibrar"]));
    assert_eq!(manut("o que tá vencido na moto?"), Some(vec![]));
    assert_eq!(manut("quando lubrifiquei a corrente pela última vez?"), Some(vec!["lubrificar", "corrente"]));
    assert_eq!(manut("/manutencao"), Some(vec![]));
    assert_eq!(atalho("desfaz", qui), Some(Atalho::Desfazer));
    assert_eq!(atalho("Desfaz o último!", qui), Some(Atalho::Desfazer));
    assert_eq!(atalho("minhas tarefas", qui), Some(Atalho::Tarefas));
    // o que grava ou é ambíguo vai pro modelo
    for t in ["lubrifiquei a corrente", "calibrei os pneus hoje", "marca dentista amanhã 15h",
              "me lembra de trocar o óleo amanhã", "o que tenho amanhã e depois cancela a reunião",
              "não era isso, desfaz", "empurra pra 17h", "Zé é o apelido do meu chefe",
              "agenda a troca de óleo pra sexta", "tenho prova amanhã?"] {
        assert_eq!(atalho(t, qui), None, "{t}");
    }
}

#[test]
fn manutencao_chat_filtra_e_formata() {
    use crate::atalhos::manutencao_chat;
    let anotado = "Honda CG 160 Titan (último registro 4741.2 km, hoje 01/10/26):\n\n--- Manutenções pendentes ---\n\n  ⚠️  Calibrar pneus\n      última vez: 24/06/26 com 2689.5 km\n      atraso: 84 dias (feito há 99 dias, prazo a cada 15 dias)\n  ⚠️  Lubrif. corrente\n      última vez: 12/09/26 com 4741.2 km\n      atraso: 4 dias (feito há 19 dias, prazo a cada 15 dias)";
    let (t, achou) = manutencao_chat(anotado, Some(&["Calibrar pneus".to_string()]));
    assert!(achou);
    assert!(t.contains("⚠️ <b>Calibrar pneus</b>\n   última vez: 24/06/26") && !t.contains("Lubrif"), "{t}");
    let (t, _) = manutencao_chat(anotado, None);
    assert!(t.starts_with("🔧 <b>Honda CG 160 Titan (último registro 4741.2 km, hoje 01/10/26)</b>") && t.contains("Lubrif"), "{t}");
    assert!(!manutencao_chat(anotado, Some(&["Fluido de freios".to_string()])).1);
}

#[test]
fn carimbo_traz_os_proximos_dias() {
    let c = crate::agenda::carimbo(&at("2026-10-01T20:40"));
    assert_eq!(c, "[agora: quinta, 01/10/2026 20:40 | próximos: sex 02/10, sáb 03/10, dom 04/10, seg 05/10, ter 06/10, qua 07/10, qui 08/10]");
}

// ==========================================
// aviso de chuva (clima.rs)
// ==========================================

/// resposta da Open-Meteo de 01/10 e 02/10; `chuva` = (dia, hora, prob, mm), o resto 0
fn meteo(chuva: &[(u32, u32, f64, f64)]) -> Value {
    let (mut time, mut prob, mut mm) = (vec![], vec![], vec![]);
    for d in [1, 2] {
        for h in 0..24 {
            time.push(json!(format!("2026-10-{d:02}T{h:02}:00")));
            let c = chuva.iter().find(|c| c.0 == d && c.1 == h);
            prob.push(json!(c.map(|c| c.2).unwrap_or(0.0)));
            mm.push(json!(c.map(|c| c.3).unwrap_or(0.0)));
        }
    }
    json!({"timezone": "America/Sao_Paulo", "hourly": {"time": time, "precipitation_probability": prob, "precipitation": mm}})
}

#[test]
fn meses_completos() {
    use crate::motoregis::months_between;
    let d = |a, m, d| chrono::NaiveDate::from_ymd_opt(a, m, d).unwrap();
    assert_eq!(months_between(d(2026, 4, 27), d(2026, 10, 1)), 5);
    assert_eq!(months_between(d(2026, 4, 27), d(2026, 10, 27)), 6);
    assert_eq!(months_between(d(2025, 12, 31), d(2026, 1, 30)), 0);
}

#[test]
fn tabela_chuva_da_ferramenta() {
    use crate::clima::{horas, tabela_chuva};
    let sex = chrono::NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
    let h = horas(&meteo(&[(2, 9, 30.0, 0.0), (2, 14, 55.0, 1.0), (2, 15, 80.0, 2.0), (2, 20, 0.0, 0.0), (1, 15, 99.0, 9.0)])).unwrap();
    let t = tabela_chuva(&h, sex, 10, 50.0);
    assert_eq!(t["periodos_acima_do_limiar"], json!([{"faixa": "14h–16h", "prob_max": 80.0, "mm": 3.0}]));
    // 9h ficou antes do `desde`, 20h tem 0%, o dia 1 é outro dia
    assert_eq!(t["horas_com_chance"], json!([{"hora": "14h", "prob": 55.0, "mm": 1.0}, {"hora": "15h", "prob": 80.0, "mm": 2.0}]));
    assert_eq!(t["desde"], json!("10h"));
}

#[test]
fn periodos_de_chuva() {
    use crate::clima::{horas, periodos_chuva, texto_chuva, Periodo};
    let sex = chrono::NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
    // 14–16h forte, 17h 60% (buraco), 18h 80%: um período só; 21h 70% exato conta; 23h vai até meia-noite
    let h = horas(&meteo(&[(2, 14, 85.0, 4.4), (2, 15, 90.0, 1.0), (2, 16, 75.0, 0.2), (2, 17, 60.0, 0.1), (2, 18, 80.0, 0.3),
                          (2, 21, 70.0, 0.0), (2, 23, 72.0, 0.5), (1, 15, 99.0, 9.0)])).unwrap();
    let p = periodos_chuva(&h, sex, 0, 70.0);
    assert_eq!(p, vec![
        Periodo { de_h: 14, ate_h: 19, prob_max: 90.0, mm_total: 4.4 + 1.0 + 0.2 + 0.1 + 0.3 },
        Periodo { de_h: 21, ate_h: 24, prob_max: 72.0, mm_total: 0.5 },
    ]);
    assert_eq!(texto_chuva("amanhã", &p), "🌧️ <b>Chuva amanhã</b>\n• 14h–19h — até 90%, ~6 mm\n• 21h–meia-noite — até 72%, ~0.5 mm");
    // a partir das 16h corta o começo; limiar acima de tudo → nada
    assert_eq!(periodos_chuva(&h, sex, 16, 70.0)[0].de_h, 16);
    assert!(periodos_chuva(&h, sex, 0, 95.0).is_empty());
    // null da API vira 0
    let mut r = meteo(&[]);
    r["hourly"]["precipitation_probability"][3] = Value::Null;
    assert_eq!(horas(&r).unwrap()[3].prob, 0.0);
    assert_eq!(crate::clima::resumo_sem_chuva("amanhã", &h, sex, 19), "☀️ amanhã sem chuva forte prevista (máx 72% às 23h)");
}

#[test]
fn aviso_de_chuva_noite_e_manha() {
    let tmp = TempDir::new("argos_chuva.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    // sem coordenadas: desligado
    bot.config.clima_lat = None;
    assert!(!bot.rain_tick(&at("2026-10-01T21:00")));
    bot.config.clima_lat = Some(-23.5);
    bot.config.clima_lon = Some(-46.6);
    let ok = json!({"ok": true, "result": {"message_id": 7}});
    let chuva = meteo(&[(2, 7, 80.0, 1.0), (2, 8, 90.0, 2.0), (2, 17, 75.0, 0.4)]);
    let (url, rx) = claude_falso(vec![(200, chuva.clone()), (200, ok.clone()), (200, chuva), (200, ok)]);
    let base = url.trim_end_matches("/v1/messages").to_string();
    bot.clima.base = base.clone();
    bot.tg.base = base;
    // 20h59: nada (a manhã de 01/10 já foi marcada como vista)
    bot.state.chuva_manha_checada = Some("2026-10-01".into());
    assert!(!bot.rain_tick(&at("2026-10-01T20:59")));
    // 21h: previsão de amanhã inteira
    assert!(bot.rain_tick(&at("2026-10-01T21:00")));
    let get = rx.recv().unwrap();
    assert!(get.headers.starts_with("get /forecast?latitude=-23.5&longitude=-46.6&hourly=precipitation_probability,precipitation&timezone=america%2fsao_paulo&forecast_days=7"), "{}", get.headers); // o servidor falso põe tudo em minúscula
    assert_eq!(rx.recv().unwrap().corpo["text"], "🌧️ <b>Chuva amanhã</b>\n• 7h–9h — até 90%, ~3 mm\n• 17h–18h — até 75%, ~0.4 mm");
    // não repete no mesmo dia
    assert!(!bot.rain_tick(&at("2026-10-01T23:00")));
    // manhã seguinte, 8h: só o que falta de hoje
    assert!(bot.rain_tick(&at("2026-10-02T08:10")));
    rx.recv().unwrap();
    assert_eq!(rx.recv().unwrap().corpo["text"], "🌧️ <b>Chuva hoje</b>\n• 8h–9h — até 90%, ~2 mm\n• 17h–18h — até 75%, ~0.4 mm");
    assert!(rx.try_recv().is_err());
    assert_eq!(bot.state.chuva_manha_checada.as_deref(), Some("2026-10-02"));
}

#[test]
fn sem_chuva_nao_manda_nada() {
    let tmp = TempDir::new("argos_seco.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    bot.config.clima_lat = Some(-23.5);
    bot.config.clima_lon = Some(-46.6);
    let (url, rx) = claude_falso(vec![(200, meteo(&[(2, 14, 69.0, 0.3)]))]);
    bot.clima.base = url.trim_end_matches("/v1/messages").to_string();
    bot.state.chuva_manha_checada = Some("2026-10-01".into());
    assert!(bot.rain_tick(&at("2026-10-01T21:30")));
    rx.recv().unwrap(); // só o GET
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(rx.try_recv().is_err());
    assert_eq!(bot.state.chuva_noite_checada.as_deref(), Some("2026-10-01"));
}

#[test]
fn atalho_de_chuva() {
    use crate::atalhos::Atalho;
    use chrono::NaiveDate;
    let qui = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    let chuva = |t: &str| match atalho(t, qui) { Some(Atalho::Chuva { dia, .. }) => Some(dia), _ => None };
    let sex = NaiveDate::from_ymd_opt(2026, 10, 2).unwrap();
    assert_eq!(chuva("vai chover amanhã?"), Some(sex));
    assert_eq!(chuva("Vai chover?"), Some(qui));
    assert_eq!(chuva("será que chove hoje"), Some(qui));
    assert_eq!(chuva("previsão do tempo pra sexta"), Some(sex));
    assert_eq!(chuva("como tá o tempo amanhã?"), Some(sex));
    assert_eq!(chuva("/chuva"), Some(qui));
    assert_eq!(chuva("vai chover depois de amanhã"), Some(NaiveDate::from_ymd_opt(2026, 10, 3).unwrap()));
    for t in ["me avisa se chover amanhã", "se chover amanhã cancela a reunião", "vai chover na hora do date?", "choveu ontem"] {
        assert_eq!(atalho(t, qui), None, "{t}");
    }
}

// ==========================================
// postos (postos.rs): xlsx da ANP, ranking por custo/km, localização e cupom
// ==========================================

const PAULISTA: &str = "09308519000106"; // Vibra 6,59
const APOLLO: &str = "06182123000177"; // Ale 5,97
const AB: &str = "12345678000199"; // bandeira branca 6,19

fn xlsx_mini() -> Vec<u8> {
    std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/anp/revendas_mini.xlsx")).unwrap()
}

#[test]
fn xlsx_da_anp() {
    use crate::postos::{postos_da_planilha, xlsx_linhas};
    let linhas = xlsx_linhas(&xlsx_mini()).unwrap();
    let p = postos_da_planilha(&linhas, "PIRACICABA", "SAO PAULO").unwrap();
    // Limeira e a Piracicaba de MG ficam de fora; município/estado sem caixa nem acento
    assert_eq!(p.keys().cloned().collect::<Vec<_>>(), vec![APOLLO, PAULISTA, AB]);
    let pa = &p[PAULISTA];
    assert_eq!((pa.nome.as_str(), pa.bandeira.as_str(), pa.endereco.as_str(), pa.bairro.as_str(), pa.cep.as_str()),
               ("PAULISTA US", "VIBRA", "AVENIDA DR. PAULO DE MORAES, 1383", "PAULISTA", "13400853"));
    assert_eq!(pa.precos["GASOLINA COMUM"], crate::postos::Preco { preco: 6.59, data: "2026-09-21".into() });
    assert_eq!(pa.precos["GASOLINA ADITIVADA"].preco, 6.89);
    // sem fantasia → razão social; bairro em runs (<r><t>); sem número; &amp; desfeito; data serial
    assert_eq!(p[APOLLO].nome, "COMERCIAL DE COMBUSTÍVEIS APOLLO PIRACICABA LTDA.");
    assert_eq!(p[APOLLO].bairro, "CENTRO");
    assert_eq!(p[APOLLO].precos["ETANOL"].preco, 3.49);
    assert_eq!((p[AB].nome.as_str(), p[AB].endereco.as_str()), ("A & B", "RUA DO PORTO"));
    assert_eq!(p[AB].precos["GASOLINA COMUM"].data, "2026-09-22");
    assert_eq!(crate::postos::nome_posto(&p[AB]), "A & B");
    assert_eq!(crate::postos::nome_posto(&p[APOLLO]), "Apollo Piracicaba (Ale)");
    assert_eq!(crate::postos::cnpj_fmt(PAULISTA), "09.308.519/0001-06");
    assert_eq!(crate::postos::cnpj("09.308.519/0001-06").as_deref(), Some(PAULISTA));
    assert_eq!(crate::postos::cnpj("123"), None);
}

#[test]
fn mesclar_guarda_coordenada_e_preco_antigo() {
    use crate::postos::{mesclar, postos_da_planilha, xlsx_linhas, Preco, Postos};
    let novos = postos_da_planilha(&xlsx_linhas(&xlsx_mini()).unwrap(), "PIRACICABA", "SAO PAULO").unwrap();
    let mut base = Postos::default();
    let mut velho = novos[PAULISTA].clone();
    velho.precos.insert("ETANOL".into(), Preco { preco: 4.19, data: "2026-09-14".into() });
    velho.precos.insert("GASOLINA COMUM".into(), Preco { preco: 6.79, data: "2026-09-14".into() });
    (velho.lat, velho.lon, velho.coord) = (Some(-22.7), Some(-47.6), "voce".into());
    base.postos.insert(PAULISTA.into(), velho);
    let mut sumiu = novos[AB].clone();
    sumiu.nome = "SAIU DA AMOSTRA".into();
    base.postos.insert("99999999000199".into(), sumiu);
    mesclar(&mut base, "2026-09-20_2026-09-26", novos);
    let pa = &base.postos[PAULISTA];
    assert_eq!((pa.lat, pa.lon, pa.coord.as_str()), (Some(-22.7), Some(-47.6), "voce"));
    assert_eq!(pa.precos["GASOLINA COMUM"].preco, 6.59); // preço novo por cima
    assert_eq!(pa.precos["ETANOL"].preco, 4.19); // produto que não veio esta semana fica
    assert!(base.postos.contains_key("99999999000199"));
    assert_eq!(base.semana, "2026-09-20_2026-09-26");
}

/// abastecimentos cheios a cada 300 km com 10 L (30 km/L), menos onde indicado
fn csv_postos() -> Vec<Vec<String>> {
    let l = |odo: &str, litros: &str, det: &str| vec!["01/09/26".into(), odo.into(), "1".into(), "60.00".into(), litros.into(), det.into()];
    let p = |c: &str| format!("gasolina_comum|cheio|posto={c}");
    vec![
        vec!["data".into(), "odometro".into(), "categoria".into(), "custo".into(), "litros".into(), "detalhes".into()],
        l("1000", "10.00", &p(PAULISTA)),
        l("1300", "10.00", &p(APOLLO)),   // trecho da PAULISTA: 30 km/L
        l("1620", "10.00", &p(PAULISTA)), // trecho da APOLLO: 32
        l("1920", "10.00", &p(PAULISTA)), // PAULISTA: 30
        l("2220", "10.00", "gasolina_comum|cheio|gap"), // gap: não conta
        vec!["02/09/26".into(), "2230".into(), "2".into(), "0.00".into(), "0.00".into(), "calibrar_pneus".into()], // manutenção no meio
        l("2500", "10.00", &p(PAULISTA)), // trecho sem posto (o anterior não tinha tag): 28
        l("2810", "10.00", "gasolina_comum|parcial"), // parcial: quebra o par
        l("3100", "10.00", &p(PAULISTA)),
        l("3400", "10.00", "etanol|cheio"), // PAULISTA: 30
        l("3600", "10.00", "gasolina_comum|cheio"), // trecho de etanol: não entra na gasolina
    ]
}

#[test]
fn trechos_creditados_ao_posto_de_saida() {
    let t = crate::postos::trechos(&csv_postos(), "gasolina_comum");
    let p = |c: &str, k: f64| (Some(c.to_string()), k);
    assert_eq!(t, vec![p(PAULISTA, 30.0), p(APOLLO, 32.0), p(PAULISTA, 30.0), (None, 28.0), p(PAULISTA, 30.0)]);
    assert_eq!(crate::postos::trechos(&csv_postos(), "etanol"), vec![(None, 20.0)]);
}

fn postos_teste() -> crate::postos::Postos {
    let mut p = crate::postos::Postos::default();
    p.postos = crate::postos::postos_da_planilha(&crate::postos::xlsx_linhas(&xlsx_mini()).unwrap(), "PIRACICABA", "SAO PAULO").unwrap();
    // PAULISTA a ~1,1 km da ponto, APOLLO a ~3 km, A & B sem coordenada
    for (c, lat, lon) in [(PAULISTA, -22.7180, -47.6310), (APOLLO, -22.7226, -47.6526)] {
        let x = p.postos.get_mut(c).unwrap();
        (x.lat, x.lon) = (Some(lat), Some(lon));
    }
    p.semana = "2026-09-20_2026-09-26".into();
    p
}

#[test]
fn ranking_por_custo_por_km() {
    use crate::postos::{ranking, texto_ranking, Fonte};
    let p = postos_teste();
    let r = ranking(&csv_postos(), &p, "gasolina_comum", None, 5.0);
    let geral = (30.0 + 32.0 + 30.0 + 28.0 + 30.0) / 5.0; // 30
    // PAULISTA tem 3 trechos → km/L dela; APOLLO só 1 e a Ale não tem outros → média geral; A & B idem
    let resumo: Vec<(&str, Fonte, usize)> = r.iter().map(|o| (o.cnpj.as_str(), o.fonte.clone(), o.n)).collect();
    assert_eq!(resumo, vec![(APOLLO, Fonte::Geral, 5), (AB, Fonte::Geral, 5), (PAULISTA, Fonte::Posto, 3)]);
    assert!((r[0].custo_100.unwrap() - 5.97 / geral * 100.0).abs() < 1e-9);
    assert!((r[2].custo_100.unwrap() - 6.59 / 30.0 * 100.0).abs() < 1e-9);
    // etanol: só a APOLLO vende; o trecho de etanol (20 km/L) vira a média
    let e = ranking(&csv_postos(), &p, "etanol", None, 5.0);
    assert_eq!((e.len(), e[0].km_l), (1, Some(20.0)));
    // perto da ponto: só quem tem coordenada e está no raio
    let ponto = (-22.708, -47.631);
    let perto = ranking(&csv_postos(), &p, "gasolina_comum", Some(ponto), 2.0);
    assert_eq!(perto.iter().map(|o| o.cnpj.as_str()).collect::<Vec<_>>(), vec![PAULISTA]);
    let t = texto_ranking(&perto, "gasolina_comum", 5, true, 2.0);
    assert_eq!(t, "⛽ <b>Onde abastecer</b> · gasolina comum\n\
                   1. <b>Paulista US (BR)</b> — R$ 6,59 → R$ 21,97/100 km · 1,1 km · 30,0 km/L aqui\n\
                   \n<i>ANP de 21/09</i>");
    let t = texto_ranking(&r, "gasolina_comum", 5, false, 5.0);
    assert!(t.contains("2. <b>A &amp; B</b> — R$ 6,19 → R$ 20,63/100 km\n"), "{t}");
    assert!(t.contains("1. <b>Apollo Piracicaba (Ale)</b> — R$ 5,97 → R$ 19,90/100 km\n"), "{t}");
    assert_eq!(texto_ranking(&[], "gasolina_comum", 5, true, 5.0), "⛽ <b>Onde abastecer</b> · gasolina comum\nNenhum posto da pesquisa da ANP a até 5 km daqui.");
    // bandeira com 3+ trechos vale pra posto da mesma bandeira sem histórico próprio
    let mut p2 = p.clone();
    let mut irmao = p2.postos[PAULISTA].clone();
    irmao.nome = "OUTRO VIBRA".into();
    p2.postos.insert("33333333000133".into(), irmao);
    let r2 = ranking(&csv_postos(), &p2, "gasolina_comum", None, 5.0);
    let o = r2.iter().find(|o| o.cnpj == "33333333000133").unwrap();
    assert_eq!((o.fonte.clone(), o.n, o.km_l), (Fonte::Bandeira, 3, Some(30.0)));
}

#[test]
fn veiculo_e_combustivel_mais_usados() {
    let pouco = vec![csv_postos()[0].clone(), vec!["x".into(), "1".into(), "1".into(), "1".into(), "1".into(), "etanol|cheio".into()]];
    let r = crate::postos::mais_usado(&[("Onix".into(), pouco), ("Moto".into(), csv_postos())]).unwrap();
    assert_eq!((r.0.as_str(), r.1.as_str(), r.2), ("Moto", "gasolina_comum", 10));
    assert_eq!(crate::postos::dist_fmt(0.34), "350 m");
    assert_eq!(crate::postos::dist_fmt(1.26), "1,3 km");
}

#[test]
fn cupom_vira_tag_no_csv() {
    let b = Base::new();
    let d = b.draft(Leitura { cnpj_posto: Some("09.308.519/0001-06".into()), ..b.reading() });
    assert_eq!(d.posto.as_deref(), Some(PAULISTA));
    assert_eq!(csv_row(&d).unwrap()[5], "gasolina_comum|cheio|posto=09308519000106");
    assert!(render(&d, &b.ctx, 0.03, 3000.0).contains("\n⛽ CNPJ 09.308.519/0001-06"));
    let l = Leitura::de_json(&json!({"cnpj_posto": "09.308.519/0001-06"}));
    assert_eq!(l.cnpj_posto.as_deref(), Some(PAULISTA));
    assert_eq!(Leitura::de_json(&json!({"cnpj_posto": "ilegível"})).cnpj_posto, None);
    let s = crate::rede::extraction_schema(&b.ctx.nomes(), &b.ctx.fuel_types);
    assert_eq!(s["properties"]["cnpj_posto"], json!({"anyOf": [{"type": "string"}, {"type": "null"}]}));
    assert!(s["required"].as_array().unwrap().contains(&json!("cnpj_posto")));
    assert!(crate::rede::build_prompt(&b.ctx).contains("leia o CNPJ do emitente"));
}

/// localização com rascunho sem posto → botões dos mais próximos → escolhe → coordenada fixada →
/// salva com posto= no CSV; localização sem rascunho → ranking por perto
#[test]
fn localizacao_escolhe_posto_e_salva() {
    let tmp = TempDir::new("argos_postos.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let csv = motor.join("veiculos/titan_22.csv");
    let mut bot = bot_de_teste(&tmp.0, &motor);
    crate::postos::gravar(&postos_teste(), &bot.postos_path).unwrap();
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 10]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    let b = Base::de(&motor);
    bot.draft = Some(b.draft(b.reading()));
    bot.context = Some(vehicle_context(&bot.mr, &bot.mr.load_config().unwrap()).unwrap());
    bot.draft_msg = Some(5);
    let aqui = (-22.7181, -47.6311);
    bot.handle(&json!({"update_id": 1, "message": {"chat": {"id": 1}, "date": 0, "location": {"latitude": aqui.0, "longitude": aqui.1}}})).unwrap();
    let m = rx.recv().unwrap().corpo;
    assert_eq!(m["text"], "⛽ Qual desses?");
    assert_eq!(m["reply_markup"]["inline_keyboard"], json!([
        [{"text": "Paulista US (BR) · 0 m", "callback_data": format!("pst:{PAULISTA}")}],
        [{"text": "Apollo Piracicaba (Ale) · 2,3 km", "callback_data": format!("pst:{APOLLO}")}],
        [{"text": "nenhum desses", "callback_data": "pst:-"}],
    ]));
    let toque = |bot: &mut crate::bot::Bot, data: &str| bot.handle(&json!({"update_id": 2, "callback_query": {"id": "q", "from": {"id": 1}, "data": data}})).unwrap();
    toque(&mut bot, &format!("pst:{PAULISTA}"));
    let _ = rx.recv().unwrap(); // answerCallbackQuery
    assert!(rx.recv().unwrap().corpo["text"].as_str().unwrap().contains("\n⛽ Paulista US (BR)"));
    let salvo = crate::postos::carregar(&bot.postos_path).unwrap();
    let pa = &salvo.postos[PAULISTA];
    assert_eq!((pa.lat, pa.lon, pa.coord.as_str()), (Some(aqui.0), Some(aqui.1), "voce"));
    toque(&mut bot, "save");
    for _ in 0..4 {
        rx.recv().unwrap();
    }
    assert!(std::fs::read_to_string(&csv).unwrap().ends_with("|posto=09308519000106\r\n"));
    // sem rascunho: localização = "onde abastecer aqui perto"
    bot.handle(&json!({"update_id": 3, "message": {"chat": {"id": 1}, "date": 0, "location": {"latitude": aqui.0, "longitude": aqui.1}}})).unwrap();
    let t = rx.recv().unwrap().corpo["text"].as_str().unwrap().to_string();
    // sem histórico por posto (o fixture não tem tag), todos usam a média: ganha o preço menor
    assert!(t.starts_with("⛽ <b>Onde abastecer</b> · gasolina comum\n1. <b>Apollo Piracicaba (Ale)</b> — R$ 5,97 → "), "{t}");
    assert!(t.contains("2. <b>Paulista US (BR)</b> — R$ 6,59 → ") && t.contains(" · 0 m") && t.contains(" · 2,3 km"), "{t}");
}

#[test]
fn atalho_de_postos() {
    use crate::atalhos::Atalho;
    let qui = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    for t in ["/postos", "/posto", "postos", "qual posto compensa?", "Qual o melhor posto?", "qual posto é mais barato", "onde abastecer?",
              "onde eu abasteço hoje", "melhor posto", "posto mais barato aqui"] {
        assert_eq!(atalho(t, qui), Some(Atalho::Postos), "{t}");
    }
    for t in ["abasteci no posto do centro", "o posto da esquina fechou?", "marca de passar no posto amanhã"] {
        assert_ne!(atalho(t, qui), Some(Atalho::Postos), "{t}");
    }
}

#[test]
fn todo_comando_do_menu_tem_atalho() {
    use crate::atalhos::COMANDOS;
    let qui = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    for (c, _) in COMANDOS {
        // atalhos e memoria são tratados direto no on_text
        if c != "atalhos" && c != "memoria" {
            assert!(atalho(&format!("/{c}"), qui).is_some(), "/{c}");
        }
        assert!(crate::atalhos::texto_atalhos(&nomes_teste(), "Ponta Grossa").contains(&format!("/{c}")) || c == "atalhos", "/{c} fora do /atalhos");
    }
}

/// servidor HTTP que responde corpos brutos (html, xlsx) na ordem; `respostas` recebe a URL base
/// (o html da ANP aponta pro próprio servidor). Devolve a primeira linha de cada pedido.
fn servidor_bruto(respostas: impl FnOnce(&str) -> Vec<Vec<u8>>) -> (String, std::sync::mpsc::Receiver<String>) {
    use std::io::{BufRead, BufReader, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let respostas = respostas(&url);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for corpo in respostas {
            let (mut s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut primeira = String::new();
            r.read_line(&mut primeira).unwrap();
            loop {
                let mut linha = String::new();
                r.read_line(&mut linha).unwrap();
                if linha == "\r\n" {
                    break;
                }
            }
            tx.send(primeira.trim().to_string()).unwrap();
            write!(s, "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", corpo.len()).unwrap();
            s.write_all(&corpo).unwrap();
        }
    });
    (url, rx)
}

#[test]
fn atualizar_baixa_mescla_e_geocodifica() {
    use crate::postos::{atualizar, carregar, Fontes};
    let tmp = TempDir::new("argos_anp.");
    let path = tmp.0.join("dados/postos.json");
    let html = |base: &str| format!(
        "<a href=\"{base}/arq/revendas_lpc_2026-09-13_2026-09-19.xlsx\">antiga</a> <a href=\"{base}/arq/revendas_lpc_2026-09-20_2026-09-26.xlsx\">nova</a> \
         <a href=\"{base}/arq/resumo_semanal_lpc_2026-09-20_2026-09-26.xlsx\">resumo</a>");
    // 1ª rodada: página, xlsx, Nominatim (APOLLO: rua não acha → CEP acha; PAULISTA: rua; A & B: nada)
    let nada = b"[]".to_vec();
    let ponto = |lat: &str, lon: &str| format!("[{{\"lat\": \"{lat}\", \"lon\": \"{lon}\"}}]").into_bytes();
    // 2ª rodada: página de novo e as 2 tentativas da A & B (rua, CEP)
    let (base, pedidos) = servidor_bruto(|base| vec![
        html(base).into_bytes(), xlsx_mini(),
        nada.clone(), ponto("-22.73", "-47.66"), ponto("-22.72", "-47.63"), nada.clone(), nada.clone(),
        html(base).into_bytes(), nada.clone(), nada.clone(),
    ]);
    let pagina = format!("{base}/lpc");
    let nom = format!("{base}/nom");
    let f = Fontes { anp: &pagina, nominatim: &nom, pausa: std::time::Duration::ZERO };
    let r = atualizar(&crate::rede::agente(), &f, "PIRACICABA", "SAO PAULO", &path).unwrap();
    assert_eq!(r, "semana 2026-09-20_2026-09-26: 3 postos em PIRACICABA · coordenadas: 2/3");
    let reqs: Vec<String> = (0..7).map(|_| pedidos.recv().unwrap()).collect();
    assert_eq!(reqs[0], "GET /lpc HTTP/1.1");
    assert_eq!(reqs[1], "GET /arq/revendas_lpc_2026-09-20_2026-09-26.xlsx HTTP/1.1");
    assert_eq!(reqs[2], "GET /nom/search?format=json&limit=1&countrycodes=br&street=RUA+SAO+JOSE+1079&city=PIRACICABA&state=SAO+PAULO HTTP/1.1");
    assert_eq!(reqs[3], "GET /nom/search?format=json&limit=1&countrycodes=br&postalcode=13416-730 HTTP/1.1");
    let p = carregar(&path).unwrap();
    assert_eq!(p.semana, "2026-09-20_2026-09-26");
    assert_eq!((p.postos[APOLLO].lat, p.postos[APOLLO].coord.as_str()), (Some(-22.73), "cep"));
    assert_eq!((p.postos[PAULISTA].lon, p.postos[PAULISTA].coord.as_str()), (Some(-47.63), "rua"));
    assert_eq!(p.postos[AB].lat, None);
    // 2ª rodada, mesma semana: não baixa o xlsx de novo, só tenta de novo quem ficou sem coordenada
    let r = atualizar(&crate::rede::agente(), &f, "PIRACICABA", "SAO PAULO", &path).unwrap();
    assert_eq!(r, "semana 2026-09-20_2026-09-26 já baixada · coordenadas: 0/1");
    let reqs: Vec<String> = (0..3).map(|_| pedidos.recv().unwrap()).collect();
    assert_eq!(reqs[0], "GET /lpc HTTP/1.1");
    assert!(reqs[1].starts_with("GET /nom/search?") && reqs[1].contains("street=RUA+DO+PORTO"), "{}", reqs[1]);
}

// ==========================================
// Remanejar tarefa (adiar_tarefa, botão 🕒 outra hora)
// ==========================================

#[test]
fn remanejar_pendente_so_muda_a_cobranca() {
    let mut st = estado();
    new_task(&mut st, "regar", "2026-10-02T09:00", Some(1), false, "09:00").unwrap();
    let t = &mut st.tarefas[0];
    assert_eq!(task_tick(t, &at("2026-10-02T09:00"), &cfg()).unwrap(), Some(Aviso::Vence));
    let proximo = t.proximo.clone(); // agenda fixa: já andou pro dia seguinte
    assert_eq!(&task_move(t, &at("2026-10-02T18:00"), &cfg()).unwrap()[..16], "2026-10-02T18:00");
    assert_eq!((t.proximo.clone(), t.hora.as_str(), t.pendente), (proximo, "09:00", true));
    assert_eq!(task_tick(t, &at("2026-10-02T17:59"), &cfg()).unwrap(), None);
    assert_eq!(task_tick(t, &at("2026-10-02T18:00"), &cfg()).unwrap(), Some(Aviso::Cobra));
}

#[test]
fn remanejar_futura_muda_o_vencimento() {
    let mut st = estado();
    new_task(&mut st, "boleto", "2026-10-05T10:00", None, false, "09:00").unwrap();
    let t = &mut st.tarefas[0];
    task_move(t, &at("2026-10-05T18:30"), &cfg()).unwrap();
    assert_eq!((&t.proximo[..16], t.hora.as_str(), t.pendente), ("2026-10-05T18:30", "18:30", false));
    assert_eq!(task_tick(t, &at("2026-10-05T10:00"), &cfg()).unwrap(), None);
    assert_eq!(task_tick(t, &at("2026-10-05T18:30"), &cfg()).unwrap(), Some(Aviso::Vence));
}

#[test]
fn remanejar_fora_da_janela_vai_pra_manha() {
    let mut st = estado();
    new_task(&mut st, "x", "2026-10-05T10:00", None, false, "09:00").unwrap();
    let t = &mut st.tarefas[0];
    task_tick(t, &at("2026-10-05T10:00"), &cfg()).unwrap();
    assert_eq!(&task_move(t, &at("2026-10-05T23:30"), &cfg()).unwrap()[..16], "2026-10-06T08:00");
}

#[test]
fn hora_livre() {
    let now = at("2026-10-02T10:16"); // sexta
    let h = |t: &str| parse_hora_livre(t, &now, "09:00").map(|d| iso(&d)[..16].to_string());
    for (t, quando) in [("18", "2026-10-02T18:00"), ("18h", "2026-10-02T18:00"), ("18h30", "2026-10-02T18:30"),
                        ("18:30", "2026-10-02T18:30"), ("às 18", "2026-10-02T18:00"), ("hoje 18h", "2026-10-02T18:00"),
                        ("9", "2026-10-03T09:00"), ("Amanhã 9", "2026-10-03T09:00"), ("amanhã às 9h", "2026-10-03T09:00"),
                        ("depois de amanhã 7", "2026-10-04T07:00"), ("sexta 14", "2026-10-02T14:00"),
                        ("sexta 9", "2026-10-09T09:00"), ("segunda-feira 8", "2026-10-05T08:00"),
                        // 13/10: "Hj as 16" e "Daqui 2 dias" iam pro Haiku, que perguntava qual tarefa
                        ("Hj as 16", "2026-10-02T16:00"), ("amnh 9", "2026-10-03T09:00"),
                        ("Daqui 2 dias", "2026-10-04T09:00"), ("daqui a 2 dias às 18", "2026-10-04T18:00"),
                        ("em três dias", "2026-10-05T09:00"), ("daqui 2 horas", "2026-10-02T12:16"),
                        ("daqui uma hora", "2026-10-02T11:16"), ("em meia hora", "2026-10-02T10:46"),
                        ("em 30 min", "2026-10-02T10:46"), ("daqui 2h", "2026-10-02T12:16"),
                        // só o dia: na hora da tarefa
                        ("amanhã", "2026-10-03T09:00"), ("depois de amanhã", "2026-10-04T09:00"),
                        ("segunda", "2026-10-05T09:00"), ("sexta", "2026-10-09T09:00"),
                        ("dia 15 às 9", "2026-10-15T09:00"), ("dia 15", "2026-10-15T09:00"),
                        ("dia 2", "2026-11-02T09:00"), ("dia 31 18h", "2026-10-31T18:00")] {
        assert_eq!(h(t).as_deref(), Some(quando), "{t}");
    }
    for t in ["hoje 9", "hoje", "25", "18:75", "bla", "me lembra às 18", "18 e pouco", "", "minhas tarefas",
              "daqui 0 dias", "em meia dia", "daqui 2 horas às 9", "dia 32", "quando eu sair da aula"] {
        assert_eq!(h(t), None, "{t}");
    }
}

#[test]
fn agenda_adia_tarefa_pendente_em_vez_de_criar() {
    let tmp = TempDir::new("argos_adiar.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let id = new_task(&mut bot.state, "regar as plantas", "2026-10-02T09:00", None, false, "09:00").unwrap().id;
    bot.state.tarefas[0].pendente = true;
    let volta1 = json!({"stop_reason": "tool_use", "content": [
        {"type": "tool_use", "id": "tu_1", "name": "adiar_tarefa", "input": {"id": id, "quando": "2026-10-02T18:00"}}]});
    let volta2 = json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "Pronto, às 18h."}]});
    let (url, rx) = claude_falso(vec![(200, volta1), (200, volta2)]);
    bot.claude.url = url;
    assert_eq!(bot.agenda_ask("me lembra às 18").unwrap(), "Pronto, às 18h.");
    assert_eq!(bot.state.tarefas.len(), 1);
    assert_eq!(&bot.state.tarefas[0].cobrar_em.as_deref().unwrap()[..16], "2026-10-02T18:00");
    assert_eq!(bot.agenda.actions, vec!["⏰ tarefa adiada: regar as plantas → sexta 02/10 18:00"]);
    assert_eq!(bot.agenda.undo_ops[0]["op"], "tarefa_antes");
    let p1 = rx.recv().unwrap();
    let texto = p1.corpo["messages"][0]["content"][0]["text"].as_str().unwrap().to_string();
    assert!(texto.starts_with("[agora: "));
    assert!(texto.ends_with(&format!("\n[tarefas pendentes (cobrando): #{id} regar as plantas]\nme lembra às 18")), "{texto}");
}

#[test]
fn intervalo_proprio_da_tarefa() {
    let mut st = estado();
    new_task(&mut st, "academia", "2026-10-05T09:00", None, false, "09:00").unwrap();
    let t = &mut st.tarefas[0];
    set_intervalo(t, Some(24.0)).unwrap();
    assert!(set_intervalo(t, Some(0.0)).is_err());
    assert_eq!(intervalo_h(t, 3.0), 24.0);
    task_tick(t, &at("2026-10-05T09:00"), &cfg()).unwrap();
    assert_eq!(&t.cobrar_em.as_deref().unwrap()[..16], "2026-10-06T09:00");
    // as outras seguem no padrão
    new_task(&mut st, "regar", "2026-10-05T09:00", None, false, "09:00").unwrap();
    let t = &mut st.tarefas[1];
    task_tick(t, &at("2026-10-05T09:00"), &cfg()).unwrap();
    assert_eq!(&t.cobrar_em.as_deref().unwrap()[..16], "2026-10-05T12:00");
}

#[test]
fn pausa_nao_cobra_e_pula_o_que_repete() {
    // fim de semana fora (13/10): regar todo dia e ligar pro médico cobravam
    let mut st = estado();
    new_task(&mut st, "regar as plantas", "2026-10-09T09:00", Some(1), false, "09:00").unwrap();
    new_task(&mut st, "ligar pro médico", "2026-10-09T10:00", None, false, "10:00").unwrap();
    for t in st.tarefas.iter_mut() {
        task_tick(t, &at("2026-10-09T10:00"), &cfg()).unwrap(); // as duas vencem
        assert!(t.pendente);
        set_pausa(t, Some(&at("2026-10-12T00:00")), &at("2026-10-09T10:30"));
    }
    for h in ["2026-10-09T13:00", "2026-10-10T09:00", "2026-10-11T15:00"] {
        for t in st.tarefas.iter_mut() {
            assert!(task_tick(t, &at(h), &cfg()).unwrap().is_none(), "{h} {}", t.texto);
        }
    }
    assert!(nota_pausa(&st.tarefas[0], &at("2026-10-11T15:00")).starts_with(" · ⏸ até segunda 12/10"));
    // acabou: regar pula o que passou e vence na próxima; o médico volta a cobrar no começo da janela
    let fim = at("2026-10-12T00:10");
    let regar = &mut st.tarefas[0];
    assert!(task_tick(regar, &fim, &cfg()).unwrap().is_none());
    assert!(!regar.pendente && regar.resto.get("pausada_ate").is_none());
    assert_eq!(&regar.proximo[..16], "2026-10-12T09:00");
    let medico = &mut st.tarefas[1];
    assert_eq!(&medico.cobrar_em.as_deref().unwrap()[..16], "2026-10-12T08:00");
    assert!(task_tick(medico, &fim, &cfg()).unwrap().is_none());
    assert!(matches!(task_tick(medico, &at("2026-10-12T08:00"), &cfg()).unwrap(), Some(Aviso::Cobra)));
}

#[test]
fn ferramenta_pausar_tarefas() {
    let tmp = TempDir::new("argos_pausa.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let a = new_task(&mut bot.state, "regar as plantas", "2099-10-09T09:00", Some(1), false, "09:00").unwrap().id;
    let b = new_task(&mut bot.state, "ligar pro médico", "2099-10-09T10:00", None, false, "10:00").unwrap().id;
    bot.run_tool("pausar_tarefas", &json!({"ids": [], "ate": "2099-10-12"})).unwrap();
    assert!(bot.state.tarefas.iter().all(|t| t.resto["pausada_ate"].as_str().unwrap().starts_with("2099-10-12T00:00")));
    assert_eq!(bot.agenda.actions.last().unwrap(), "⏸ pausadas até segunda 12/10 00:00: todas as tarefas");
    assert_eq!(bot.agenda.undo_ops.len(), 2);
    assert!(tasks_overview(&bot.state).unwrap().contains("regar as plantas — sexta 09/10 09:00 · a cada 1d · ⏸ até segunda 12/10 00:00"));
    // tira só a do médico
    bot.run_tool("pausar_tarefas", &json!({"ids": [b], "ate": null})).unwrap();
    assert_eq!(bot.agenda.actions.last().unwrap(), "▶️ pausa tirada: ligar pro médico");
    assert!(nota_pausa(&bot.state.tarefas[1], &crate::util::agora()).is_empty());
    assert!(!nota_pausa(&bot.state.tarefas[0], &crate::util::agora()).is_empty());
    assert!(bot.run_tool("pausar_tarefas", &json!({"ids": [a], "ate": "2020-01-01"})).is_err());
    assert!(bot.run_tool("pausar_tarefas", &json!({"ids": [999], "ate": "2099-10-12"})).is_err());
}

#[test]
fn tarefa_parecida_acha_a_mesma() {
    let mut st = estado();
    new_task(&mut st, "limpar a caixa de areia do guajuju", "2026-10-05T09:00", None, false, "09:00").unwrap();
    new_task(&mut st, "acertar com a academia", "2026-10-05T09:00", None, false, "09:00").unwrap();
    st.tarefas[1].pendente = true;
    let p = |t: &str| tarefa_parecida(&st.tarefas, t).map(|t| t.texto.clone());
    assert_eq!(p("Academia: aumente o intervalo").as_deref(), Some("acertar com a academia"));
    assert_eq!(p("trocar a areia do Guajuju").as_deref(), Some("limpar a caixa de areia do guajuju"));
    assert_eq!(p("limpar o quarto"), None);
    assert_eq!(p("comprar pão"), None);
}

#[test]
fn criar_tarefa_parecida_volta_pro_modelo_e_adiar_muda_intervalo() {
    let tmp = TempDir::new("argos_parecida.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let id = new_task(&mut bot.state, "acertar com a academia", "2026-10-09T09:00", None, false, "09:00").unwrap().id;
    bot.state.tarefas[0].pendente = true;
    let args = json!({"texto": "Academia: aumente o intervalo", "quando": "2099-10-15", "repetir_dias": null,
                      "contar_desde_feito": false, "janelas": [], "habito": false, "cobrar_a_cada_h": null});
    let e = bot.run_tool("criar_tarefa", &args).unwrap_err().to_string();
    assert!(e.contains(&format!("#{id} 'acertar com a academia' (pendente, cobrando)")) && e.contains("adiar_tarefa"), "{e}");
    assert_eq!(bot.state.tarefas.len(), 1);
    // insistiu com o mesmo texto: é outra tarefa mesmo
    bot.run_tool("criar_tarefa", &args).unwrap();
    assert_eq!(bot.state.tarefas.len(), 2);
    bot.state.tarefas.pop();
    // o certo: uma chamada de adiar com horário e intervalo; sem horário, só o intervalo
    let antes = bot.state.tarefas[0].cobrar_em.clone();
    let r = bot.run_tool("adiar_tarefa", &json!({"id": id, "cobrar_a_cada_h": 12})).unwrap();
    assert!(r.contains("\"cobra_a_cada_h\": 12"), "{r}");
    assert_eq!(bot.state.tarefas[0].cobrar_em, antes);
    assert_eq!(bot.agenda.actions.last().unwrap(), "⏰ tarefa ajustada: acertar com a academia · cobra a cada 12h");
    assert!(bot.run_tool("adiar_tarefa", &json!({"id": id, "quando": null, "janelas": [], "cobrar_a_cada_h": null})).is_err());
    // as pendentes que vão pro Haiku mostram o intervalo próprio
    let (url, rx) = claude_falso(vec![(200, json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "ok"}]}))]);
    bot.claude.url = url;
    bot.agenda_ask("e aí").unwrap();
    let p = rx.recv().unwrap();
    let texto = p.corpo["messages"][0]["content"][0]["text"].as_str().unwrap().to_string();
    assert!(texto.contains(&format!("[tarefas pendentes (cobrando): #{id} acertar com a academia · cobra a cada 12h]")), "{texto}");
    assert!(crate::util::dumps(&p.corpo["system"]).contains("Cobrança padrão das tarefas: a cada 3h."));
}

#[test]
fn botao_outra_hora_e_desfazer() {
    let tmp = TempDir::new("argos_outra_hora.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 20]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    let id = new_task(&mut bot.state, "regar as plantas", "2026-10-01T09:00", None, false, "09:00").unwrap().id;
    bot.state.tarefas[0].pendente = true;
    let enviadas = |rx: &std::sync::mpsc::Receiver<Pedido>| -> Vec<Value> {
        rx.try_iter().filter(|p| p.headers.starts_with("post /sendmessage")).map(|p| p.corpo).collect()
    };
    let toque = |bot: &mut crate::bot::Bot, data: &str| {
        bot.handle(&json!({"update_id": 1, "callback_query": {"id": "q", "from": {"id": 1}, "data": data}})).unwrap();
    };
    let fala = |bot: &mut crate::bot::Bot, text: &str| {
        bot.handle(&json!({"update_id": 2, "message": {"chat": {"id": 1}, "date": 0, "text": text}})).unwrap();
    };
    toque(&mut bot, &format!("tar:qq:{id}"));
    assert!(enviadas(&rx)[0]["text"].as_str().unwrap().starts_with("regar as plantas: pra quando?"));
    fala(&mut bot, "18h30");
    let m = enviadas(&rx).pop().unwrap();
    assert!(m["text"].as_str().unwrap().starts_with("regar as plantas: ⏰ te cobro "), "{}", m["text"]);
    assert!(m["text"].as_str().unwrap().ends_with(" 18:30"), "{}", m["text"]);
    assert!(bot.state.tarefas[0].cobrar_em.as_deref().unwrap().contains("T18:30"));
    let und = m["reply_markup"]["inline_keyboard"][0][0]["callback_data"].as_str().unwrap().to_string();
    toque(&mut bot, &und);
    assert_eq!(bot.state.tarefas[0].cobrar_em, None);
    // não era horário: a espera acaba e o texto segue o fluxo normal (aqui, o atalho de tarefas)
    enviadas(&rx);
    toque(&mut bot, &format!("tar:qq:{id}"));
    fala(&mut bot, "minhas tarefas");
    assert!(bot.esperando_hora.is_none());
    assert!(enviadas(&rx).last().unwrap()["text"].as_str().unwrap().contains("regar as plantas"));
    assert_eq!(bot.state.tarefas[0].cobrar_em, None);
    // só o dia: vai na hora da tarefa
    toque(&mut bot, &format!("tar:qq:{id}"));
    fala(&mut bot, "Daqui 2 dias");
    let m = enviadas(&rx).pop().unwrap();
    let esperado = (crate::util::agora() + chrono::Duration::days(2)).format("%d/%m 09:00").to_string();
    assert!(m["text"].as_str().unwrap().ends_with(&esperado), "{} / {esperado}", m["text"]);
}

#[test]
fn atalho_de_custos() {
    use crate::atalhos::Atalho;
    let qui = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    let c = |v: Option<&str>| Some(Atalho::Custos { veiculo: v.map(String::from) });
    let (hima, onix) = (Some("honda cg 160 titan"), Some("chevrolet onix"));
    for (t, v) in [("Qual a média km/l da moto", c(hima)), ("/custos", c(None)), ("/consumo", c(None)),
                   ("/custos carro", c(onix)), ("consumo do carro", c(onix)),
                   ("quanto gasto com a moto?", c(hima)), ("quantos km por litro tá fazendo?", c(None)),
                   ("qual o custo por km da titan", c(hima)), ("análise de custos", c(None))] {
        assert_eq!(atalho(t, qui), v, "{t}");
    }
    // período, gravação ou fora de veículo: modelo
    for t in ["média km/l da moto em setembro", "quanto gastei com a moto esse mês", "abasteci 10 litros na moto",
              "quanto gastei no mercado", "qual a média da prova", "registra o gasto da moto"] {
        assert_eq!(atalho(t, qui), None, "{t}");
    }
}

#[test]
fn custos_ponta_a_ponta_sem_claude() {
    let tmp = TempDir::new("argos_custos.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 10]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    bot.claude.url = "http://127.0.0.1:9/v1/messages".into(); // se chamar o Claude, falha
    let fala = |bot: &mut crate::bot::Bot, text: &str| {
        bot.handle(&json!({"update_id": 2, "message": {"chat": {"id": 1}, "date": 0, "text": text}})).unwrap();
    };
    fala(&mut bot, "Qual a média km/l da moto?");
    let corpos: Vec<Value> = rx.try_iter().filter(|p| p.headers.starts_with("post /sendmessage")).map(|p| p.corpo).collect();
    let t = corpos.last().unwrap()["text"].as_str().unwrap().to_string();
    assert!(t.starts_with(&format!("🧾 <b>{MOTO}</b>\n<pre>  14/04/26 → 10/09/26  (3531.2 km)\n")), "{t}");
    assert!(t.contains("(u)Km/L: 38.08 km/L (-1%)") && t.contains("Custo/10km: R$1.58/10km (+1%)</pre>"), "{t}");
    assert!(!t.contains(CARRO), "{t}");
    fala(&mut bot, "/custos");
    let corpos: Vec<Value> = rx.try_iter().filter(|p| p.headers.starts_with("post /sendmessage")).map(|p| p.corpo).collect();
    let t = corpos.last().unwrap()["text"].as_str().unwrap().to_string();
    assert!(t.contains(MOTO) && t.contains(&format!("🧾 <b>{CARRO}</b>\n<pre>  Sem registros suficientes.</pre>")), "{t}");
}

/// bot de instância nova: só `usuario` no configs.json, credenciais do Telegram/Anthropic, sem Google nem veículo
fn bot_minimo(tmp: &Path) -> crate::bot::Bot {
    let cred = tmp.join("credenciais");
    std::fs::create_dir_all(&cred).unwrap();
    for (n, c) in [("telegram_token", "0:falso"), ("chat_id", "1"), ("anthropic_key", "sk-falsa")] {
        std::fs::write(cred.join(n), c).unwrap();
    }
    std::fs::write(tmp.join("configs.json"), r#"{"usuario": {"nome": "Ana", "ela": true}}"#).unwrap();
    crate::bot::Bot::new(crate::bot::load_config(tmp).unwrap(), tmp).unwrap()
}

#[test]
fn instancia_minima_sem_google_nem_veiculo() {
    let tmp = TempDir::new("argos_min.");
    let mut bot = bot_minimo(&tmp.0);
    assert!(bot.gcal.is_none() && !bot.config.tem_veiculos);
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 10]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    bot.claude.url = "http://127.0.0.1:9/v1/messages".into(); // se chamar o Claude, falha
    let ultima = |rx: &std::sync::mpsc::Receiver<Pedido>| -> String {
        let corpos: Vec<Value> = rx.try_iter().filter(|p| p.headers.starts_with("post /sendmessage")).map(|p| p.corpo).collect();
        corpos.last().unwrap()["text"].as_str().unwrap().to_string()
    };
    let fala = |bot: &mut crate::bot::Bot, text: &str| {
        bot.handle(&json!({"update_id": 2, "message": {"chat": {"id": 1}, "date": 0, "text": text}})).unwrap();
    };
    // foto: nem baixa (o getFile iria pro servidor falso e quebraria a sequência)
    bot.handle(&json!({"update_id": 3, "message": {"chat": {"id": 1}, "date": 0, "photo": [{"file_id": "x"}]}})).unwrap();
    assert!(ultima(&rx).starts_with("Sem veículo cadastrado"));
    assert!(bot.photos.is_empty());
    fala(&mut bot, "o que tenho amanhã?");
    assert_eq!(ultima(&rx), "Google Agenda não configurada neste bot.");
    fala(&mut bot, "/custos");
    assert_eq!(ultima(&rx), "Sem veículo cadastrado neste bot.");
    fala(&mut bot, "/atalhos");
    let t = ultima(&rx);
    assert!(t.contains("/tarefas") && t.contains("/memoria") && !t.contains("/hoje") && !t.contains("/manutencao") && !t.contains("/custos")
            && !t.contains("/temporal") && !t.contains("/chuva") && !t.contains("/postos"), "{t}");
    fala(&mut bot, "/qualquercoisa");
    let t = ultima(&rx);
    assert!(t.contains("<b>Tarefas</b>") && t.contains("desligados (sem veículo cadastrado)") && !t.contains("Pra corrigir") && !t.contains("<b>Agenda:</b>"), "{t}");
    // tarefa vence e cobra sem consultar agenda nenhuma
    crate::tarefas::new_task(&mut bot.state, "regar as plantas", "2026-01-01T09:00", None, false, "09:00").unwrap();
    bot.tick().unwrap();
    assert!(ultima(&rx).contains("regar as plantas"));
    // cadastrou veículo com o bot rodando: o próximo tick já enxerga
    crate::tarefas::novo_veiculo(Path::new(&bot.config.motor_dir), "moto", "Honda CG 160", 2024).unwrap();
    bot.tick().unwrap();
    assert!(bot.config.tem_veiculos);
}

/// argos --init: monta a pasta que o bot sobe (mínima, em pareamento); token errado ou pasta pronta não grava nada
#[test]
fn init_monta_a_pasta() {
    let tmp = TempDir::new("argos_init.");
    let ruim = tmp.0.join("ruim");
    std::fs::create_dir_all(&ruim).unwrap();
    let r = crate::instalar::init_em(&ruim, &mut "Carla\nela\n\n\n\nnao-e-token\nsk-ant-x\n".as_bytes(), false);
    assert!(r.unwrap_err().to_string().contains("BotFather") && !ruim.join("configs.json").exists());
    crate::instalar::init_em(&tmp.0, &mut "Carla\nela\nestudante\n\na Io\n123456:ABCdefGHIjklMNOpqrSTUvwx\nsk-ant-x\n".as_bytes(), false).unwrap();
    let c = crate::bot::load_config(&tmp.0).unwrap();
    assert_eq!((c.usuario.nome.as_str(), c.usuario.ela, c.usuario.sobre.as_str(), c.usuario.cidade.as_str(), c.usuario.assistente.as_str()),
               ("Carla", true, "estudante", "", "a Io"));
    assert_eq!(std::fs::read_to_string(tmp.0.join("credenciais/anthropic_key")).unwrap(), "sk-ant-x");
    let b = crate::bot::Bot::new(c, &tmp.0).unwrap();
    assert!(b.pareamento.is_some() && b.gcal.is_none() && !b.config.tem_veiculos);
    assert!(crate::agenda::agenda_system(&b.config).starts_with("Você é a Io, assistente pessoal da Carla no Telegram (estudante). Gerencia uma lista"));
    assert!(crate::instalar::init_em(&tmp.0, &mut "x\n".as_bytes(), false).unwrap_err().to_string().contains("já existe"));
}

/// sem chat_id: só a mensagem privada com o código pareia (grava chat_id 0600); grupo, código errado e botão não
#[test]
fn pareamento_pelo_codigo() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("argos_par.");
    let mut bot = bot_minimo(&tmp.0);
    let cred = tmp.0.join("credenciais");
    std::fs::remove_file(cred.join("chat_id")).unwrap();
    let mut bot2 = crate::bot::Bot::new(bot.config.clone(), &tmp.0).unwrap();
    std::mem::swap(&mut bot, &mut bot2);
    let codigo = bot.pareamento.clone().unwrap();
    assert_eq!((codigo.len(), bot.chat_id), (6, 0));
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 5]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    let msg = |chat: i64, tipo: &str, text: &str| json!({"update_id": 1, "message": {"chat": {"id": chat, "type": tipo}, "date": 0, "text": text}});
    let errado = if codigo == "000000" { "000001" } else { "000000" };
    bot.handle(&msg(55, "private", errado)).unwrap();
    bot.handle(&msg(-100, "group", &codigo)).unwrap();
    bot.handle(&json!({"update_id": 1, "callback_query": {"id": "q", "from": {"id": 55}, "data": "und:1"}})).unwrap();
    bot.tick().unwrap();
    assert!(bot.pareamento.is_some() && !cred.join("chat_id").exists());
    assert!(rx.try_recv().is_err(), "não manda nada antes de parear");
    bot.handle(&msg(55, "private", &format!(" {codigo} "))).unwrap();
    assert_eq!((bot.chat_id, bot.pareamento.as_deref()), (55, None));
    assert_eq!(std::fs::read_to_string(cred.join("chat_id")).unwrap(), "55");
    assert_eq!(std::fs::metadata(cred.join("chat_id")).unwrap().permissions().mode() & 0o777, 0o600);
    let p = rx.recv().unwrap();
    assert!(p.headers.starts_with("post /sendmessage") && p.corpo["chat_id"] == json!(55) && p.corpo["text"].as_str().unwrap().starts_with("✅ Pareado"));
    // depois de parear, o código não vale mais nada (vira texto normal pro dono)
    assert_eq!(crate::bot::Bot::new(bot.config.clone(), &tmp.0).unwrap().chat_id, 55);
}

#[test]
fn outra_hora_sem_horario_vai_pro_haiku_com_a_tarefa() {
    let tmp = TempDir::new("argos_foco.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    // a mesma resposta serve pro Telegram e pro Claude (a ordem dos pedidos não importa)
    let tudo = json!({"ok": true, "result": {"message_id": 7}, "stop_reason": "end_turn", "content": [{"type": "text", "text": "Certo."}]});
    let (url, rx) = claude_falso(vec![(200, tudo); 20]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    bot.claude.url = url;
    for t in ["regar as plantas", "academia", "pote de ração"] {
        new_task(&mut bot.state, t, "2026-10-01T09:00", None, false, "09:00").unwrap();
    }
    for t in bot.state.tarefas.iter_mut() {
        t.pendente = true;
    }
    let id = bot.state.tarefas[1].id;
    bot.handle(&json!({"update_id": 1, "callback_query": {"id": "q", "from": {"id": 1}, "data": format!("tar:qq:{id}")}})).unwrap();
    let fala = |bot: &mut crate::bot::Bot, text: &str| {
        bot.handle(&json!({"update_id": 2, "message": {"chat": {"id": 1}, "date": 0, "text": text}})).unwrap();
    };
    let ao_claude = |rx: &std::sync::mpsc::Receiver<Pedido>| -> String {
        let p = rx.try_iter().find(|p| p.headers.starts_with("post /v1/messages")).expect("chamou o Claude");
        let msgs = p.corpo["messages"].as_array().unwrap();
        msgs.last().unwrap()["content"][0]["text"].as_str().unwrap().to_string()
    };
    fala(&mut bot, "quando eu sair da aula");
    let t = ao_claude(&rx);
    assert!(t.ends_with(&format!("[respondendo ao \"pra quando?\" do 🕒 da tarefa #{id} academia]\nquando eu sair da aula")), "{t}");
    // só naquela mensagem
    fala(&mut bot, "e o dentista?");
    let t = ao_claude(&rx);
    assert!(!t.contains("respondendo ao"), "{t}");
}

#[test]
fn atalho_situacao_da_moto() {
    use crate::atalhos::Atalho;
    let qui = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    for t in ["como ta a situação da moto", "Como tá a moto?", "qual o estado do carro", "como anda minha moto"] {
        assert_eq!(atalho(t, qui), Some(Atalho::Manutencao { filtro: vec![] }), "{t}");
    }
    for t in ["troquei o óleo da moto", "lavei a moto hoje", "a moto tá fazendo barulho"] {
        assert_eq!(atalho(t, qui), None, "{t}");
    }
}

// ==========================================
// alerta de temporal (temporal.rs)
// ==========================================

/// /avisos/ativos do INMET: (id, geocodes, severidade, inicio, fim) — com o lixo que o bot ignora
fn inmet_json(avisos: &[(i64, &str, &str, &str, &str)]) -> Value {
    let v: Vec<Value> = avisos
        .iter()
        .map(|(id, geo, sev, ini, fim)| json!({
            "id": id * 10, "id_aviso": id, "id_sequencia": 1, "codigo": format!("urn:oid:{id}"), "referencia": null,
            "poligono": "{\"type\":\"Polygon\",\"coordinates\":[[[-50.1,-24.2]]]}", "municipios": "Ponta Grossa - PR (4119905)",
            "geocodes": geo, "alterado": false, "encerrado": false, "inicio": ini, "fim": fim, "icone": "data:image/png;base64,AAAA",
            "descricao": "Tempestade", "aviso_cor": "#FF9900", "id_severidade": 1, "severidade": sev,
            "riscos": ["Chuva entre 30 e 60 mm/h, ventos intensos (60-100 km/h). Risco de queda de árvores."], "instrucoes": ["x"]}))
        .collect();
    json!({"hoje": v, "futuro": []})
}

/// Open-Meteo de 01 a 03/10: (dia, hora, mm, rajada, código), o resto 0
fn meteo_ext(h: &[(u32, u32, f64, f64, u32)]) -> Value {
    let (mut time, mut mm, mut raj, mut cod) = (vec![], vec![], vec![], vec![]);
    for d in [1, 2, 3] {
        for hr in 0..24 {
            time.push(json!(format!("2026-10-{d:02}T{hr:02}:00")));
            let c = h.iter().find(|c| c.0 == d && c.1 == hr);
            mm.push(json!(c.map(|c| c.2).unwrap_or(0.0)));
            raj.push(json!(c.map(|c| c.3).unwrap_or(10.0)));
            cod.push(json!(c.map(|c| c.4).unwrap_or(0)));
        }
    }
    json!({"hourly": {"time": time, "precipitation": mm, "wind_gusts_10m": raj, "weather_code": cod}})
}

const MUNICIPIO: &str = "4100103,4119905,4119954";

#[test]
fn temporal_puro() {
    use crate::clima::horas_extremas;
    use crate::temporal::*;
    let lim = Limites { rajada_kmh: 70.0, chuva_mm_h: 30.0, chuva_mm_dia: 60.0 };
    let qui = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    // código exato (41199050 não é Ponta Grossa), sem repetir id, encerrado fora, ordem por início e gravidade
    let mut j = inmet_json(&[
        (5, "41199050", "Grande Perigo", "2026-10-01 00:00", "2026-10-01 23:59"),
        (1, MUNICIPIO, "Perigo Potencial", "2026-10-01 00:00", "2026-10-01 23:59"),
        (2, MUNICIPIO, "Perigo", "2026-10-01 00:00", "2026-10-01 23:59"),
        (3, MUNICIPIO, "Grande Perigo", "2026-10-01 14:30", "2026-10-02 06:00"),
        (2, MUNICIPIO, "Perigo", "2026-10-01 00:00", "2026-10-01 23:59"),
        (4, MUNICIPIO, "Perigo", "2026-10-02 00:00", "2026-10-02 23:59"),
    ]);
    j["hoje"][5]["encerrado"] = json!(true);
    let todos: Vec<Aviso> = serde_json::from_value(j["hoje"].clone()).unwrap();
    let av = do_municipio(todos.into_iter(), "4119905");
    assert_eq!(av.iter().map(|a| a.id_aviso).collect::<Vec<_>>(), vec![2, 1, 3]);
    assert_eq!(criticos(&av).iter().map(|a| a.id_aviso).collect::<Vec<_>>(), vec![3]); // laranja não avisa mais
    assert_eq!(chave_inmet(&av[2]), "inmet:3:2");
    assert_eq!(faixa_inmet(&av[2], qui), "hoje 14h30 → amanhã 6h");
    assert_eq!(faixa_inmet(&av[0], qui), "hoje 0h → 23h59");

    // previsão local: rajada 14–16h (pico 95 às 15h), chuva forte às 20h, granizo, 65 mm no dia 2, nada do dia 3 (fora das 36 h)
    let h = horas_extremas(&meteo_ext(&[
        (1, 9, 0.0, 99.0, 0), // antes das 10h: fora
        (1, 14, 2.0, 72.0, 95), (1, 15, 5.0, 95.0, 96), (1, 16, 1.0, 70.0, 95), (1, 20, 31.5, 40.0, 0),
        (2, 3, 20.0, 0.0, 0), (2, 4, 25.0, 0.0, 0), (2, 5, 20.0, 0.0, 0), (3, 1, 0.0, 120.0, 0),
    ])).unwrap();
    let ext = extremos_locais(&h, at("2026-10-01T10:20").naive_local(), JANELA_H, &lim);
    let resumo: Vec<(Tipo, u32, u32, u32, u32, f64)> =
        ext.iter().map(|e| (e.tipo, chrono::Datelike::day(&e.dia), e.de_h, e.ate_h, e.hora_pico, e.pico)).collect();
    assert_eq!(resumo, vec![
        (Tipo::Rajada, 1, 14, 17, 15, 95.0),
        (Tipo::Granizo, 1, 15, 16, 15, 0.0),
        (Tipo::Chuva, 1, 20, 21, 20, 31.5),
        (Tipo::ChuvaDia, 2, 0, 22, 4, 65.0), // a janela termina 22h do dia 2
    ]);
    assert!(!ext[0].vermelho());
    assert_eq!(ext[0].chave(), "local:2026-10-01:Rajada");
    // texto_alerta só formata (cor nenhuma é filtrada aqui)
    assert_eq!(texto_alerta("Ponta Grossa", &[&av[0]], &ext, qui), "⚠️ <b>Temporal em Ponta Grossa</b>\n\n\
        🟠 <b>Tempestade (Perigo)</b> — hoje 0h → 23h59\nChuva entre 30 e 60 mm/h, ventos intensos (60-100 km/h).\n\n\
        📍 <b>Na propriedade</b> (previsão):\n• hoje 14h–17h: rajadas até 95 km/h (15h)\n• hoje 15h–16h: risco de granizo\n\
        • hoje 20h–21h: chuva forte, até 31.5 mm/h (20h)\n• amanhã: 65 mm de chuva no dia\n\n👉 Vale ligar pra ver se tá tudo bem.");
    // a rajada de 120 do dia 3 entra numa janela de 48 h a partir de amanhã e é vermelha
    let v = extremos_locais(&h, at("2026-10-02T06:00").naive_local(), 48, &lim);
    assert!(v.iter().any(|e| e.vermelho() && e.chave() == "local:2026-10-03:Rajada!"));

    // /temporal: nada crítico mostra o pico; fonte fora vira linha
    let calmo = horas_extremas(&meteo_ext(&[(1, 17, 3.0, 45.0, 0)])).unwrap();
    let t = texto_status("Ponta Grossa", &Ok(vec![]), &Ok(calmo), at("2026-10-01T10:00").naive_local(), &lim);
    assert_eq!(t, "✅ <b>Ponta Grossa</b>: nada crítico previsto\n\nINMET: nenhum aviso pra cá.\n\n📍 Na propriedade, 48 h: rajada máx 45 km/h (hoje 17h), chuva máx 3 mm/h.");
    let t = texto_status("Ponta Grossa", &Err(crate::util::Erro::Rede("timed out".into())), &Ok(vec![]), at("2026-10-01T10:00").naive_local(), &lim);
    assert!(t.starts_with("✅") && t.contains("INMET fora do ar (&lt;urlopen error timed out&gt;)"), "{t}");
    let t = texto_status("Ponta Grossa", &Ok(av.clone()), &Ok(vec![]), at("2026-10-01T10:00").naive_local(), &lim);
    assert!(t.starts_with("⚠️") && t.contains("🟡 <b>Tempestade (Perigo Potencial)</b>"), "{t}");

    let sil = ("23:00".to_string(), "06:00".to_string());
    assert!(em_silencio("23:00", &sil) && em_silencio("02:15", &sil) && em_silencio("05:59", &sil));
    assert!(!em_silencio("06:00", &sil) && !em_silencio("22:59", &sil));
    assert!(em_silencio("13:00", &("12:00".into(), "14:00".into())));
}

fn bot_temporal(tmp: &TempDir) -> crate::bot::Bot {
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    bot.config.clima_lat = None; // o aviso de chuva da moto não entra nesses testes
    bot.config.alerta_geocode = Some("4119905".into());
    bot.config.alerta_lat = Some(-25.0916);
    bot.config.alerta_lon = Some(-50.1668);
    bot
}

fn aponta(bot: &mut crate::bot::Bot, url: &str) {
    let base = url.trim_end_matches("/v1/messages").to_string();
    bot.inmet.base = base.clone();
    bot.clima.base = base.clone();
    bot.tg.base = base;
}

#[test]
fn temporal_avisa_uma_vez_e_de_novo_se_piorar() {
    let tmp = TempDir::new("argos_temporal.");
    let mut bot = bot_temporal(&tmp);
    let geo = bot.config.alerta_geocode.take();
    assert!(!bot.storm_tick(&at("2026-10-01T10:00")));
    bot.config.alerta_geocode = geo;
    let ok = json!({"ok": true, "result": {"message_id": 7}});
    let laranja = inmet_json(&[(1, MUNICIPIO, "Perigo Potencial", "2026-10-01 00:00", "2026-10-01 23:59"),
                               (2, MUNICIPIO, "Perigo", "2026-10-01 00:00", "2026-10-01 23:59"),
                               (9, "4100103", "Grande Perigo", "2026-10-01 00:00", "2026-10-01 23:59")]);
    let mut vermelho = laranja.clone();
    vermelho["hoje"].as_array_mut().unwrap().push(inmet_json(&[(3, MUNICIPIO, "Grande Perigo", "2026-10-01 15:00", "2026-10-01 20:00")])["hoje"][0].clone());
    let vento = meteo_ext(&[(1, 15, 4.0, 80.0, 0)]);
    let (url, rx) = claude_falso(vec![
        (200, laranja.clone()), (200, vento.clone()), (200, ok.clone()), // 10h00: avisa
        (200, laranja), (200, vento.clone()),                          // 10h30: nada novo
        (200, vermelho), (200, vento), (200, ok),                      // 11h00: vermelho novo
    ]);
    aponta(&mut bot, &url);

    assert!(bot.storm_tick(&at("2026-10-01T10:00")));
    let g = rx.recv().unwrap();
    assert!(g.headers.starts_with("get /avisos/ativos"), "{}", g.headers);
    assert!(g.headers.contains("user-agent: mozilla"), "{}", g.headers);
    let g = rx.recv().unwrap();
    assert!(g.headers.starts_with("get /forecast?latitude=-25.0916&longitude=-50.1668&hourly=precipitation,wind_gusts_10m,weather_code"), "{}", g.headers);
    let t = rx.recv().unwrap().corpo["text"].as_str().unwrap().to_string();
    assert!(t.contains("rajadas até 80 km/h (15h)"), "{t}");
    assert!(!t.contains("🟡") && !t.contains("🟠") && !t.contains("🔴"), "amarelo, laranja e outro município não avisam: {t}");

    // antes do intervalo: nem consulta
    assert!(!bot.storm_tick(&at("2026-10-01T10:20")));
    // mesma coisa de novo: só os dois GETs
    assert!(bot.storm_tick(&at("2026-10-01T10:30")));
    rx.recv().unwrap();
    rx.recv().unwrap();
    // vermelho novo: avisa só ele (laranja nunca avisa), com o bloco local de contexto
    assert!(bot.storm_tick(&at("2026-10-01T11:00")));
    rx.recv().unwrap();
    rx.recv().unwrap();
    let t = rx.recv().unwrap().corpo["text"].as_str().unwrap().to_string();
    assert!(t.contains("🔴 <b>Tempestade (Grande Perigo)</b> — hoje 15h → 20h") && !t.contains("🟠") && t.contains("80 km/h"), "{t}");
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(rx.try_recv().is_err());
    assert_eq!(bot.state.alerta_enviados.keys().cloned().collect::<Vec<_>>(), vec!["local:2026-10-01:Rajada", "inmet:3:2"]);
    // sobrevive ao save/load
    let st: crate::tarefas::Estado = serde_json::from_str(&serde_json::to_string(&bot.state).unwrap()).unwrap();
    assert_eq!(st.alerta_enviados.len(), 2);
}

#[test]
fn temporal_segura_a_madrugada_menos_vermelho() {
    let tmp = TempDir::new("argos_temporal_noite.");
    let mut bot = bot_temporal(&tmp);
    let ok = json!({"ok": true, "result": {"message_id": 7}});
    let laranja = inmet_json(&[(2, MUNICIPIO, "Perigo", "2026-10-01 00:00", "2026-10-01 23:59")]);
    let calmo = meteo_ext(&[]);
    let raj = meteo_ext(&[(1, 4, 0.0, 110.0, 0)]);
    let (url, rx) = claude_falso(vec![
        (200, laranja.clone()), (200, calmo.clone()),             // 02h00: laranja não avisa
        (200, laranja.clone()), (200, raj),                       // 02h30: rajada vermelha local → manda tudo
        (200, ok.clone()),
    ]);
    aponta(&mut bot, &url);
    assert!(bot.storm_tick(&at("2026-10-01T02:00")));
    rx.recv().unwrap();
    rx.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(rx.try_recv().is_err(), "laranja não avisa");
    assert!(bot.state.alerta_enviados.is_empty());
    assert!(bot.storm_tick(&at("2026-10-01T02:30")));
    rx.recv().unwrap();
    rx.recv().unwrap();
    let t = rx.recv().unwrap().corpo["text"].as_str().unwrap().to_string();
    assert!(!t.contains("🟠") && t.contains("rajadas até 110 km/h (4h)"), "{t}");

    // outro bot: rajada não vermelha segurada às 3h sai às 6h
    let tmp2 = TempDir::new("argos_temporal_6h.");
    let mut bot = bot_temporal(&tmp2);
    let vento = meteo_ext(&[(1, 15, 4.0, 80.0, 0)]);
    let (url, rx) = claude_falso(vec![(200, laranja.clone()), (200, vento.clone()), (200, laranja), (200, vento), (200, ok)]);
    aponta(&mut bot, &url);
    assert!(bot.storm_tick(&at("2026-10-01T03:00")));
    rx.recv().unwrap();
    rx.recv().unwrap();
    assert!(bot.storm_tick(&at("2026-10-01T06:00")));
    rx.recv().unwrap();
    rx.recv().unwrap();
    assert!(rx.recv().unwrap().corpo["text"].as_str().unwrap().contains("rajadas até 80 km/h (15h)"));
}

#[test]
fn temporal_avisa_uma_vez_quando_as_fontes_somem() {
    let tmp = TempDir::new("argos_temporal_falha.");
    let mut bot = bot_temporal(&tmp);
    let ok = json!({"ok": true, "result": {"message_id": 7}});
    let erro = json!({"erro": "x"});
    let (url, rx) = claude_falso(vec![
        (500, erro.clone()), (500, erro.clone()),               // 08h: só log
        (500, erro.clone()), (500, erro.clone()), (200, ok),    // 20h: 12 h → avisa
        (500, erro.clone()), (500, erro),                       // 20h30: já avisou
    ]);
    aponta(&mut bot, &url);
    assert!(bot.storm_tick(&at("2026-10-01T08:00")));
    rx.recv().unwrap();
    rx.recv().unwrap();
    assert_eq!(bot.state.alerta_falha_desde.as_deref(), Some("2026-10-01T08:00"));
    assert!(bot.storm_tick(&at("2026-10-01T20:00")));
    rx.recv().unwrap();
    rx.recv().unwrap();
    assert_eq!(rx.recv().unwrap().corpo["text"], "⚠️ Alerta de temporal em Ponta Grossa sem dados desde 01/10 08h (INMET e Open-Meteo falhando). Fica de olho por conta própria.");
    assert!(bot.storm_tick(&at("2026-10-01T20:30")));
    rx.recv().unwrap();
    rx.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(rx.try_recv().is_err());
}

#[test]
fn atalho_de_temporal() {
    use crate::atalhos::Atalho;
    let qui = chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
    for t in ["/temporal", "/pontagrossa", "alerta ponta grossa", "Como tá o tempo em Ponta Grossa?", "vai chover em ponta grossa amanhã", "tem alerta pra ponta grossa"] {
        assert_eq!(atalho(t, qui), Some(Atalho::Temporal), "{t}");
    }
    // a chuva da moto continua igual
    assert!(matches!(atalho("vai chover amanhã?", qui), Some(Atalho::Chuva { .. })));
}

// ==========================================
// fotos separadas e posto citado em texto (05/10)
// ==========================================

fn leitura_bomba() -> Leitura {
    Leitura { total_reais: Some(54.84), litros: Some(8.86), preco_litro: Some(6.19), combustivel: Some("gasolina_comum".into()), observacao: "só a bomba".into(), ..Default::default() }
}

fn leitura_painel(km: f64) -> Leitura {
    Leitura { odometro_km: Some(km), veiculo: Some("Honda CG 160 Titan".into()), observacao: "só o painel".into(), ..Default::default() }
}

#[test]
fn juntar_leituras_complementares_e_conflitantes() {
    let j = juntar(&leitura_bomba(), &leitura_painel(4928.6)).unwrap();
    assert_eq!((j.total_reais, j.litros, j.odometro_km), (Some(54.84), Some(8.86), Some(4928.6)));
    assert_eq!(j.combustivel.as_deref(), Some("gasolina_comum"));
    assert_eq!(j.observacao, "só a bomba · só o painel");
    // mesmo valor lido duas vezes (arredondamento) não é conflito
    let quase = Leitura { total_reais: Some(54.8), ..Default::default() };
    assert!(juntar(&leitura_bomba(), &quase).is_some());
    // odômetro diferente = outro abastecimento
    assert!(juntar(&leitura_painel(4928.6), &leitura_painel(5075.0)).is_none());
    assert!(juntar(&leitura_bomba(), &Leitura { combustivel: Some("etanol".into()), ..Default::default() }).is_none());
}

/// bomba numa mensagem e painel 30 s depois (o que aconteceu em 05/10): um rascunho só
#[test]
fn fotos_separadas_viram_um_rascunho() {
    let tmp = TempDir::new("argos_juntar.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 8]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    let ctx = || vehicle_context(&Motoregis::new(&motor), &Motoregis::new(&motor).load_config().unwrap()).unwrap();

    bot.apply_reading(leitura_bomba(), ctx()).unwrap();
    assert!(rx.recv().unwrap().headers.starts_with("post /sendmessage"));
    bot.apply_reading(leitura_painel(4928.6), ctx()).unwrap();
    let ed = rx.recv().unwrap();
    assert!(ed.headers.starts_with("post /editmessagetext"));
    assert!(ed.corpo["text"].as_str().unwrap().starts_with("↪ juntei"));
    assert!(rx.recv().unwrap().headers.starts_with("post /sendmessage"));
    let d = bot.draft.as_ref().unwrap();
    assert_eq!((d.total, d.litros, d.odometro), (Some(54.84), Some(8.86), Some(4928.6)));
    assert_eq!(d.veiculo, "Honda CG 160 Titan");

    // outro odômetro: rascunho novo, sem os valores da bomba
    bot.apply_reading(leitura_painel(4300.0), ctx()).unwrap();
    assert!(rx.recv().unwrap().headers.starts_with("post /sendmessage"));
    assert_eq!(bot.draft.as_ref().unwrap().total, None);
}

fn posto_teste(nome: &str, endereco: &str) -> crate::postos::Posto {
    crate::postos::Posto { nome: nome.into(), bandeira: "BRANCA".into(), endereco: endereco.into(), ..Default::default() }
}

#[test]
fn posto_por_nome_e_apelido() {
    use crate::postos::{por_nome, Postos};
    let mut p = Postos::default();
    p.postos.insert("11111111000111".into(), posto_teste("AUTO POSTO PAULISTA LTDA", "AV INDEPENDENCIA, 100"));
    p.postos.insert("22222222000122".into(), posto_teste("POSTO SAO JUDAS", "RUA XV DE NOVEMBRO, 5"));
    assert_eq!(por_nome(&p, "abasteci no posto Paulista"), vec!["11111111000111"]);
    assert_eq!(por_nome(&p, "é o posto da independência"), vec!["11111111000111"]);
    assert!(por_nome(&p, "É o posto \"perto do cena\"").is_empty());
    p.postos.get_mut("22222222000122").unwrap().apelidos.push("perto do Cena".into());
    assert_eq!(por_nome(&p, "É o posto \"perto do cena\""), vec!["22222222000122"]);
    assert!(por_nome(&p, "posto").is_empty());
}

/// "É o posto perto do cena" com rascunho aberto: não vai pro Haiku (nem pra memória); pede a localização,
/// e o posto escolhido aprende o apelido
#[test]
fn posto_citado_em_texto_com_rascunho_aberto() {
    let tmp = TempDir::new("argos_posto_texto.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let mut p = crate::postos::Postos::default();
    p.postos.insert("22222222000122".into(), posto_teste("POSTO SAO JUDAS", "RUA XV DE NOVEMBRO, 5"));
    crate::postos::gravar(&p, &bot.postos_path).unwrap();
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 8]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    bot.claude.url = "http://127.0.0.1:9/nao-chamar".into();
    let b = Base::de(&motor);
    let mut d = b.draft(b.reading());
    d.posto = None;
    bot.draft = Some(d);
    bot.context = Some(vehicle_context(&bot.mr, &bot.mr.load_config().unwrap()).unwrap());
    bot.draft_msg = Some(5);
    let msg = |bot: &mut crate::bot::Bot, t: &str| bot.handle(&json!({"update_id": 1, "message": {"chat": {"id": 1}, "date": 0, "text": t}})).unwrap();

    msg(&mut bot, "É o posto \"perto do cena\"");
    assert!(rx.recv().unwrap().corpo["text"].as_str().unwrap().starts_with("🤔 Não sei qual posto"));
    assert!(bot.state.memoria.is_empty());
    bot.handle(&json!({"update_id": 2, "callback_query": {"id": "q", "from": {"id": 1}, "data": "pst:22222222000122"}})).unwrap();
    let _ = rx.recv().unwrap(); // answerCallbackQuery
    assert_eq!(bot.draft.as_ref().unwrap().posto.as_deref(), Some("22222222000122"));
    let salvo = crate::postos::carregar(&bot.postos_path).unwrap();
    assert_eq!(salvo.postos["22222222000122"].apelidos, vec!["É o posto \"perto do cena\""]);
    // da próxima vez resolve pelo apelido
    assert_eq!(crate::postos::por_nome(&salvo, "posto perto do Cena"), vec!["22222222000122"]);
}

// ==========================================
// rascunho esquecido e posto fora da lista (05/10, 2ª rodada)
// ==========================================

#[test]
fn pergunta_sobre_o_rascunho() {
    use crate::bot::pergunta_do_rascunho;
    for t in ["O abastecimento foi registrado?", "salvou?", "foi gravado?", "registrou o abastecimento", "lançou?"] {
        assert!(pergunta_do_rascunho(t), "{t}");
    }
    for t in ["grava na agenda dentista amanhã às 10?", "o que tem amanhã?", "salva o contato do Carlos", "abasteci ontem"] {
        assert!(!pergunta_do_rascunho(t), "{t}");
    }
}

/// bot com rascunho aberto sem posto, Telegram falso e Claude numa porta morta (qualquer chamada falha)
fn bot_com_rascunho(tmp: &TempDir) -> (crate::bot::Bot, std::sync::mpsc::Receiver<Pedido>) {
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let mut p = crate::postos::Postos::default();
    p.postos.insert("22222222000122".into(), posto_teste("POSTO UNIVERSO 2000", "AV INDEPENDENCIA, 5"));
    crate::postos::gravar(&p, &bot.postos_path).unwrap();
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 12]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    bot.claude.url = "http://127.0.0.1:9/nao-chamar".into();
    let b = Base::de(&motor);
    let mut d = b.draft(b.reading());
    d.posto = None;
    bot.draft = Some(d);
    bot.leitura = Some((b.reading(), std::time::Instant::now()));
    bot.context = Some(vehicle_context(&bot.mr, &bot.mr.load_config().unwrap()).unwrap());
    bot.draft_msg = Some(5);
    (bot, rx)
}

fn texto(bot: &mut crate::bot::Bot, t: &str) {
    bot.handle(&json!({"update_id": 1, "message": {"chat": {"id": 1}, "date": 0, "text": t}})).unwrap();
}

#[test]
fn pergunta_se_salvou_reenvia_o_rascunho_sem_modelo() {
    let tmp = TempDir::new("argos_pergunta.");
    let (mut bot, rx) = bot_com_rascunho(&tmp);
    texto(&mut bot, "O abastecimento foi registrado?");
    assert_eq!(rx.recv().unwrap().corpo["text"], "Ainda não — falta tocar ✅ Salvar. Tá aqui:");
    let r = rx.recv().unwrap();
    assert!(r.headers.starts_with("post /sendmessage"));
    assert!(r.corpo["reply_markup"]["inline_keyboard"].to_string().contains("save"));
    assert!(bot.draft.is_some());
}

/// o caso de 05/10: localização mandada, nenhum dos 3 é, explica por texto → não pede localização de novo,
/// não grava apelido, reenvia o rascunho
#[test]
fn explicacao_do_posto_depois_da_localizacao() {
    let tmp = TempDir::new("argos_posto_longo.");
    let (mut bot, rx) = bot_com_rascunho(&tmp);
    bot.loc = Some(((-22.7, -47.63), std::time::Instant::now()));
    texto(&mut bot, "É shell mas desconfio que não seja esse posto universo 200, esse parece ser lá na vila independência que não é o caso. É novo n achei no Google maps");
    assert_eq!(rx.recv().unwrap().corpo["text"], "👍 posto fica em aberto — dá pra salvar assim:");
    assert!(rx.recv().unwrap().corpo["reply_markup"]["inline_keyboard"].to_string().contains("save"));
    assert!(bot.apelido_posto.is_none());
    assert!(bot.state.memoria.is_empty());
}

#[test]
fn explicacao_longa_sem_localizacao_nao_vira_apelido() {
    let tmp = TempDir::new("argos_posto_longo2.");
    let (mut bot, rx) = bot_com_rascunho(&tmp);
    texto(&mut bot, "abasteci num posto shell novo que abriu na saída pra Rio Claro");
    assert!(rx.recv().unwrap().corpo["text"].as_str().unwrap().starts_with("🤔"));
    assert!(bot.apelido_posto.is_none());
    texto(&mut bot, "posto perto do cena");
    let _ = rx.recv().unwrap();
    assert_eq!(bot.apelido_posto.as_deref(), Some("posto perto do cena"));
}

#[test]
fn nenhum_desses_reenvia_o_rascunho() {
    let tmp = TempDir::new("argos_nenhum.");
    let (mut bot, rx) = bot_com_rascunho(&tmp);
    bot.handle(&json!({"update_id": 2, "callback_query": {"id": "q", "from": {"id": 1}, "data": "pst:-"}})).unwrap();
    let _ = rx.recv().unwrap(); // answerCallbackQuery
    assert_eq!(rx.recv().unwrap().corpo["text"], "👍 fica sem posto — dá pra salvar assim:");
    assert!(rx.recv().unwrap().corpo["reply_markup"]["inline_keyboard"].to_string().contains("save"));
}

#[test]
fn lembrete_de_rascunho_sai_uma_vez() {
    let tmp = TempDir::new("argos_lembrete.");
    let (mut bot, rx) = bot_com_rascunho(&tmp);
    bot.draft_reminder().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(rx.try_recv().is_err(), "rascunho novo não lembra");
    let (l, _) = bot.leitura.take().unwrap();
    bot.leitura = Some((l, std::time::Instant::now() - std::time::Duration::from_secs(1900)));
    bot.draft_reminder().unwrap();
    assert!(rx.recv().unwrap().corpo["text"].as_str().unwrap().starts_with("⛽ O abastecimento de"));
    assert!(rx.recv().unwrap().corpo["reply_markup"]["inline_keyboard"].to_string().contains("save"));
    bot.draft_reminder().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(rx.try_recv().is_err(), "só uma vez");
}

#[test]
fn relatorio_sem_o_bloco_de_manutencao() {
    use crate::bot::sem_manutencao;
    let r = "--- Manutenções pendentes ---\n\n  ⚠️  Nível do óleo\n\n--- Resumo ---\n  (u)Km/L: 21.07";
    assert_eq!(sem_manutencao(r), "--- Resumo ---\n  (u)Km/L: 21.07");
    assert_eq!(sem_manutencao("--- Resumo ---\n  x"), "--- Resumo ---\n  x");
    assert_eq!(sem_manutencao("--- Manutenções pendentes ---\n  só isso"), "");
}

#[test]
fn conversa_poda_24h_e_le_botao_foto_local() {
    use crate::conversa::{da_mensagem, do_botao, do_envio, podar};
    let now = crate::util::from_iso("2026-10-07T12:00:00-03:00").unwrap();
    let todas = concat!(
        "{\"em\": \"2026-10-06T11:59:00-03:00\", \"de\": \"eu\", \"texto\": \"velha\"}\n",
        "lixo\n",
        "{\"em\": \"2026-10-06T12:01:00-03:00\", \"de\": \"argos\", \"texto\": \"nova\"}\n",
    );
    assert_eq!(podar(todas, &now), "{\"em\": \"2026-10-06T12:01:00-03:00\", \"de\": \"argos\", \"texto\": \"nova\"}\n");
    let q = json!({"message": {"reply_markup": {"inline_keyboard": [[{"text": "💾 Salvar", "callback_data": "save"}], [{"text": "✖", "callback_data": "x"}]]}}});
    assert_eq!(do_botao(&q, "save"), "[botão] 💾 Salvar");
    assert_eq!(do_botao(&json!({}), "mnt:0:oleo"), "[botão] mnt:0:oleo");
    assert_eq!(da_mensagem(&json!({"photo": [], "caption": "onix"})).unwrap(), "[foto] onix");
    assert_eq!(da_mensagem(&json!({"location": {"latitude": -22.7, "longitude": -47.6}})).unwrap(), "[localização] -22.7, -47.6");
    assert_eq!(do_envio("editMessageText", &json!({"text": "oi"})).unwrap(), "[editou] oi");
    assert!(do_envio("sendChatAction", &json!({"action": "typing"})).is_none());
}

/// mensagem minha + resposta do argos vão pro dados/conversa.jsonl; chat de outro não
#[test]
fn conversa_grava_os_dois_lados() {
    let tmp = TempDir::new("argos_conversa.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    std::fs::create_dir_all(tmp.0.join("dados")).unwrap();
    let (url, _rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 4]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    bot.handle(&json!({"update_id": 1, "message": {"chat": {"id": 99}, "date": 0, "text": "intruso"}})).unwrap();
    bot.handle(&json!({"update_id": 2, "message": {"chat": {"id": 1}, "date": 0, "text": "/atalhos"}})).unwrap();
    let linhas: Vec<Value> = std::fs::read_to_string(tmp.0.join("dados/conversa.jsonl")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(linhas.len(), 2, "{linhas:?}");
    assert_eq!((linhas[0]["de"].as_str(), linhas[0]["texto"].as_str()), (Some("eu"), Some("/atalhos")));
    assert_eq!(linhas[1]["de"], "argos");
    assert!(!linhas[1]["texto"].as_str().unwrap().is_empty());
}

#[test]
fn livres_respeita_janelas() {
    use crate::agenda::{horarios_livres, linha_livre, parse_janela};
    let ev = |t: &str, a: &str, b: &str| json!({"summary": t, "start": {"dateTime": format!("{a}:00-03:00")}, "end": {"dateTime": format!("{b}:00-03:00")}});
    let util = [parse_janela("08:00-11:59").unwrap(), parse_janela("14:00-17:49").unwrap()];
    let agora = at("2026-10-07T07:36").naive_local(); // quarta, o caso do print
    let algodao = ev("Algodão", "2026-10-07T08:00", "2026-10-07T12:00");
    let livres = horarios_livres(&[algodao.clone()], &util, agora, 1, 30);
    let linhas: Vec<String> = livres.iter().map(|(a, b)| linha_livre(a, b)).collect();
    assert_eq!(linhas, ["qua 07/10 · 14h–17h49"]); // nunca 12h

    // de no meio da janela começa em de; evento atravessando corta só o pedaço; buraco curto some
    let livres = horarios_livres(&[ev("Reunião", "2026-10-07T15:00", "2026-10-07T17:30")], &util, at("2026-10-07T14:40").naive_local(), 1, 30);
    let linhas: Vec<String> = livres.iter().map(|(a, b)| linha_livre(a, b)).collect();
    assert_eq!(linhas, Vec::<String>::new()); // 14h40–15h (20 min) e 17h30–17h49 (19 min)
    let livres = horarios_livres(&[ev("Reunião", "2026-10-07T15:00", "2026-10-07T17:30")], &util, at("2026-10-07T14:40").naive_local(), 1, 15);
    let linhas: Vec<String> = livres.iter().map(|(a, b)| linha_livre(a, b)).collect();
    assert_eq!(linhas, ["qua 07/10 · 14h40–15h", "qua 07/10 · 17h30–17h49"]);

    // dia inteiro e transparente não bloqueiam; vários dias; fuso Z vira São Paulo
    let eventos = [
        json!({"summary": "Feriado", "start": {"date": "2026-10-08"}, "end": {"date": "2026-10-09"}}),
        json!({"summary": "Lembrete", "transparency": "transparent", "start": {"dateTime": "2026-10-08T08:00:00-03:00"}, "end": {"dateTime": "2026-10-08T12:00:00-03:00"}}),
        json!({"summary": "Prova", "start": {"dateTime": "2026-10-08T17:00:00Z"}, "end": {"dateTime": "2026-10-08T21:00:00Z"}}), // 14h–18h local
        algodao,
        ev("Estágio", "2026-10-05T10:30", "2026-10-09T12:30"), // vários dias com horário: não ocupa
    ];
    let livres = horarios_livres(&eventos, &util, agora, 2, 30);
    let linhas: Vec<String> = livres.iter().map(|(a, b)| linha_livre(a, b)).collect();
    assert_eq!(linhas, ["qua 07/10 · 14h–17h49", "qui 08/10 · 08h–11h59"]);

    // horário livre (sem 'útil'): dia todo, Sono ocupa
    let dia = [parse_janela("00:00-23:59").unwrap()];
    let sono = [ev("Sono", "2026-10-07T00:00", "2026-10-07T07:00"), ev("Sono", "2026-10-07T23:00", "2026-10-08T07:00")];
    let livres = horarios_livres(&sono, &dia, at("2026-10-07T00:00").naive_local(), 1, 30);
    let linhas: Vec<String> = livres.iter().map(|(a, b)| linha_livre(a, b)).collect();
    assert_eq!(linhas, ["qua 07/10 · 07h–23h"]);

    assert!(parse_janela("12:00-08:00").is_err());
    assert!(parse_janela("8h-12h").is_err());
}

#[test]
fn livres_agenda_real_de_07_10() {
    use crate::agenda::{horarios_livres, linha_livre, parse_janela};
    let ev = |t: &str, a: &str, b: &str| json!({"summary": t, "start": {"dateTime": format!("{a}:00-03:00")}, "end": {"dateTime": format!("{b}:00-03:00")}});
    let util = [parse_janela("08:00-11:59").unwrap(), parse_janela("14:00-17:49").unwrap()];
    let eventos = [
        ev("Estágio", "2026-10-05T10:30", "2026-10-09T12:30"),
        ev("Máquinas e Implementos Agrícolas", "2026-10-07T09:00", "2026-10-07T12:00"),
        ev("Produção de Algodão, Café e Agroecologia", "2026-10-07T14:00", "2026-10-07T16:00"),
        ev("Sono", "2026-10-07T22:00", "2026-10-08T06:30"),
        ev("Introdução às Ciências Sociais", "2026-10-08T14:00", "2026-10-08T16:00"),
        ev("gp2", "2026-10-08T16:00", "2026-10-08T18:00"),
    ];
    let livres = horarios_livres(&eventos, &util, at("2026-10-07T09:46").naive_local(), 2, 30);
    let linhas: Vec<String> = livres.iter().map(|(a, b)| linha_livre(a, b)).collect();
    assert_eq!(linhas, ["qua 07/10 · 16h–17h49", "qui 08/10 · 08h–11h59"]);
}

#[test]
fn concluir_tarefa_que_nao_cobra_pede_botao() {
    // 07/10: "Moto o óleo tá conferido" e o Haiku concluiu "Regar as plantas", que nem estava cobrando
    let tmp = TempDir::new("argos_concluir.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    std::fs::create_dir_all(tmp.0.join("dados")).unwrap();
    let plantas = new_task(&mut bot.state, "Regar as plantas", "2099-10-08T09:00", Some(1), false, "09:00").unwrap();
    let dent = new_task(&mut bot.state, "dentista", "2026-10-05T14:30", None, false, "09:00").unwrap();
    bot.state.tarefas.iter_mut().find(|t| t.id == dent.id).unwrap().pendente = true;

    let r = bot.run_tool("concluir_tarefa", &json!({"id": plantas.id})).unwrap();
    assert!(r.contains("botão de confirmação"), "{r}");
    let t = bot.state.tarefas.iter().find(|t| t.id == plantas.id).unwrap();
    assert_eq!(t.proximo, plantas.proximo); // não andou
    assert!(bot.agenda.actions.is_empty() && bot.agenda.undo_ops.is_empty());
    assert_eq!(bot.agenda.pending_done.len(), 1);
    assert_eq!(bot.agenda.pending_done[0].0, plantas.id);

    // a que está cobrando conclui direto, como antes
    bot.run_tool("concluir_tarefa", &json!({"id": dent.id})).unwrap();
    assert!(bot.state.tarefas.iter().all(|t| t.id != dent.id)); // única: sai da lista
    assert_eq!(bot.agenda.actions.len(), 1);
}

#[test]
fn cobranca_so_no_horario_util() {
    // "joga o dentista pras 16h e, se eu não confirmar, cobra de novo só em horário útil"
    let mut st = estado();
    let t = new_task(&mut st, "dentista", "2026-10-05T14:30", None, false, "09:00").unwrap();
    let util = ["08:00-11:59".to_string(), "14:00-17:49".to_string()];
    let t = st.tarefas.iter_mut().find(|x| x.id == t.id).unwrap();
    t.pendente = true;
    set_janelas(t, &util).unwrap();
    assert!(set_janelas(t, &["8h-12h".to_string()]).is_err());
    set_janelas(t, &[]).unwrap(); // [] mantém
    assert_eq!(t.resto["janelas"], json!(util));

    // remanejar pra 12h30 cai às 14h; pra 16h fica 16h
    assert_eq!(task_move(t, &at("2026-10-07T12:30"), &cfg()).unwrap(), iso(&at("2026-10-07T14:00")));
    task_move(t, &at("2026-10-07T16:00"), &cfg()).unwrap();
    // cobra às 16h; +3h = 19h, fora do útil → amanhã 08h (sem as janelas seria 19h)
    assert_eq!(task_tick(t, &at("2026-10-07T16:00"), &cfg()).unwrap(), Some(Aviso::Cobra));
    assert_eq!(t.cobrar_em.as_deref(), Some(iso(&at("2026-10-08T08:00")).as_str()));
    // cobra às 10h; +3h = 13h (almoço) → 14h
    assert_eq!(task_tick(t, &at("2026-10-08T10:00"), &cfg()).unwrap(), Some(Aviso::Cobra));
    assert_eq!(t.cobrar_em.as_deref(), Some(iso(&at("2026-10-08T14:00")).as_str()));
    // passou da última janela do dia → 08h do dia seguinte
    assert_eq!(into_janelas(&at("2026-10-09T17:50"), &[(chrono::NaiveTime::from_hms_opt(8, 0, 0).unwrap(), chrono::NaiveTime::from_hms_opt(11, 59, 0).unwrap())]), at("2026-10-10T08:00"));
}

#[test]
fn aviso_de_manutencao_todo_dia_por_calendario() {
    // repetir_dias 1: o aviso de ontem 06:00:11 não segura o de hoje 06:00:05; no mesmo dia não repete
    let tmp = TempDir::new("argos_mnt_dia.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    std::fs::create_dir_all(tmp.0.join("dados")).unwrap();
    bot.config.manutencao_repetir_dias = 1.0;
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 4]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    let hoje = at("2026-10-07T06:00:05");
    let mconfig = bot.mr.load_config().unwrap();
    let vencidas = overdue_maintenance(&bot.mr, &mconfig, &hoje).unwrap();
    assert!(!vencidas.is_empty());
    for (v, items) in &vencidas {
        let mut keys: Vec<String> = items.iter().map(|(k, _)| k.clone()).collect();
        keys.sort();
        bot.state.avisos_manutencao.insert(v.clone(), json!({"itens": keys, "em": "2026-10-06T06:00:11-03:00"}));
    }
    bot.check_maintenance_alerts(&hoje, false).unwrap();
    let p = rx.recv_timeout(std::time::Duration::from_secs(2)).expect("tinha que avisar hoje");
    assert!(p.corpo["text"].as_str().unwrap().contains("manutenção vencida"));
    bot.check_maintenance_alerts(&at("2026-10-07T20:00"), false).unwrap();
    assert!(rx.recv_timeout(std::time::Duration::from_millis(300)).is_err(), "não repete no mesmo dia");
}

#[test]
fn botao_velho_ainda_faz_a_acao() {
    // 07/10 10:26: Haiku levou 2 min, o botão tocado nesse meio tempo deu "query is too old" e a ação se perdia
    let tmp = TempDir::new("argos_botao_velho.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    std::fs::create_dir_all(tmp.0.join("dados")).unwrap();
    let t = new_task(&mut bot.state, "dentista", "2026-10-05T14:30", None, false, "09:00").unwrap();
    let (url, rx) = claude_falso(vec![
        (400, json!({"ok": false, "description": "Bad Request: query is too old and response timeout expired or query ID is invalid"})),
        (200, json!({"ok": true, "result": {"message_id": 7}})),
    ]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    bot.handle(&json!({"update_id": 1, "callback_query": {"id": "q", "from": {"id": 1}, "data": format!("tar:ok:{}", t.id)}})).unwrap();
    assert!(bot.state.tarefas.iter().all(|x| x.id != t.id), "a tarefa tinha que ser concluída");
    let _ = rx.recv().unwrap();
    assert!(rx.recv().unwrap().corpo["text"].as_str().unwrap().contains("dentista"));
}

/// manutenção gravada com km estimada acima da real (média km/dia) não pode mandar a foto seguinte pro Onix
#[test]
fn manutencao_estimada_acima_nao_desvia_o_abastecimento() {
    let b = Base::de(&fixture("motor"));
    let csv = b.ctx.v(MOTO).csv.clone();
    let row: Vec<String> = ["30/09/26", &crate::util::fmt_g(b.last + 300.0), "2", "0.00", "0.00", "calibrar_pneus"].map(String::from).to_vec();
    b.mr.append_record(&csv, &row).unwrap();
    let ctx = vehicle_context(&b.mr, &b.mconfig).unwrap();
    assert_eq!(ctx.v(MOTO).last_odo, Some(b.last)); // o último abastecimento, não a linha da manutenção
    let d = new_draft(&Leitura { odometro_km: Some(b.last + 100.0), veiculo: None, ..b.reading() }, &ctx, "01/10/26", 3000.0);
    assert_eq!(d.veiculo, MOTO);
    assert!(problems(&d, &ctx, 0.03, 3000.0).is_empty(), "{:?}", problems(&d, &ctx, 0.03, 3000.0));
}

/// botão "Gravar ~X km?" de uma mensagem não reaparece na seguinte (dizer a km grava direto; tocar o velho duplicava)
#[test]
fn pendente_da_agenda_vale_so_pra_mensagem_dela() {
    let tmp = TempDir::new("argos_pend.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    bot.run_tool("registrar_manutencao", &json!({"veiculo": MOTO, "itens": ["calibrar_pneus"]})).unwrap();
    assert!(bot.agenda.pending_maint.is_some());
    bot.agenda.pending_delete = Some(json!({"id": "x"}));
    let fim = json!({"stop_reason": "end_turn", "content": [{"type": "text", "text": "Anotado."}]});
    let (url, _rx) = claude_falso(vec![(200, fim)]);
    bot.claude.url = url;
    assert_eq!(bot.agenda_ask("foi com 3800 km").unwrap(), "Anotado.");
    assert!(bot.agenda.pending_maint.is_none());
    assert!(bot.agenda.pending_delete.is_none());
}

/// aviso que falha no tick (MotorCostTracker sumido, Telegram fora) não derruba os outros nem o save do estado
#[test]
fn tick_segue_quando_um_aviso_falha() {
    let tmp = TempDir::new("argos_tick.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    bot.tg.base = "http://127.0.0.1:9".into(); // todo envio falha
    bot.mr = Motoregis::new(&tmp.0.join("nao_existe"));
    (bot.config.hora_manutencao, bot.config.hora_revisao_memoria) = ("00:00".into(), "00:00".into());
    (bot.config.clima_lat, bot.config.alerta_geocode, bot.config.postos_municipio) = (None, None, None);
    remember(&mut bot.state, "fato velho", &(crate::util::agora() - chrono::Duration::days(30)), None);
    bot.tick().unwrap();
    let salvo = load_state(&bot.state_path).unwrap();
    let hoje = crate::util::agora().format("%Y-%m-%d").to_string();
    assert_eq!(salvo.manutencao_checada.as_deref(), Some(hoje.as_str()));
    assert_eq!(salvo.memoria_checada.as_deref(), Some(hoje.as_str())); // passou do aviso de manutenção que falhou
}

/// hábito: concluir antes da cobrança grava direto no tracker.csv (sem botão de confirmação) e o ↩️ tira a linha
#[test]
fn habito_vai_pro_tracker_e_desfaz() {
    let tmp = TempDir::new("argos_habito.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let tracker = bot.state_path.with_file_name("tracker.csv");
    let amanha = (crate::util::agora().date_naive() + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
    let cria = |habito: bool, texto: &str| json!({"texto": texto, "quando": amanha, "repetir_dias": 2, "contar_desde_feito": true, "janelas": [], "habito": habito});
    bot.run_tool("criar_tarefa", &cria(true, "dar ração, gato")).unwrap();
    bot.run_tool("criar_tarefa", &cria(false, "regar as plantas")).unwrap();
    let (gato, planta) = (bot.state.tarefas[0].clone(), bot.state.tarefas[1].id);
    assert!(e_habito(&gato) && !e_habito(&bot.state.tarefas[1]));
    assert!(bot.agenda.actions[0].ends_with(" · 📊 hábito (vai pro tracker)"), "{}", bot.agenda.actions[0]);
    assert!(tasks_overview(&bot.state).unwrap().contains("• dar ração, gato 📊 — "));
    assert_eq!(describe_task(&gato).unwrap()["habito"], true);

    // não está cobrando: hábito conclui direto; a tarefa comum continua pedindo o botão
    bot.agenda.undo_ops.clear();
    let r = bot.run_tool("concluir_tarefa", &json!({"id": gato.id})).unwrap();
    assert!(r.contains("📊 no tracker"), "{r}");
    assert!(bot.agenda.pending_done.is_empty());
    assert!(bot.run_tool("concluir_tarefa", &json!({"id": planta})).unwrap().contains("botão de confirmação"));
    let csv = std::fs::read_to_string(&tracker).unwrap();
    let linhas: Vec<&str> = csv.split("\r\n").filter(|l| !l.is_empty()).collect();
    assert_eq!(linhas[0], "em,tarefa_id,tarefa,repetir_dias,previsto,estava_cobrando");
    assert!(linhas[1].ends_with(&format!(",{},\"dar ração, gato\",2,{},nao", gato.id, gato.proximo)), "{}", linhas[1]);
    assert_eq!(linhas.len(), 2);
    assert_ne!(bot.state.tarefas[0].proximo, gato.proximo); // desde feito: próxima conta de agora

    // ↩️: linha some, tarefa volta como estava
    let ops = std::mem::take(&mut bot.agenda.undo_ops);
    crate::desfazer::push_undo(&mut bot.state, "tarefa dar ração", ops, &crate::util::agora());
    assert!(bot.undo(None).unwrap().starts_with("↩️ desfeito"));
    assert_eq!(std::fs::read_to_string(&tracker).unwrap(), "em,tarefa_id,tarefa,repetir_dias,previsto,estava_cobrando\r\n");
    assert_eq!(bot.state.tarefas[0], gato);

    // botão ✅ da cobrança também grava
    let ok = json!({"ok": true, "result": {"message_id": 7}});
    let (url, rx) = claude_falso(vec![(200, ok.clone()), (200, ok)]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    bot.handle(&json!({"update_id": 1, "callback_query": {"id": "q", "from": {"id": 1}, "data": format!("tar:ok:{}", gato.id)}})).unwrap();
    rx.recv().unwrap();
    assert!(rx.recv().unwrap().corpo["text"].as_str().unwrap().contains("📊 no tracker"));
    assert_eq!(std::fs::read_to_string(&tracker).unwrap().matches("\r\n").count(), 2);
}

/// em evento da agenda não avisa nem cobra: segura até o fim (eventos colados juntos, mesmo critério do horarios_livres)
#[test]
fn compromisso_segura_aviso() {
    use crate::agenda::ocupado_ate;
    use crate::tarefas::adiar_por_compromisso;
    let ev = |t: &str, a: &str, b: &str| json!({"summary": t, "start": {"dateTime": format!("{a}:00-03:00")}, "end": {"dateTime": format!("{b}:00-03:00")}});
    let n = |s: &str| at(s).naive_local();
    let eventos = [
        ev("Sono", "2026-10-13T22:00", "2026-10-14T06:30"),
        ev("Máquinas", "2026-10-14T09:00", "2026-10-14T12:00"),
        ev("Almoço", "2026-10-14T12:00", "2026-10-14T13:00"), // colado: conta junto
        ev("Estágio", "2026-10-12T10:30", "2026-10-16T12:30"), // vários dias: não ocupa
        json!({"summary": "Lembrete", "transparency": "transparent", "start": {"dateTime": "2026-10-14T15:00:00-03:00"}, "end": {"dateTime": "2026-10-14T16:00:00-03:00"}}),
        json!({"summary": "Feriado", "start": {"date": "2026-10-14"}, "end": {"date": "2026-10-15"}}),
    ];
    assert_eq!(ocupado_ate(&eventos, n("2026-10-14T06:00")), Some(n("2026-10-14T06:30")));
    assert_eq!(ocupado_ate(&eventos, n("2026-10-14T10:00")), Some(n("2026-10-14T13:00")));
    assert_eq!(ocupado_ate(&eventos, n("2026-10-14T13:00")), None); // acabou
    assert_eq!(ocupado_ate(&eventos, n("2026-10-14T15:30")), None);
    assert_eq!(ocupado_ate(&eventos, n("2026-10-14T08:59")), None);

    // Cobra segurada: mesma contagem quando sair; Vence segurada: fica pendente e cobra no fim
    let mut st = estado();
    new_task(&mut st, "regar", "2026-10-14T06:00", Some(1), false, "09:00").unwrap();
    let t = &mut st.tarefas[0];
    crate::tarefas::set_janelas(t, &["06:00-07:59".into(), "18:00-23:29".into()]).unwrap();
    assert_eq!(task_tick(t, &at("2026-10-14T06:00"), &cfg()).unwrap(), Some(Aviso::Vence));
    adiar_por_compromisso(t, Aviso::Vence, &at("2026-10-14T06:30"), &cfg()).unwrap();
    assert_eq!((t.pendente, t.cobrancas, &t.cobrar_em.as_ref().unwrap()[..16]), (true, 0, "2026-10-14T06:30"));
    assert_eq!(task_tick(t, &at("2026-10-14T06:30"), &cfg()).unwrap(), Some(Aviso::Cobra));
    adiar_por_compromisso(t, Aviso::Cobra, &at("2026-10-14T13:00"), &cfg()).unwrap(); // fim fora das janelas → 18h
    assert_eq!((t.cobrancas, &t.cobrar_em.as_ref().unwrap()[..16]), (0, "2026-10-14T18:00"));
    assert_eq!(task_tick(t, &at("2026-10-14T18:00"), &cfg()).unwrap(), Some(Aviso::Cobra));
    assert_eq!(t.cobrancas, 1);
}

/// ponta a ponta: em evento não manda nada ao Telegram; Google fora avisa mesmo assim
#[test]
fn compromisso_segura_aviso_ponta_a_ponta() {
    let tmp = TempDir::new("argos_compromisso.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let aula = json!({"items": [{"id": "1", "summary": "Aula", "start": {"dateTime": "2026-10-14T09:00:00-03:00"}, "end": {"dateTime": "2026-10-14T11:00:00-03:00"}}]});
    let ok = json!({"ok": true, "result": {"message_id": 7}});
    let (url, rx) = claude_falso(vec![(200, json!({"access_token": "tok", "expires_in": 3600})), (200, aula), (200, json!({"items": []})), (200, ok)]);
    let base = url.trim_end_matches("/v1/messages").to_string();
    let cred = tmp.0.join("cred");
    std::fs::write(cred.join("google_client.json"), json!({"installed": {"client_id": "x", "client_secret": "y", "token_uri": format!("{base}/token")}}).to_string()).unwrap();
    bot.gcal = Some(crate::gcal::GoogleCalendar::new(crate::rede::agente(), &cred).unwrap());
    bot.gcal.as_mut().unwrap().base = base.clone();
    bot.tg.base = base;
    new_task(&mut bot.state, "dentista", "2026-10-14T10:00", None, false, "09:00").unwrap();
    new_task(&mut bot.state, "academia", "2026-10-14T10:00", None, false, "09:00").unwrap();

    // 10h, na aula: as duas seguram até 11h com uma consulta só à agenda
    let mut ocupado = None;
    for i in 0..2 {
        assert_eq!(bot.task_alert(i, &at("2026-10-14T10:00"), &cfg(), &mut ocupado).unwrap(), None);
    }
    assert!(rx.recv().unwrap().headers.starts_with("post /token"));
    assert!(rx.recv().unwrap().headers.contains("timemin=2026-10-14t10%3a00%3a00-03%3a00"));
    assert!(bot.state.tarefas.iter().all(|t| t.pendente && t.cobrar_em.as_deref().unwrap().starts_with("2026-10-14T11:00")));
    // 11h, livre: manda
    let k = bot.task_alert(0, &at("2026-10-14T11:00"), &cfg(), &mut None).unwrap().unwrap();
    bot.send_task_alerts(&[(0, k)]).unwrap();
    rx.recv().unwrap(); // agenda vazia
    let msg = rx.recv().unwrap();
    assert!(msg.headers.starts_with("post /sendmessage"), "{}", msg.headers);
    assert!(msg.corpo["text"].as_str().unwrap().contains("dentista"));

    // Google fora (servidor falso esgotado): avisa mesmo assim
    bot.gcal.as_mut().unwrap().base = "http://127.0.0.1:9".into();
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 8}}))]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    let k = bot.task_alert(1, &at("2026-10-14T11:00"), &cfg(), &mut None).unwrap().unwrap();
    bot.send_task_alerts(&[(1, k)]).unwrap();
    assert!(rx.recv().unwrap().corpo["text"].as_str().unwrap().contains("academia"));
}

/// 14/10 06:30: 4 cobranças em 4 mensagens. Na mesma volta do tick vão juntas, como a manutenção vencida
#[test]
fn cobrancas_juntas_numa_mensagem() {
    let tmp = TempDir::new("argos_juntas.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 10]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    for txt in ["regar as plantas", "acertar com a academia", "limpar a caixa de areia do guajuju e trocar"] {
        new_task(&mut bot.state, txt, "2026-10-14T06:30", None, false, "06:30").unwrap();
    }
    for t in &mut bot.state.tarefas[..2] {
        t.pendente = true;
    }
    bot.state.tarefas[1].cobrancas = 27;
    let ids: Vec<i64> = bot.state.tarefas.iter().map(|t| t.id).collect();
    bot.send_task_alerts(&[(0, Aviso::Cobra), (1, Aviso::Cobra), (2, Aviso::Vence)]).unwrap();
    let m = rx.recv().unwrap().corpo;
    assert_eq!(m["text"], "⏰ <b>3 tarefas cobrando</b>:\n• regar as plantas\n• acertar com a academia <i>(27ª vez)</i>\n\
                           • limpar a caixa de areia do guajuju e trocar 🔔\n\nToca no que você já fez:");
    let kb = m["reply_markup"]["inline_keyboard"].as_array().unwrap();
    assert_eq!(kb.len(), 4);
    assert_eq!(kb[0][0], json!({"text": "✅ regar as plantas", "callback_data": format!("tar:ok:{}", ids[0])}));
    assert_eq!(kb[0][1]["callback_data"], format!("tar:qq:{}", ids[0]));
    assert_eq!(kb[2][0]["text"], "✅ limpar a caixa de areia do…");
    let todas = format!("{},{},{}", ids[0], ids[1], ids[2]);
    assert_eq!(kb[3][1]["callback_data"], format!("tar:am:{todas}"));

    // ✅ numa, depois "⏰ 1h todas": a feita não volta a cobrar
    bot.state.tarefas[2].pendente = true;
    let toque = |bot: &mut crate::bot::Bot, data: &str| {
        bot.handle(&json!({"update_id": 1, "callback_query": {"id": "q", "from": {"id": 1}, "data": data}})).unwrap();
    };
    toque(&mut bot, &format!("tar:ok:{}", ids[0]));
    toque(&mut bot, &format!("tar:1h:{todas}"));
    let ultima = rx.try_iter().filter(|p| p.headers.starts_with("post /sendmessage")).last().unwrap().corpo;
    let txt = ultima["text"].as_str().unwrap();
    assert!(!txt.contains("regar") && txt.contains("acertar com a academia: ⏰ te cobro") && txt.contains("guajuju"), "{txt}");
    assert!(bot.state.tarefas.iter().filter(|t| t.id != ids[0]).all(|t| t.cobrar_em.is_some()));
    // um ↩️ desfaz as duas
    let und = ultima["reply_markup"]["inline_keyboard"][0][0]["callback_data"].as_str().unwrap().to_string();
    toque(&mut bot, &und);
    assert!(bot.state.tarefas.iter().filter(|t| t.id != ids[0]).all(|t| t.cobrar_em.is_none()));
}

/// login do Google em Rust (era só no argos.py): SHA-256, PKCE da RFC 7636, URL e retorno do navegador
#[test]
fn google_login_pkce_url_e_retorno() {
    use crate::gcal::{pkce_challenge, retorno_login, url_login};
    use crate::util::{sha256, unquote_plus};
    let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    assert_eq!(hex(sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(hex(sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(hex(sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
               "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    assert_eq!(hex(sha256(&[b'a'; 1000])), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
    assert_eq!(pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");

    let client = json!({"auth_uri": "https://accounts.google.com/o/oauth2/auth", "client_id": "123.apps"});
    let escopos = format!("{} https://www.googleapis.com/auth/drive.file", crate::gcal::GOOGLE_SCOPE);
    let url = url_login(&client, "http://127.0.0.1:5555", "st", "ch", &escopos).unwrap();
    assert_eq!(url, "https://accounts.google.com/o/oauth2/auth?client_id=123.apps&redirect_uri=http%3A%2F%2F127.0.0.1%3A5555\
        &response_type=code&scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar.events+https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.file\
        &access_type=offline&prompt=consent&state=st&code_challenge=ch&code_challenge_method=S256");

    let ps = retorno_login("GET /?state=st&code=4%2F0Ab_x+y&scope=a HTTP/1.1\r\n", "st").unwrap();
    assert!(ps.contains(&("code".into(), "4/0Ab_x y".into())));
    assert_eq!(retorno_login("GET /favicon.ico HTTP/1.1\r\n", "st"), None);
    assert_eq!(retorno_login("GET /?state=outro&code=x HTTP/1.1\r\n", "st"), None);
    assert!(retorno_login("GET /?state=st&error=access_denied HTTP/1.1\r\n", "st").is_some());
    assert_eq!(unquote_plus("%E2%9C%85+ok%"), "✅ ok%");
}

/// instância com tudo ligado (veículo, Google, chuva, temporal, postos, prompt_extra): prompt e ferramentas
/// travados em tests/fixtures/prompt/completo (sem o arquivo, grava; mudou de propósito → apaga e roda)
#[test]
fn prompt_completo_travado() {
    let d = fixture("prompt").join("completo");
    let mut c = config_completo();
    c.agenda_parceiro = None;
    assert!(c.ligado("chuva") && c.prompt_extra.is_some());
    let s = crate::agenda::agenda_system(&c);
    let t = serde_json::to_string_pretty(&agenda_tools(&c)).unwrap();
    if !d.join("system.txt").exists() {
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("system.txt"), &s).unwrap();
        std::fs::write(d.join("tools.json"), &t).unwrap();
    }
    assert_eq!(s, std::fs::read_to_string(d.join("system.txt")).unwrap());
    assert_eq!(t, std::fs::read_to_string(d.join("tools.json")).unwrap());
    // o prompt_extra entra entre MANUTENÇÃO e CHUVA, com os marcadores trocados
    assert!(s.contains("\n\nACADEMIA: ele treina seg/qua/sex às 18h; não marque nada nesse horário sem perguntar.\n\nCHUVA:"), "{s}");
}

/// instância nova só com `usuario` no configs.json: sem veículo, sem Google, sem chuva/temporal/postos
fn config_minima() -> crate::bot::Config {
    let tmp = TempDir::new("argos_min.");
    std::fs::write(tmp.0.join("configs.json"), r#"{"usuario": {"nome": "Ana", "ela": true}}"#).unwrap();
    let c = crate::bot::load_config(&tmp.0).unwrap();
    assert!(!c.tem_veiculos && !c.tem_google && c.prompt_extra.is_none() && !c.ligado("chuva") && !c.ligado("temporal") && !c.ligado("postos"));
    c
}

/// o prompt e as ferramentas da instância mínima ficam travados em tests/fixtures/prompt/minimo
/// (sem o arquivo, o teste grava e passa: mudou de propósito → apaga e roda de novo)
#[test]
fn prompt_minimo_travado() {
    let d = fixture("prompt").join("minimo");
    let c = config_minima();
    let s = crate::agenda::agenda_system(&c);
    let t = serde_json::to_string_pretty(&agenda_tools(&c)).unwrap();
    if !d.join("system.txt").exists() {
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("system.txt"), &s).unwrap();
        std::fs::write(d.join("tools.json"), &t).unwrap();
    }
    assert_eq!(s, std::fs::read_to_string(d.join("system.txt")).unwrap());
    assert_eq!(t, std::fs::read_to_string(d.join("tools.json")).unwrap());
    for proibido in ["{", "Google Agenda dela", "manutenção dos veículos", "MANUTENÇÃO:", "ABASTECIMENTO", "CHUVA:", "criar_evento só", "Lista de agenda", " ele ", "()"] {
        assert!(!s.contains(proibido), "{proibido} no prompt mínimo:\n{s}");
    }
    let tools = agenda_tools(&c);
    let nomes: Vec<&str> = tools.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    for fora in ["listar_eventos", "horarios_livres", "criar_evento", "editar_evento", "apagar_evento", "ver_manutencao", "registrar_manutencao", "previsao_chuva"] {
        assert!(!nomes.contains(&fora), "{fora} nas ferramentas");
    }
}

/// fuso do configs.json: relógio, evento no Google, URL do Open-Meteo e prompt seguem ele
#[test]
fn fuso_configuravel() {
    let tmp = TempDir::new("argos_fuso.");
    std::fs::write(tmp.0.join("configs.json"), r#"{"usuario": {"nome": "Ana"}, "fuso": "America/Manaus"}"#).unwrap();
    let c = crate::bot::load_config(&tmp.0).unwrap();
    assert_eq!(c.tz(), chrono_tz::Tz::America__Manaus);
    // o prompt diz o fuso da config (sem mexer no global do processo)
    assert!(crate::agenda::agenda_system(&c).contains("data e hora atuais de Manaus;"));
    assert!(agenda_tools(&c).to_string().contains("(horário de Manaus)") && !agenda_tools(&c).to_string().contains("São Paulo"));
    assert!(crate::agenda::agenda_system(&config_minima()).contains("data e hora atuais de São Paulo;"));
    crate::util::fuso_no_teste(Some(c.tz()));
    assert_eq!(crate::util::no_fuso(chrono::NaiveDate::from_ymd_opt(2026, 10, 3).unwrap().and_hms_opt(15, 0, 0).unwrap()).offset().local_minus_utc(), -4 * 3600);
    assert_eq!(crate::util::agora().offset().local_minus_utc(), -4 * 3600);
    assert_eq!(when_to_google("2026-10-03T15:00").unwrap(), json!({"dateTime": "2026-10-03T15:00:00", "timeZone": "America/Manaus"}));
    crate::util::fuso_no_teste(None);
    assert_eq!(crate::util::nome_do_fuso(chrono_tz::Tz::America__Cuiaba), "Cuiabá");
    assert_eq!(crate::util::nome_do_fuso(chrono_tz::Tz::America__Porto_Velho), "Porto Velho");
    // fora do binário (ou inventado): não sobe
    for f in ["America/New_York", "Marte/Olympus"] {
        std::fs::write(tmp.0.join("configs.json"), format!(r#"{{"usuario": {{"nome": "Ana"}}, "fuso": "{f}"}}"#)).unwrap();
        assert!(crate::bot::load_config(&tmp.0).unwrap_err().to_string().contains("não está no binário"), "{f}");
    }
}

/// configs.json sem `usuario` não sobe; chave que depende de outra também não
#[test]
fn config_sem_usuario_ou_incompleta_nao_sobe() {
    let tmp = TempDir::new("argos_cfg.");
    let erro = |j: &str| {
        std::fs::write(tmp.0.join("configs.json"), j).unwrap();
        crate::bot::load_config(&tmp.0).err().map(|e| e.to_string()).unwrap_or_default()
    };
    assert!(erro(r#"{"modelo": "x"}"#).contains("usuario"));
    assert!(erro(r#"{"usuario": {"nome": "Ana"}, "alerta_geocode": "1"}"#).contains("alerta_nome"));
    assert!(erro(r#"{"usuario": {"nome": "Ana"}, "postos_municipio": "X"}"#).contains("postos_estado"));
    assert_eq!(erro(r#"{"usuario": {"nome": "Ana"}}"#), "");
    // padrões: credenciais e veículos dentro da pasta do bot, sem git
    let c = crate::bot::load_config(&tmp.0).unwrap();
    assert_eq!(c.cred_dir, tmp.0.join("credenciais").to_string_lossy());
    assert_eq!(c.motor_dir, tmp.0.join("dados").join("motor").to_string_lossy());
    assert!(!c.git_sync && c.cobrar_a_cada_h == 3.0 && c.modelo_agenda == "claude-haiku-5-5");
    // o exemplo do repositório sobe, com tudo ligado
    std::fs::copy(Path::new(env!("CARGO_MANIFEST_DIR")).join("exemplos/configs.exemplo.json"), tmp.0.join("configs.json")).unwrap();
    let c = crate::bot::load_config(&tmp.0).unwrap();
    assert_eq!(c.motor_dir, tmp.0.join("dados/motor").to_string_lossy());
    assert!(c.ligado("chuva") && c.ligado("temporal") && c.ligado("postos"));
}

/// /atalhos, menu e "pra corrigir" com os veículos do fixture; com um veículo só, sem o exemplo de troca
#[test]
fn textos_com_os_veiculos_da_instancia() {
    let antes = concat!(
        "⚡ <b>Atalhos</b> (respondo na hora, sem IA):\n", "\n",
        "📅 /hoje · /amanha · /semana · /agenda — ou <i>o que tenho sexta?</i>\n",
        "✅ /tarefas — ou <i>minhas tarefas</i>\n",
        "🔧 /manutencao — ou <i>como tá a moto?</i>, <i>quando calibrei os pneus?</i>\n",
        "🌧️ /chuva — ou <i>vai chover amanhã?</i>, <i>previsão sexta</i>\n",
        "⛈️ /temporal — ou <i>alerta ponta grossa</i>, <i>como tá o tempo em ponta grossa?</i> (aviso sozinho se vier temporal)\n",
        "⛽ /postos — ou <i>qual posto compensa?</i>, <i>onde abastecer?</i> (manda a localização antes pra ver os de perto)\n",
        "🧾 /custos — ou <i>média km/l da moto</i>, <i>quanto gasto com o carro?</i>\n",
        "↩️ /desfazer — desfaz a última gravação\n",
        "🧠 /memoria — revisar o que eu guardei\n", "\n",
        "O resto (marcar, apagar, perguntas abertas) vai pra IA. Tudo: /ajuda",
    );
    assert_eq!(crate::atalhos::texto_atalhos(&nomes_teste(), "Ponta Grossa"), antes);
    let um = crate::atalhos::Nomes::de(&[("Fiat Uno Mille".into(), false)], Some("São José dos Pinhais"));
    let t = crate::atalhos::texto_atalhos(&um, "São José dos Pinhais");
    assert!(t.contains("<i>como tá o carro?</i>") && t.contains("<i>média km/l do carro</i>\n") && t.contains("tempo em são josé dos pinhais?"), "{t}");
    assert_eq!(crate::atalhos::atalho("/saojosedospinhais", chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), &um), Some(crate::atalhos::Atalho::Temporal));
    assert_eq!(crate::atalhos::atalho("alerta São José dos Pinhais", chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), &um), Some(crate::atalhos::Atalho::Temporal));
    assert_eq!(atalho("alerta são josé dos pinhais", chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()), None);
    assert_eq!(crate::atalhos::atalho("consumo da mille", chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(), &um),
               Some(crate::atalhos::Atalho::Custos { veiculo: Some("fiat uno mille".into()) }));

    let dois = ["Honda CG 160 Titan".to_string(), "Chevrolet Onix".to_string()];
    assert_eq!(crate::rascunho::texto_corrigir(&dois, "Pra corrigir"),
               "Pra corrigir, escreve tipo <code>km 3810</code>, <code>litros 8,5</code>, <code>total 52,30</code>, <code>etanol</code> ou <code>onix</code>.");
    assert_eq!(crate::rascunho::texto_corrigir(&dois[..1], "Pra corrigir"),
               "Pra corrigir, escreve tipo <code>km 3810</code>, <code>litros 8,5</code>, <code>total 52,30</code> ou <code>etanol</code>.");

    // moto pelo item de corrente (JSON do MotorCostTracker, sem "tipo")
    let mr = Motoregis::new(fixture("motor"));
    assert!(crate::rascunho::e_moto(&mr.load_json("veiculos/titan_22.json").unwrap().unwrap()));
    assert!(!crate::rascunho::e_moto(&mr.load_json("veiculos/onix_19.json").unwrap().unwrap()));
    assert!(crate::rascunho::e_moto(&json!({"tipo": "moto"})) && !crate::rascunho::e_moto(&json!({"tipo": "carro", "manutencoes": {"x": {"corrente": 1}}})));
}

/// argos --veiculo: pasta nova no formato do MotorCostTracker, que o resto do bot usa igual
#[test]
fn cadastro_de_veiculo_do_zero() {
    let tmp = TempDir::new("argos_veic.");
    let motor = tmp.0.join("motor");
    let j = crate::tarefas::novo_veiculo(&motor, "carro", "Fiat Uno Mille", 2012).unwrap();
    assert!(j.ends_with("veiculos/mille_12.json"));
    crate::tarefas::novo_veiculo(&motor, "moto", "Honda CG 160", 2024).unwrap();
    assert!(crate::tarefas::novo_veiculo(&motor, "moto", "Honda CG 160", 2024).is_err());
    assert!(crate::tarefas::novo_veiculo(&motor, "barco", "X", 2024).is_err());
    let mr = Motoregis::new(&motor);
    let mconfig = mr.load_config().unwrap();
    assert_eq!(mconfig.veiculos_index.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["Fiat Uno Mille", "Honda CG 160"]);
    let ctx = vehicle_context(&mr, &mconfig).unwrap();
    assert!(!ctx.v("Fiat Uno Mille").moto && ctx.v("Honda CG 160").moto);
    assert_eq!(std::fs::read_to_string(motor.join("veiculos/cg_24.csv")).unwrap(), "data,odometro,categoria,custo,litros,detalhes\r\n");
    // primeiro abastecimento grava e o relatório sai
    let leitura = Leitura { total_reais: Some(50.0), litros: Some(8.0), preco_litro: Some(6.25), combustivel: Some("gasolina_comum".into()),
                            odometro_km: Some(1200.0), veiculo: Some("Honda CG 160".into()), observacao: String::new(), cnpj_posto: None };
    let d = crate::rascunho::new_draft(&leitura, &ctx, "01/10/26", 3000.0);
    assert_eq!(d.veiculo, "Honda CG 160");
    let rel = crate::tarefas::save(&mr, &d, &ctx, &mconfig).unwrap();
    assert!(!rel.is_empty());
    assert!(std::fs::read_to_string(motor.join("veiculos/cg_24.csv")).unwrap().contains("01/10/26,1200,1,"));
    crate::tarefas::overdue_maintenance(&mr, &mconfig, &at("2026-10-02T08:00")).unwrap();
    // o Config enxerga
    std::fs::write(tmp.0.join("configs.json"), format!(r#"{{"usuario": {{"nome": "Ana"}}, "motor_dir": {:?}}}"#, motor.to_string_lossy())).unwrap();
    assert!(crate::bot::load_config(&tmp.0).unwrap().tem_veiculos);
}

/// instância dela: ela/dela, sem Pedro como dono, sem chuva, e o `de` nas consultas de agenda
#[test]
fn prompt_de_outra_pessoa() {
    let c = config_dela();
    let s = crate::agenda::agenda_system(&c);
    assert!(s.starts_with("Você é o Argos, assistente pessoal da Ana no Telegram (estudante de biologia). Gerencia a Google Agenda dela,"), "{}", crate::util::corta(&s, 200));
    // " ele " só na linha da agenda do parceiro (é o Pedro)
    let sem_parceiro: String = s.lines().filter(|l| !l.starts_with("- Agenda do Pedro")).collect();
    assert!(!sem_parceiro.contains(" ele ") && !s.contains("{") && !s.contains("CHUVA:"), "{s}");
    assert!(s.contains("chefe da Ana") && s.contains("Ela pediu pra esquecer"));
    assert!(s.contains("- Agenda do Pedro: listar_eventos e horarios_livres com de='Pedro' consultam a agenda dele (só leitura)."), "{s}");
    assert!(s.contains("= a agenda dela.\n\nTAREFA ou EVENTO"), "{s}");
    let tools = agenda_tools(&c);
    let txt = tools.to_string();
    assert!(!txt.contains(" ele ") && !txt.contains("{ele}") && !txt.contains("previsao_chuva"));
    assert!(txt.contains("usa o fim dele como validade")); // "dele" do evento fica
    let listar = tools.as_array().unwrap().iter().find(|t| t["name"] == "listar_eventos").unwrap();
    assert_eq!(listar["input_schema"]["properties"]["de"]["enum"], json!(["eu", "Pedro"]));
    // nome do bot: sem a chave é "o Argos" (acima); com ela, o que vier
    let mut c = config_dela();
    c.usuario.assistente = "a Io".into();
    assert!(crate::agenda::agenda_system(&c).starts_with("Você é a Io, assistente pessoal da Ana"));
}

/// listar_eventos com de='Pedro' lê a agenda compartilhada (calendars/<email>), sem `de` lê a própria;
/// agenda não compartilhada (403) vira uma explicação pro modelo
#[test]
fn agenda_do_parceiro_so_leitura() {
    let tmp = TempDir::new("argos_parceiro.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    bot.config.agenda_parceiro = config_dela().agenda_parceiro;
    let aula = json!({"items": [{"id": "1", "summary": "Aula", "start": {"dateTime": "2026-10-15T09:00:00-03:00"}, "end": {"dateTime": "2026-10-15T11:00:00-03:00"}}]});
    let (url, rx) = claude_falso(vec![(200, json!({"access_token": "tok", "expires_in": 3600})), (200, aula.clone()), (200, json!({"items": []})),
                                     (403, json!({"error": {"message": "Not Found"}})), (200, aula.clone()),
                                     (200, json!({"items": [{"id": "2", "summary": "Estágio", "start": {"dateTime": "2026-10-15T14:00:00-03:00"}, "end": {"dateTime": "2026-10-15T16:00:00-03:00"}}]}))]);
    let base = url.trim_end_matches("/v1/messages").to_string();
    let cred = tmp.0.join("cred");
    std::fs::write(cred.join("google_client.json"), json!({"installed": {"client_id": "x", "client_secret": "y", "token_uri": format!("{base}/token")}}).to_string()).unwrap();
    bot.gcal = Some(crate::gcal::GoogleCalendar::new(crate::rede::agente(), &cred).unwrap());
    bot.gcal.as_mut().unwrap().base = base;
    let args = |de: &str| json!({"inicio": "2026-10-15", "fim": "2026-10-15", "busca": null, "de": de});

    let r = bot.run_tool("listar_eventos", &args("pedro")).unwrap();
    assert!(r.contains("Aula"), "{r}");
    rx.recv().unwrap(); // token
    assert!(rx.recv().unwrap().headers.starts_with("get /calendars/m%40exemplo.com/events?"));
    bot.run_tool("listar_eventos", &args("eu")).unwrap();
    assert!(rx.recv().unwrap().headers.starts_with("get /calendars/primary/events?"));
    let e = bot.run_tool("listar_eventos", &args("Pedro")).unwrap_err().to_string();
    assert!(e.contains("a agenda de Pedro não está compartilhada"), "{e}");
    // 'juntos': as duas agendas somadas, uma linha por dia
    rx.recv().unwrap(); // o 403
    let r = bot.run_tool("horarios_livres", &json!({"janelas": ["08:00-18:00"], "inicio": "2026-10-15T08:00", "dias": 1, "duracao_min": 30, "de": "juntos"})).unwrap();
    assert!(r.contains("\"linhas_prontas\": [\"qui 15/10 · 08h–09h, 11h–14h, 16h–18h\"]"), "{r}");
    assert!(rx.recv().unwrap().headers.starts_with("get /calendars/primary/events?"));
    assert!(rx.recv().unwrap().headers.starts_with("get /calendars/m%40exemplo.com/events?"));
}

/// aviso das 06:00 só dos veículos de `manutencao_veiculos` (na fixture só a Titan tem registros vencidos)
#[test]
fn aviso_de_manutencao_so_dos_veiculos_escolhidos() {
    let tmp = TempDir::new("argos_mnt_filtro.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    std::fs::create_dir_all(tmp.0.join("dados")).unwrap();
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 4]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    bot.config.manutencao_veiculos = Some(vec!["Onix".into()]);
    bot.check_maintenance_alerts(&at("2026-10-07T06:00"), false).unwrap();
    assert!(rx.recv_timeout(std::time::Duration::from_millis(300)).is_err(), "a moto não é dela");
    bot.config.manutencao_veiculos = Some(vec!["titan".into()]);
    bot.check_maintenance_alerts(&at("2026-10-07T06:00"), false).unwrap();
    assert!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap().corpo["text"].as_str().unwrap().contains("Titan"));
}

/// chuva desligada: atalho responde que está desligado, /atalhos sem a linha;
/// chuva_aviso false segura só o aviso automático
#[test]
fn recursos_desligados() {
    let tmp = TempDir::new("argos_desligados.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let mut bot = bot_de_teste(&tmp.0, &motor);
    let (url, rx) = claude_falso(vec![(200, json!({"ok": true, "result": {"message_id": 7}})); 6]);
    bot.tg.base = url.trim_end_matches("/v1/messages").to_string();
    let c = config_dela();
    bot.config.clima_lat = c.clima_lat;
    let quarta = at("2026-10-14T20:00");
    assert!(!bot.rain_tick(&quarta));
    let fala = |bot: &mut crate::bot::Bot, text: &str| {
        bot.handle(&json!({"update_id": 2, "message": {"chat": {"id": 1}, "date": 0, "text": text}})).unwrap();
    };
    fala(&mut bot, "/chuva");
    assert_eq!(rx.recv().unwrap().corpo["text"], "/chuva: desligado neste bot.");
    fala(&mut bot, "/atalhos");
    let t = rx.recv().unwrap().corpo["text"].as_str().unwrap().to_string();
    assert!(!t.contains("/chuva") && t.contains("/temporal") && t.contains("/postos"), "{t}");
    // "o que a ana tem amanhã" não é a própria agenda: vai pro modelo
    let hoje = chrono::NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
    for f in ["o que a ana tem amanhã?", "agenda do pedro amanhã", "a ana tá livre sexta?"] {
        assert!(atalho(f, hoje).is_none(), "{f}");
    }
    // com lat/lon mas chuva_aviso false: sem aviso automático
    bot.config.clima_lat = config_completo().clima_lat;
    bot.config.chuva_aviso = false;
    assert!(!bot.rain_tick(&at("2026-10-14T21:00")));
}

/// Pedro ↔ Ana na mesma máquina: o Argos lê a memória (sem o privado) e as tarefas do bot dela e manda
/// recado; o tick do bot dela entrega (tarefa entra na hora, mensagem espera o horário)
#[test]
fn consulta_e_recado_entre_bots() {
    let tmp = TempDir::new("argos_recado.");
    let motor = tmp.0.join("motor");
    copia(&fixture("motor"), &motor);
    let (dir_a, dir_b) = (tmp.0.join("argos"), tmp.0.join("ana"));
    for d in [&dir_a, &dir_b] {
        std::fs::create_dir_all(d.join("dados")).unwrap();
    }
    let mut argos = bot_de_teste(&dir_a, &motor);
    let mut dela = bot_de_teste(&dir_b, &motor);
    let now = crate::util::agora();
    // sem `dir`: nem as ferramentas nem o `privado` existem
    let nomes = |c: &crate::bot::Config| agenda_tools(c).as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert!(!nomes(&config_dela()).contains(&"consultar_parceiro".to_string()));
    assert!(!agenda_tools(&config_dela()).to_string().contains("privado"));
    let c = config_dela_com_dir(&dir_b.to_string_lossy());
    assert!(nomes(&c).contains(&"consultar_parceiro".to_string()) && nomes(&c).contains(&"mandar_recado".to_string()));
    let s = crate::agenda::agenda_system(&c);
    assert!(s.contains("- Recado pra Pedro: mandar_recado. 'lembra o Pedro de X', 'pede pra ele X'"), "{s}");
    assert!(s.contains("quando ela pedir pra esconder algo"), "{s}");

    // memória da Ana: um fato normal e um privado (este pela ferramenta, como o Haiku faria)
    dela.config = config_dela_com_dir(&dir_a.to_string_lossy());
    dela.run_tool("lembrar", &json!({"texto": "Prova de Solos dia 20/10", "valido_ate": null, "evento_ref": null, "privado": false})).unwrap();
    dela.run_tool("lembrar", &json!({"texto": "Presente surpresa do Pedro: relógio", "valido_ate": null, "evento_ref": null, "privado": true})).unwrap();
    assert!(dela.system_extra().unwrap().contains("relógio 🔒 privado"));
    crate::tarefas::new_task(&mut dela.state, "Regar as plantas", "2099-01-01T09:00", None, false, "09:00").unwrap();
    crate::tarefas::save_state(&dela.state, &dela.state_path).unwrap();

    argos.config.agenda_parceiro = Some(serde_json::from_value(json!({"nome": "Ana", "calendar_id": "j@exemplo.com", "ela": true, "dir": dir_b})).unwrap());
    let presente = json!({"texto": "Comprar presente do Pedro", "quando": "2099-01-01", "repetir_dias": null, "contar_desde_feito": false,
                          "janelas": [], "habito": false, "cobrar_a_cada_h": null, "privado": true});
    dela.run_tool("criar_tarefa", &presente).unwrap();
    let id = dela.state.tarefas.last().unwrap().id;
    assert!(dela.run_tool("listar_tarefas", &json!({})).unwrap().contains("\"privada\": true"));
    let r = argos.run_tool("consultar_parceiro", &json!({})).unwrap();
    assert!(r.contains("Prova de Solos") && r.contains("Regar as plantas") && !r.contains("relógio") && !r.contains("presente"), "{r}");
    // tirar o privado sem mexer no resto
    dela.run_tool("adiar_tarefa", &json!({"id": id, "privado": false})).unwrap();
    assert_eq!(dela.agenda.actions.last().unwrap(), "🔓 tarefa visível: Comprar presente do Pedro");
    assert!(argos.run_tool("consultar_parceiro", &json!({})).unwrap().contains("presente"));

    let depois = crate::util::iso(&(now + chrono::Duration::hours(3)));
    let depois_hm = &depois[..16];
    argos.run_tool("mandar_recado", &json!({"texto": "Pegar o pão", "quando": depois_hm, "tarefa": true})).unwrap();
    argos.run_tool("mandar_recado", &json!({"texto": "Cheguei em casa", "quando": null, "tarefa": false})).unwrap();
    argos.run_tool("mandar_recado", &json!({"texto": "Boa prova!", "quando": depois_hm, "tarefa": false})).unwrap();
    assert!(argos.agenda.actions.iter().any(|a| a.starts_with("📨 tarefa pra Ana: Pegar o pão — ")), "{:?}", argos.agenda.actions);

    let ok = json!({"ok": true, "result": {"message_id": 7}});
    let (url, rx) = claude_falso(vec![(200, ok.clone()); 4]);
    dela.tg.base = url.trim_end_matches("/v1/messages").to_string();
    assert_eq!(dela.recados_tick(&now).unwrap(), 2);
    let m1 = rx.recv().unwrap().corpo;
    assert!(m1["text"].as_str().unwrap().starts_with("📨 Tarefa do Pedro: Pegar o pão — "), "{m1}");
    assert_eq!(rx.recv().unwrap().corpo["text"], "📨 Recado do Pedro: Cheguei em casa");
    let t = dela.state.tarefas.iter().find(|t| t.texto == "Pegar o pão (pedido do Pedro)").expect("tarefa do recado");
    assert_eq!(&t.proximo[..16], depois_hm);
    assert_eq!(crate::tarefas::load_state(&dela.state_path).unwrap().tarefas.len(), 3); // gravou (plantas, presente, pão)
    // a mensagem marcada pra depois espera na caixa
    assert_eq!(dela.recados_tick(&now).unwrap(), 0);
    assert_eq!(dela.recados_tick(&(now + chrono::Duration::hours(4))).unwrap(), 1);
    assert_eq!(rx.recv().unwrap().corpo["text"], "📨 Recado do Pedro: Boa prova!");
    assert!(!dir_b.join("dados/recados.jsonl").exists() && !dir_b.join("dados/recados.lendo").exists());
    // recado pra pasta que não existe: erro pro modelo
    argos.config.agenda_parceiro.as_mut().unwrap().dir = Some(tmp.0.join("nada").to_string_lossy().into());
    assert!(argos.run_tool("mandar_recado", &json!({"texto": "x", "quando": null, "tarefa": false})).is_err());
}

/// horário livre: uma linha por dia com os buracos dele; 'juntos' só existe no horarios_livres com parceiro
#[test]
fn horarios_livres_por_dia_e_juntos() {
    use crate::agenda::linhas_livres;
    let t = |s: &str| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").unwrap();
    let livres = [(t("2026-10-14T21:00"), t("2026-10-14T23:00")), (t("2026-10-15T06:30"), t("2026-10-15T08:00")),
                  (t("2026-10-15T12:00"), t("2026-10-15T13:00")), (t("2026-10-16T22:00"), t("2026-10-16T23:00"))];
    assert_eq!(linhas_livres(&livres, 7), ["qua 14/10 · 21h–23h", "qui 15/10 · 06h30–08h, 12h–13h", "sex 16/10 · 22h–23h"]);
    assert_eq!(linhas_livres(&livres, 2).len(), 2);
    let tools = agenda_tools(&config_dela());
    let de = |n: &str| tools.as_array().unwrap().iter().find(|t| t["name"] == n).unwrap()["input_schema"]["properties"]["de"]["enum"].clone();
    assert_eq!(de("horarios_livres"), json!(["eu", "Pedro", "juntos"]));
    assert_eq!(de("listar_eventos"), json!(["eu", "Pedro"]));
}
