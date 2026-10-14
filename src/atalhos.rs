//! Atalhos determinísticos: perguntas comuns respondidas pelo código, sem chamar o Claude.
//! Só leitura (agenda de um dia/semana, manutenção vencida, tarefas) e o desfazer — o que grava
//! continua indo pro modelo. Na dúvida, `atalho` devolve None e a mensagem segue pro Haiku.
use crate::agenda::fold;
use chrono::{Datelike, Duration, NaiveDate};
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, PartialEq)]
pub enum Atalho {
    /// eventos de `inicio` até `inicio + dias` (exclusivo)
    Agenda { inicio: NaiveDate, dias: i64, rotulo: String },
    /// chaves (pedaços do nome da chave no JSON do veículo) pra filtrar; vazio = tudo
    Manutencao { filtro: Vec<&'static str> },
    Tarefas,
    Desfazer,
    /// previsão de chuva de um dia (clima.rs)
    Chuva { dia: NaiveDate, rotulo: String },
    /// onde abastecer (postos.rs)
    Postos,
    /// alerta de temporal do município configurado (temporal.rs)
    Temporal,
    /// consumo e custo (resumo do show_cost_analysis); Some = nome do veículo sem acento (`fold`)
    Custos { veiculo: Option<String> },
}

/// o que muda de instância pra instância nas frases: como se chama cada veículo e o lugar do temporal
#[derive(Debug, Default, Clone)]
pub struct Nomes {
    /// (apelido, nome do veículo), tudo com `fold`: "titan" → "honda cg 160 titan", "moto" → idem
    pub veiculos: Vec<(String, String)>,
    /// `alerta_nome` com `fold` ("ponta grossa"), se o temporal está ligado
    pub lugar: Option<String>,
}

impl Nomes {
    /// `veiculos` = (nome, é moto). Apelido = cada palavra do nome com 3+ letras; "moto"/"carro" quando só
    /// tem um veículo daquele tipo
    pub fn de(veiculos: &[(String, bool)], lugar: Option<&str>) -> Nomes {
        let mut v = vec![];
        for (tipo, moto) in [("moto", true), ("carro", false)] {
            let deste: Vec<&String> = veiculos.iter().filter(|(_, m)| *m == moto).map(|(n, _)| n).collect();
            if let [n] = deste[..] {
                v.push((tipo.to_string(), fold(n)));
            }
        }
        for (n, _) in veiculos {
            let f = fold(n);
            for w in f.split_whitespace().filter(|w| w.chars().count() >= 3 && !w.chars().all(|c| c.is_ascii_digit())) {
                if !v.iter().any(|(a, _)| a == w) {
                    v.push((w.to_string(), f.clone()));
                }
            }
        }
        Nomes { veiculos: v, lugar: lugar.map(fold).filter(|l| !l.is_empty()) }
    }

    /// "(veiculo|moto|carro|titan|…)" pro regex
    fn alternativas(&self) -> String {
        let mut a = vec!["veiculo".to_string()];
        a.extend(self.veiculos.iter().map(|(w, _)| regex::escape(w)));
        format!("({})", a.join("|"))
    }

    fn qual(&self, apelido: &str) -> Option<String> {
        self.veiculos.iter().find(|(a, _)| a == apelido).map(|(_, n)| n.clone())
    }
}

const DIAS: [&str; 7] = ["segunda", "terca", "quarta", "quinta", "sexta", "sabado", "domingo"];

/// palavra da pergunta → pedaço da chave do item (limpar_lubrificar_corrente, calibrar_pneus…)
const ITENS: [(&str, &[&str]); 8] = [
    ("calibr", &["calibrar"]),
    ("pneu", &["pneus"]),
    ("lubrif", &["lubrificar"]),
    ("estic", &["esticar"]),
    ("corrente", &["corrente"]),
    ("oleo", &["oleo"]),
    ("filtro de ar", &["filtro_ar"]),
    ("freio", &["freios"]),
];

/// resposta do /atalhos: o que o código responde sozinho (sem gastar o Claude). Exemplos com os veículos
/// (`nomes`) e o lugar do temporal desta instância; quem chama tira as linhas do que está desligado
pub fn texto_atalhos(nomes: &Nomes, lugar: &str) -> String {
    // "a moto" / "o carro" / "o onix": o primeiro apelido é o do tipo, quando tem
    let artigo = |a: &str| if a == "moto" { "a" } else { "o" };
    let a1 = nomes.veiculos.first().map(|(a, _)| a.as_str()).unwrap_or("carro");
    let a2 = nomes.veiculos.get(1).filter(|(_, n)| Some(n) != nomes.veiculos.first().map(|(_, n)| n)).map(|(a, _)| a.as_str());
    let ex1 = format!("{} {a1}", artigo(a1));
    let custos1 = format!("d{} {a1}", artigo(a1));
    let custos2 = a2.map(|a| format!(", <i>quanto gasto com {} {a}?</i>", artigo(a))).unwrap_or_default();
    let lugar = lugar.to_lowercase();
    [
        "⚡ <b>Atalhos</b> (respondo na hora, sem IA):\n".to_string(),
        "\n".into(),
        "📅 /hoje · /amanha · /semana · /agenda — ou <i>o que tenho sexta?</i>\n".into(),
        "✅ /tarefas — ou <i>minhas tarefas</i>\n".into(),
        format!("🔧 /manutencao — ou <i>como tá {ex1}?</i>, <i>quando calibrei os pneus?</i>\n"),
        "🌧️ /chuva — ou <i>vai chover amanhã?</i>, <i>previsão sexta</i>\n".into(),
        format!("⛈️ /temporal — ou <i>alerta {lugar}</i>, <i>como tá o tempo em {lugar}?</i> (aviso sozinho se vier temporal)\n"),
        "⛽ /postos — ou <i>qual posto compensa?</i>, <i>onde abastecer?</i> (manda a localização antes pra ver os de perto)\n".into(),
        format!("🧾 /custos — ou <i>média km/l {custos1}</i>{custos2}\n"),
        "↩️ /desfazer — desfaz a última gravação\n".into(),
        "🧠 /memoria — revisar o que eu guardei\n".into(),
        "\n".into(),
        "O resto (marcar, apagar, perguntas abertas) vai pra IA. Tudo: /ajuda".into(),
    ]
    .concat()
}

/// primeiro "/comando" da linha (pra tirar do /atalhos o que está desligado)
pub fn comando_da_linha(l: &str) -> Option<&str> {
    let i = l.find('/')?;
    let c = &l[i + 1..];
    Some(&c[..c.find(|ch: char| !ch.is_alphanumeric()).unwrap_or(c.len())])
}

/// menu do "/" no Telegram (setMyCommands)
pub const COMANDOS: [(&str, &str); 12] = [
    ("atalhos", "o que eu respondo na hora"), ("hoje", "agenda de hoje"), ("amanha", "agenda de amanhã"),
    ("semana", "agenda da semana"), ("tarefas", "lista de tarefas"), ("manutencao", "manutenção vencida"),
    ("chuva", "vai chover hoje?"), ("temporal", "alerta de temporal"), ("postos", "onde abastecer"), ("custos", "consumo e custo por veículo"),
    ("desfazer", "desfaz a última gravação"), ("memoria", "revisar memória"),
];

fn re(p: &str) -> Regex {
    Regex::new(p).expect("regex válida")
}

pub fn atalho(text: &str, hoje: NaiveDate, nomes: &Nomes) -> Option<Atalho> {
    static R: OnceLock<[Regex; 7]> = OnceLock::new();
    let [desfaz, tarefas, agenda_cmd, agenda_q, pergunta, fez, chuva] = R.get_or_init(|| {
        let quando = r"(hoje|amanha|depois de amanha|(essa |esta |nessa |nesta )?semana( que vem)?|proxima semana|(segunda|terca|quarta|quinta|sexta|sabado|domingo)(-feira)?)";
        [
            re(r"^/?(desfaz(er)?|desfaz (isso|o ultimo|a ultima)|volta atras)$"),
            re(r"^(/tarefas|minhas tarefas|quais (sao )?(as )?(minhas )?tarefas|lista de tarefas)$"),
            re(&format!(r"^/(agenda|hoje|amanha|semana)$|^(minha )?agenda (de |da |pra |do |dessa |desta )?{quando}$")),
            re(&format!(r"^(o que|oq|q|que) (eu )?(tenho|tem|ha)( marcado| agendado)?( na agenda)?( pra| para| de| na| no| nessa| nesta| essa| esta)? ?{quando}$")),
            re(r"^(quando|quanto|a quanto|ha quanto|qual|quais|o que|oq|que|como|ta|esta|tem|precisa)\b|\?$"),
            re(r"\b(fiz|feito|troquei|calibrei|lubrifiquei|estiquei|registr|grav|anota|marca|lembr|cobra|agenda)"),
            re(r"^/chuva$|^(sera que )?(vai chover|chove|vai ter chuva|tem chuva|previsao( do tempo)?|como (ta|esta|vai estar|vai ficar) o tempo)( (pra|para|de|na|no))? ?(hoje|amanha|depois de amanha|(segunda|terca|quarta|quinta|sexta|sabado|domingo)(-feira)?)?$"),
        ]
    });
    let t = fold(text.trim());
    let t = t.trim_end_matches(['?', '!', '.', ' ']);
    if desfaz.is_match(t) {
        return Some(Atalho::Desfazer);
    }
    if tarefas.is_match(t) {
        return Some(Atalho::Tarefas);
    }
    if agenda_cmd.is_match(t) || agenda_q.is_match(t) {
        return Some(quando(t, hoje));
    }
    if postos().is_match(t) {
        return Some(Atalho::Postos);
    }
    if temporal(nomes).is_match(t) {
        return Some(Atalho::Temporal);
    }
    if let Some(c) = custos(t, fez, nomes) {
        return Some(c);
    }
    if chuva.is_match(t) {
        // sem dia na frase (ou /chuva) = hoje; o quando() só sabe o dia quando a frase diz
        let (dia, rotulo) = match quando(t, hoje) {
            Atalho::Agenda { inicio, dias: 1, rotulo } => (inicio, rotulo),
            _ => (hoje, "hoje".into()),
        };
        return Some(Atalho::Chuva { dia, rotulo });
    }
    let original = fold(text.trim());
    if t == "/manutencao" {
        return Some(Atalho::Manutencao { filtro: vec![] });
    }
    // pergunta sobre manutenção (não "fiz X", que grava e vai pro modelo)
    let v = nomes.alternativas();
    let situacao = re(&format!(r"\b(situacao|estado) d[aoe] (minha |meu )?{v}\b|^como (ta|esta|anda|vai) (a |o )?(minha |meu )?{v}$"));
    let fala_de_manutencao = t.contains("manutenc") || t.contains("vencid") || t.contains("atrasad")
        || ITENS.iter().any(|(w, _)| t.contains(w)) || situacao.is_match(t);
    let comeca_com_pergunta = pergunta.is_match(&original);
    let so_pergunta = !fez.is_match(t) || re(r"^(quando|quanto|a quanto|ha quanto)\b").is_match(t);
    if fala_de_manutencao && comeca_com_pergunta && so_pergunta {
        let mut filtro: Vec<&'static str> = ITENS.iter().filter(|(w, _)| t.contains(w)).flat_map(|(_, k)| k.iter().copied()).collect();
        filtro.dedup();
        return Some(Atalho::Manutencao { filtro });
    }
    None
}

fn postos() -> &'static Regex {
    static P: OnceLock<Regex> = OnceLock::new();
    P.get_or_init(|| re(r"^/?postos?$|^(qual|que) (o |e o )?(melhor )?posto( (e )?(o )?(mais barato|compensa|vale (mais )?a pena|melhor))?( pra abastecer| pra encher)?( (hoje|agora|aqui))?$|^(onde|aonde) (eu )?(abasteco|abastecer|vale abastecer|compensa abastecer|e mais barato abastecer)( hoje| agora)?$|^melhor posto( (hoje|agora|aqui|perto))?$|^posto mais barato( (hoje|agora|aqui|perto))?$"))
}

/// "média km/l da moto", "quanto gasto com o carro?", "/custos": palavra de custo + veículo ou km/L.
/// Com período ("em setembro", "esse mês") ou cara de gravação ("abasteci…") fica com o modelo.
fn custos(t: &str, fez: &Regex, nomes: &Nomes) -> Option<Atalho> {
    static R: OnceLock<[Regex; 3]> = OnceLock::new();
    let [custo, km, fora] = R.get_or_init(|| [
        re(r"\b(media|consumo|custos?|gast\w*|km ?/ ?l|kml|km por litro|faz quantos km)\b"),
        re(r"\b(km ?/ ?l|kml|km por litro|por km|consumo|analise de custos?)\b"),
        re(r"\b(desde|ontem|semana|mes|meses|ano|anos|ultim\w*|compar\w*|vs|janeiro|fevereiro|marco|abril|maio|junho|julho|agosto|setembro|outubro|novembro|dezembro|abasteci\w*|paguei|coloquei|botei)\b"),
    ]);
    let v = nomes.alternativas();
    if let Some(m) = re(&format!(r"^/(custos?|consumo)( {v})?$")).captures(t) {
        return Some(Atalho::Custos { veiculo: m.get(3).and_then(|v| nomes.qual(v.as_str())) });
    }
    let v = re(&format!(r"\b{v}\b")).captures(t);
    if !custo.is_match(t) || (v.is_none() && !km.is_match(t)) || fora.is_match(t) || fez.is_match(t) {
        return None;
    }
    Some(Atalho::Custos { veiculo: v.and_then(|m| nomes.qual(&m[1])) })
}

/// "/temporal", "alerta ponta grossa", "como tá o tempo em ponta grossa?" (o lugar é o `alerta_nome`)
fn temporal(nomes: &Nomes) -> Regex {
    let Some(l) = &nomes.lugar else { return re(r"^/temporal$") };
    let cmd = regex::escape(&l.replace(' ', ""));
    let l = regex::escape(l);
    re(&format!(r"^/(temporal|{cmd})$|^(alertas?|temporal|tempo|previsao|clima|vai ter temporal|tem temporal|tem alerta|(como )?(ta|esta|vai ficar|vai estar) o tempo|vai chover)( (em|de|pra|para|na|no|la em|la))? {l}( hoje| amanha)?$"))
}

fn quando(t: &str, hoje: NaiveDate) -> Atalho {
    let dia = |d: NaiveDate, rotulo: &str| Atalho::Agenda { inicio: d, dias: 1, rotulo: rotulo.into() };
    if t.contains("depois de amanha") {
        return dia(hoje + Duration::days(2), "depois de amanhã");
    }
    if t.contains("amanha") {
        return dia(hoje + Duration::days(1), "amanhã");
    }
    if t.contains("hoje") {
        return dia(hoje, "hoje");
    }
    if let Some(i) = DIAS.iter().position(|d| t.contains(d)) {
        let ahead = (i as i64 - hoje.weekday().num_days_from_monday() as i64).rem_euclid(7);
        return dia(hoje + Duration::days(ahead), DIAS[i]);
    }
    if t.contains("que vem") || t.contains("proxima") {
        let seg = hoje + Duration::days(7 - hoje.weekday().num_days_from_monday() as i64);
        return Atalho::Agenda { inicio: seg, dias: 7, rotulo: "semana que vem".into() };
    }
    // "semana", "/agenda", "/semana": daqui até o fim de domingo
    let dias = 7 - hoje.weekday().num_days_from_monday() as i64;
    Atalho::Agenda { inicio: hoje, dias: if t.starts_with("/agenda") { 7 } else { dias }, rotulo: "semana".into() }
}

/// blocos do check_maintenance anotado ("⚠️  Nome" + linhas recuadas) → texto de chat.
/// `nomes` = nomes de exibição que passam no filtro (None = todos). Devolve (texto, achou algum).
pub fn manutencao_chat(anotado: &str, nomes: Option<&[String]>) -> (String, bool) {
    let mut out = Vec::new();
    let mut dentro = false;
    let mut achou = false;
    for line in anotado.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with("---") {
            continue;
        }
        if l == "nada vencido" {
            out.push("   nada vencido ✅".into());
            continue;
        }
        if let Some(nome) = l.strip_prefix("⚠️").map(str::trim) {
            dentro = nomes.map(|n| n.iter().any(|x| x == nome)).unwrap_or(true);
            if dentro {
                achou = true;
                out.push(format!("⚠️ <b>{}</b>", crate::rascunho::esc(nome)));
            }
        } else if line.starts_with(' ') {
            if dentro {
                out.push(format!("   {}", crate::rascunho::esc(l)));
            }
        } else {
            dentro = false;
            out.push(format!("🔧 <b>{}</b>", crate::rascunho::esc(l.trim_end_matches(':'))));
        }
    }
    (out.join("\n"), achou)
}
