//! ZONE 6c: agenda (Claude + tool use). Pra acrescentar ferramenta: um `tool(...)` em `agenda_tools`,
//! um braço no `match` de `Bot::run_tool`; pra mexer no prompt: `AGENDA_SYSTEM` (+ `Bot::system_extra`).
//! Os dois são modelos: `agenda_system`/`agenda_tools` trocam a pessoa e tiram o que está desligado no configs.json.
use crate::bot::Bot;
use crate::rede::{blocos, model_params};
use crate::tarefas::{describe_task, fmt_when, memory_block, new_task, remember, save_state, task_move, validity_note, when_to_aware};
use crate::util::{arg, arg_opt_str, arg_str, corta, dia_semana, dumps, parse_iso, so_data, verdade, Erro, R};
use chrono::{DateTime, NaiveDateTime, NaiveTime};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::time::Instant;

fn nullable(kind: &str) -> Value {
    json!({"anyOf": [{"type": kind}, {"type": "null"}]})
}

/// {**_nullable(kind), 'description': ...}
fn nullable_desc(kind: &str, desc: &str) -> Value {
    let mut v = nullable(kind);
    v["description"] = json!(desc);
    v
}

/// _tool: schema estrito (todas as propriedades obrigatórias, sem extras)
fn tool(name: &str, description: &str, props: Value) -> Value {
    let required: Vec<String> = props.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    json!({"name": name, "description": description, "strict": true,
           "input_schema": {"type": "object", "properties": props, "required": required, "additionalProperties": false}})
}

/// _tool_opcional: sem strict; só `required` é obrigatório (os outros o modelo omite)
fn tool_opcional(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({"name": name, "description": description,
           "input_schema": {"type": "object", "properties": props, "required": required, "additionalProperties": false}})
}

/// quem usa esta instância (configs.json → "usuario", obrigatório)
#[derive(Deserialize, Clone, Debug)]
pub struct Usuario {
    pub nome: String,
    /// pronome no prompt: ela/dela em vez de ele/dele
    #[serde(default)]
    pub ela: bool,
    /// o que vai entre parênteses depois do nome ("estudante de biologia em Campinas"); vazio = sem parênteses
    #[serde(default)]
    pub sobre: String,
    /// cidade da previsão de chuva, como aparece no prompt; vazio = sem a cidade
    #[serde(default)]
    pub cidade: String,
    /// nome do bot com artigo ("a Io"), no "Você é o Argos…" do prompt; sem a chave, "o Argos"
    #[serde(default = "argos")]
    pub assistente: String,
}

fn argos() -> String {
    "o Argos".into()
}

/// só pro `Default` do `Config`: o `load_config` não aceita configs.json sem `usuario`
impl Default for Usuario {
    fn default() -> Usuario {
        Usuario { nome: String::new(), ela: false, sobre: String::new(), cidade: String::new(), assistente: argos() }
    }
}

/// agenda de outra pessoa que esta instância só consulta (configs.json → "agenda_parceiro");
/// `calendar_id` = e-mail da agenda principal dela, compartilhada com a conta Google desta instância
#[derive(Deserialize, Clone, Debug)]
pub struct Parceiro {
    pub nome: String,
    pub calendar_id: String,
    #[serde(default)]
    pub ela: bool,
    /// ARGOS_DIR da instância dela na mesma máquina: liga consultar_parceiro (memória e tarefas, menos o privado)
    /// e mandar_recado
    #[serde(default)]
    pub dir: Option<String>,
}

/// troca os marcadores ({ele}, {dele}, {do_nome}, {sobre}, {cidade}) do prompt e das ferramentas
fn personaliza(t: &str, u: &Usuario) -> String {
    let (ele, dele, artigo) = if u.ela { ("ela", "dela", "da") } else { ("ele", "dele", "do") };
    let mut maiusc = ele.chars();
    let ele_m: String = maiusc.next().map(|c| c.to_uppercase().chain(maiusc).collect()).unwrap_or_default();
    let t = if u.sobre.is_empty() { t.replace(" ({sobre})", "") } else { t.to_string() };
    let t = if u.cidade.is_empty() { t.replace(", {cidade}", "") } else { t };
    t.replace("{ele}", ele).replace("{Ele}", &ele_m).replace("{dele}", dele)
        .replace("{do_nome}", &format!("{artigo} {}", u.nome)).replace("{sobre}", &u.sobre).replace("{cidade}", &u.cidade)
        .replace("{assistente}", &u.assistente)
}

/// tira do texto o parágrafo que começa com `inicio` (até a linha em branco seguinte, inclusive)
fn sem_paragrafo(t: &str, inicio: &str) -> String {
    match t.find(&format!("\n{inicio}")) {
        Some(i) => {
            let fim = t[i + 1..].find("\n\n").map(|j| i + 1 + j + 1).unwrap_or(t.len());
            format!("{}{}", &t[..i + 1], &t[fim + 1..])
        }
        None => t.to_string(),
    }
}

/// tira a linha que começa com `inicio` (com o \n)
fn sem_linha(t: &str, inicio: &str) -> String {
    match t.find(&format!("\n{inicio}")) {
        Some(i) => {
            let fim = t[i + 1..].find('\n').map(|j| i + 1 + j).unwrap_or(t.len() - 1);
            format!("{}{}", &t[..i], &t[fim..])
        }
        None => t.to_string(),
    }
}

/// sem veículo ou sem Google: tira do modelo (antes do `personaliza`) o que fala deles
fn sem_o_desligado(t: &str, c: &crate::bot::Config) -> String {
    // o parágrafo do configs.json entra no lugar do marcador (sem ele, o marcador sai com a linha em branco)
    let mut s = match c.prompt_extra.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => t.replacen("{prompt_extra}", p, 1),
        None => sem_paragrafo(t, "{prompt_extra}"),
    };
    if !c.tem_veiculos {
        s = s.replacen(", a manutenção dos veículos e uma memória", " e uma memória", 1);
        s = sem_paragrafo(&s, "MANUTENÇÃO:");
        s = sem_linha(&s, "ABASTECIMENTO é por foto");
        s.push_str("VEÍCULOS: nenhum cadastrado neste bot; se {ele} falar de abastecimento ou manutenção, diga em uma linha que não está configurado.\n");
    }
    if !c.tem_google {
        s = s.replacen("Gerencia a Google Agenda {dele}, uma lista", "Gerencia uma lista", 1);
        s = sem_linha(&s, "- Lista de agenda:");
        s = sem_linha(&s, "- criar_evento só quando");
        s = sem_linha(&s, "- O bot já não avisa nem cobra durante evento");
        s = sem_paragrafo(&s, "Se {ele} se referir a um evento pelo nome");
        // do parágrafo AGENDA só fica a 1ª linha (data e hora de cada mensagem: as tarefas também usam)
        if let Some(i) = s.find("\nAGENDA:\n") {
            let ini = i + "\nAGENDA:\n".len();
            let fim_linha = s[ini..].find('\n').map(|j| ini + j + 1).unwrap_or(s.len());
            let primeira = s[ini..fim_linha].to_string();
            s = format!("{}\nAGENDA: a Google Agenda não está ligada neste bot. Pedido de marcar algo vira tarefa (criar_tarefa); pergunta sobre a agenda: diga em uma linha que ela não está configurada.\n{primeira}\nAGENDA_FORA:\n{}", &s[..i], &s[fim_linha..]);
            s = sem_paragrafo(&s, "AGENDA_FORA:");
        }
    }
    s
}

/// system prompt desta instância: pessoa, agenda do parceiro e só os parágrafos do que está ligado
pub fn agenda_system(c: &crate::bot::Config) -> String {
    let mut s = personaliza(&sem_o_desligado(AGENDA_SYSTEM, c), &c.usuario).replace("{fuso}", &crate::util::nome_do_fuso(c.tz()));
    if !c.ligado("chuva") {
        s = sem_paragrafo(&s, "CHUVA:");
    }
    if let Some(p) = &c.agenda_parceiro {
        let (ela, dela) = if p.ela { ("ela", "dela") } else { ("ele", "dele") };
        let linha = if !c.tem_google { String::new() } else { format!("- Agenda {da} {n}: listar_eventos e horarios_livres com de='{n}' consultam a agenda {dela} (só leitura). \
Nunca crie, edite nem apague lá; pedido pra marcar algo na agenda {dela}: diga que só {ela} marca. Sem 'de' ou de='eu' = a agenda {dono}.\n",
                            da = if p.ela { "da" } else { "do" }, n = p.nome, dono = personaliza("{dele}", &c.usuario)) };
        let mut linha = linha;
        if p.dir.is_some() {
            linha += &personaliza(&format!("- O que {n} guardou no bot {dela} (memória e tarefas): consultar_parceiro. Pergunta sobre {n} (prova, compromisso, \
o que {ela} tem que fazer) → consultar_parceiro E listar_eventos com de='{n}'. Horário livre dos dois → horarios_livres com de='juntos' (uma chamada só). Fato ou tarefa com privado=true não aparece pra {n}: quando {{ele}} pedir \
pra esconder algo ('isso é privado', 'não conta pra {n}'), privado=true no lembrar/corrigir_memoria/criar_tarefa/adiar_tarefa; o resto {a} {n} consegue ver.
- Recado pra {n}: mandar_recado. 'lembra {a} {n} de X', 'pede pra {ela} X' → tarefa=true (cobra no bot {dela}); 'avisa/fala pra {n} que…' → tarefa=false (só a mensagem).\n",
                n = p.nome, a = if p.ela { "a" } else { "o" }), &c.usuario);
        }
        s = s.replacen("\nTAREFA ou EVENTO", &format!("{linha}\nTAREFA ou EVENTO"), 1);
    }
    s
}

/// ferramentas que precisam da Google Agenda
const EVENTOS: [&str; 5] = ["listar_eventos", "horarios_livres", "criar_evento", "editar_evento", "apagar_evento"];

/// ferramentas desta instância: textos personalizados, `de` nas consultas de agenda se tem parceiro,
/// sem previsao_chuva com a chuva desligada
pub fn agenda_tools(c: &crate::bot::Config) -> Value {
    let texto = personaliza(&ferramentas().to_string(), &c.usuario).replace("{fuso}", &crate::util::nome_do_fuso(c.tz()));
    let mut tools: Value = serde_json::from_str(&texto).expect("ferramentas em JSON");
    let lista = tools.as_array_mut().expect("lista de ferramentas");
    if !c.ligado("chuva") {
        lista.retain(|t| t["name"] != "previsao_chuva");
    }
    if !c.tem_veiculos {
        lista.retain(|t| !["ver_manutencao", "registrar_manutencao"].contains(&t["name"].as_str().unwrap_or_default()));
    }
    if !c.tem_google {
        lista.retain(|t| !EVENTOS.contains(&t["name"].as_str().unwrap_or_default()));
    }
    if let Some(p) = &c.agenda_parceiro {
        for t in lista.iter_mut().filter(|t| t["name"] == "listar_eventos" || t["name"] == "horarios_livres") {
            let livres = t["name"] == "horarios_livres";
            let schema = &mut t["input_schema"];
            schema["properties"]["de"] = if livres {
                json!({"type": "string", "enum": ["eu", p.nome, "juntos"],
                       "description": format!("de quem é a agenda: 'eu', '{}' (só consulta) ou 'juntos' (livre nas duas ao mesmo tempo)", p.nome)})
            } else {
                json!({"type": "string", "enum": ["eu", p.nome], "description": format!("de quem é a agenda: 'eu' ou '{}' (só consulta)", p.nome)})
            };
            schema["required"].as_array_mut().expect("required").push(json!("de"));
        }
        if p.dir.is_some() {
            for t in lista.iter_mut().filter(|t| ["lembrar", "corrigir_memoria", "criar_tarefa", "adiar_tarefa"].contains(&t["name"].as_str().unwrap_or_default())) {
                let manter = match t["name"].as_str() {
                    Some("corrigir_memoria") => "; ao corrigir, mantenha o que o fato já tem (🔒 privado na memória = true)",
                    Some("adiar_tarefa") => "; omitido = mantém",
                    _ => "",
                };
                let oque = if t["name"].as_str().is_some_and(|n| n.contains("tarefa")) { "esta tarefa" } else { "este fato" };
                let strict = t.get("strict").is_some();
                let schema = &mut t["input_schema"];
                schema["properties"]["privado"] = json!({"type": "boolean",
                    "description": personaliza(&format!("true = {} não vê {oque} quando consultar; false = normal (só true se {{ele}} pedir pra esconder){manter}", p.nome), &c.usuario)});
                if strict {
                    schema["required"].as_array_mut().expect("required").push(json!("privado"));
                }
            }
            let (dela, ela) = if p.ela { ("dela", "ela") } else { ("dele", "ele") };
            lista.push(tool("consultar_parceiro",
                &format!("O que {} guardou no bot {dela}: a memória e as tarefas (pendentes e próximas), menos o que for privado. \
Só leitura. Pra agenda {dela} use listar_eventos com de='{}'.", p.nome, p.nome), json!({})));
            lista.push(tool("mandar_recado",
                &format!("Manda um recado pro bot {dela} ({}): tarefa=true vira tarefa que cobra {ela} no horário; tarefa=false é só uma mensagem.", p.nome),
                json!({"texto": {"type": "string", "description": format!("o recado, curto, como {} vai ler (sem dizer de quem é: o bot já diz)", p.nome)},
                       "quando": nullable_desc("string", "'AAAA-MM-DDTHH:MM' em que chega/vence; null = agora"),
                       "tarefa": {"type": "boolean", "description": "true = tarefa que cobra até marcar feito; false = só mensagem"}})));
        }
    }
    tools
}

/// o que o bot do parceiro vê deste estado: memória e tarefas, sem as privadas (nem os fatos vencidos)
pub fn visivel_ao_parceiro(estado: &crate::tarefas::Estado, nome: &str) -> R<Value> {
    let hoje = crate::util::agora().format("%Y-%m-%d").to_string();
    let memoria: Vec<String> = estado.memoria.iter()
        .filter(|f| !crate::tarefas::privado(&f.resto) && f.ate.as_deref().is_none_or(|a| a.is_empty() || a >= hoje.as_str()))
        .map(|f| format!("{}{}", f.texto, validity_note(f)))
        .collect();
    let tarefas: R<Vec<Value>> = estado.tarefas.iter().filter(|t| !crate::tarefas::privado(&t.resto)).map(describe_task).collect();
    Ok(json!({"de": nome, "memoria": memoria, "tarefas": tarefas?}))
}

const QUANDO: &str = "'AAAA-MM-DDTHH:MM' (horário de {fuso}, sem fuso) ou 'AAAA-MM-DD' para dia inteiro";

/// ferramentas com os marcadores de `personaliza` (quem usa é o `agenda_tools`)
fn ferramentas() -> Value {
    let quando = || json!({"type": "string", "description": QUANDO});
    json!([
        tool("listar_eventos",
             "Lista eventos da agenda principal entre duas datas. Use antes de editar/apagar para achar o id, \
e para responder perguntas sobre a agenda. Eventos recorrentes vêm expandidos (cada ocorrência tem id próprio).",
             json!({"inicio": quando(), "fim": quando(),
                    "busca": nullable_desc("string", "palavra pra filtrar no título/local/descrição (ignora acento e maiúscula) ou null pra tudo")})),
        tool("horarios_livres",
             "Horários livres na agenda dentro das janelas dadas (ex.: o horário útil da memória), já descontando os eventos. \
Use pra 'próximo horário livre/útil', 'quando dá pra marcar X'. Copie as linhas_prontas, uma por linha (uma por dia), sem recalcular nem juntar numa frase.",
             json!({"janelas": {"type": "array", "items": {"type": "string"}, "description": "janelas de cada dia, 'HH:MM-HH:MM' (ex.: ['08:00-11:59', '14:00-17:49'])"},
                    "inicio": {"type": "string", "description": "'AAAA-MM-DDTHH:MM' de onde começar (pra 'próximo': a hora atual da mensagem)"},
                    "dias": {"type": "integer", "description": "quantos dias olhar, contando o do início (1 a 14; pra 'próximo': 7)"},
                    "duracao_min": {"type": "integer", "description": "tamanho mínimo do buraco em minutos (sem duração dita: 30)"}})),
        tool("criar_evento",
             "Cria um evento na Google Agenda (NÃO manda aviso pelo bot). Só pra compromissos: \
quando {ele} pede pra marcar/agendar, ou consulta, reunião, aula, prova.",
             json!({"titulo": {"type": "string"},
                    "inicio": quando(),
                    "fim": {"type": "string", "description": format!("{QUANDO}. Sem duração dita: 1h depois. Dia inteiro: dia seguinte ao último dia.")},
                    "local": nullable("string"),
                    "descricao": nullable("string"),
                    "lembrete_min": nullable_desc("integer", "minutos antes pra notificar; null = padrão da agenda")})),
        tool("editar_evento", "Altera um evento existente. Campos null ficam como estão.",
             json!({"id": {"type": "string"},
                    "titulo": nullable("string"),
                    "inicio": nullable_desc("string", QUANDO),
                    "fim": nullable_desc("string", QUANDO),
                    "local": nullable("string"),
                    "descricao": nullable("string")})),
        tool("apagar_evento",
             "Pede pra apagar um evento. NÃO apaga na hora: mostra um botão de confirmação pro usuário. \
Um evento por chamada.",
             json!({"id": {"type": "string"}})),
        tool("criar_tarefa",
             "Lembrete/tarefa do bot: na hora certa o bot MANDA MENSAGEM no Telegram e cobra a cada poucas horas até \
{ele} marcar feito. Use pra todo \"me lembra/me avisa/me cobra\" e rotinas (varrer a casa, regar planta, \
comprar algo, levantar do sofá daqui 3 min).",
             json!({"texto": {"type": "string", "description": "a tarefa, curta, como {ele} vai ler na cobrança"},
                    "quando": {"type": "string", "description": "quando vence: 'AAAA-MM-DDTHH:MM' se {ele} deu horário; senão só 'AAAA-MM-DD' (o bot usa o horário padrão). Nunca invente horário."},
                    "repetir_dias": nullable_desc("integer", "repete a cada N dias (1 diário, 7 semanal, 30 mensal) ou null pra uma vez só"),
                    "contar_desde_feito": {"type": "boolean", "description": "true = a próxima conta a partir de quando {ele} marcar feito ('a cada 7 dias desde a última vez'); false = agenda fixa ('toda sexta')"},
                    "janelas": {"type": "array", "items": {"type": "string"}, "description": "só cobrar dentro destas janelas 'HH:MM-HH:MM' (ex.: horário útil da memória, se {ele} pedir 'só em horário útil'); [] = padrão"},
                    "habito": {"type": "boolean", "description": "true = hábito: cada vez que {ele} fizer vira uma linha no tracker (histórico pra ajustar a frequência) e concluir não pede confirmação. Só se {ele} pedir pra acompanhar/registrar ou chamar de hábito"},
                    "cobrar_a_cada_h": nullable_desc("number", "horas entre uma cobrança e outra até {ele} marcar feito ('me cobra 1x por dia' = 24); null = padrão")})),
        tool("listar_tarefas", "Lista as tarefas do bot: pendentes (já venceram) e próximas.", json!({})),
        tool("concluir_tarefa", "Marca uma tarefa como feita ({ele} disse que fez).", json!({"id": {"type": "integer"}})),
        tool("apagar_tarefa", "Remove uma tarefa da lista de vez (não é concluir).", json!({"id": {"type": "integer"}})),
        tool_opcional("adiar_tarefa",
             "Muda o horário de uma tarefa que já existe (adiar, remanejar, 'me lembra disso às 18'). \
Pendente: a próxima cobrança vai pro horário novo e a repetição não muda. Ainda não venceu: o vencimento vai pro horário novo. \
Também muda janelas e intervalo de cobrança de uma tarefa que já existe.",
             json!({"id": {"type": "integer"},
                    "quando": {"type": "string", "description": "'AAAA-MM-DDTHH:MM' (horário de {fuso}); só 'AAAA-MM-DD' mantém o horário da tarefa; omitido = não mexe no horário (só janelas/intervalo)"},
                    "janelas": {"type": "array", "items": {"type": "string"}, "description": "só cobrar dentro destas janelas 'HH:MM-HH:MM' (ex.: horário útil da memória, se {ele} pedir 'só em horário útil'); [] = mantém as que ela já tem"},
                    "cobrar_a_cada_h": {"type": "number", "description": "novo intervalo entre cobranças, em horas; omitido = mantém"}}),
             &["id"]),
        tool("pausar_tarefas",
             "Pausa tarefas até uma data (viagem, folga, 'não me cobra nada no fim de semana'): nada vence nem cobra até lá. \
Repetição fixa ('todo dia') pula o que cairia na pausa; o resto volta a cobrar quando ela acabar.",
             json!({"ids": {"type": "array", "items": {"type": "integer"}, "description": "ids das tarefas; [] = todas"},
                    "ate": nullable_desc("string", "'AAAA-MM-DDTHH:MM' em que volta a cobrar; só 'AAAA-MM-DD' = volta no começo desse dia; null = tira a pausa agora")})),
        tool("lembrar", "Salva um fato na memória (uma frase autocontida). Fato temporário ganha validade e some sozinho depois.",
             json!({"texto": {"type": "string"},
                    "valido_ate": nullable_desc("string", "'AAAA-MM-DD': último dia em que o fato vale; null = permanente ou quando usar evento_ref"),
                    "evento_ref": nullable_desc("string", "se o fato é sobre um evento da agenda ou {ele} disse 'até lá': uma palavra do nome do evento \
(ex.: 'estagio'); o bot acha o evento e usa o fim dele como validade. null se não for sobre evento")})),
        tool("corrigir_memoria", "Reescreve um fato da memória que mudou.",
             json!({"id": {"type": "integer"}, "texto": {"type": "string"},
                    "valido_ate": nullable_desc("string", "'AAAA-MM-DD' ou null = permanente")})),
        tool("esquecer", "Apaga um fato da memória permanente.", json!({"id": {"type": "integer"}})),
        tool("desfazer_ultimo", "Desfaz o último grupo de ações gravadas pelo bot (abastecimento, manutenção, evento criado/alterado/apagado, tarefa, memória). Use quando {ele} pedir pra desfazer/voltar atrás/\"não era isso\".", json!({})),
        tool("ver_manutencao",
             "Manutenções vencidas dos veículos, pelo check_maintenance do motoregis (CSVs do MotorCostTracker): \
cada item vencido com o atraso já calculado, há quanto tempo foi feito, o prazo e a data da última vez. \
Use pra qualquer pergunta sobre manutenção, atraso ou o que está vencido.", json!({})),
        tool_opcional("previsao_chuva",
             "Previsão de chuva hora a hora de um dia (até 7 dias à frente): os períodos com probabilidade >= limiar e cada hora com alguma chance (% e mm). Hoje começa da hora atual.",
             json!({"dia": {"type": "string", "description": "AAAA-MM-DD"},
                    "limiar": {"type": "number", "description": "% mínimo pros períodos; omitido = o do aviso automático (70)"}}),
             &["dia"]),
        tool_opcional("registrar_manutencao",
             "Registra manutenção preventiva feita num veículo do MotorCostTracker (ex.: lubrificou a corrente, calibrou pneus). \
Sem odometro, a km é estimada pela média km/dia dos abastecimentos e {ele} confirma num botão antes de gravar.",
             json!({"veiculo": {"type": "string"},
                    "itens": {"type": "array", "items": {"type": "string"}, "description": "chaves exatas da lista de itens do veículo"},
                    "custo_total": {"type": "number", "description": "R$ total; omitido se {ele} não disse (grava 0)"},
                    "data": {"type": "string", "description": "AAAA-MM-DD do dia em que {ele} fez ('ontem' = a data de ontem pelo carimbo); omitido = hoje"},
                    "odometro": {"type": "number", "description": "km do odômetro, só se {ele} disse; omitido = estimada"}}),
             &["veiculo", "itens"]),
    ])
}

/// modelo do system prompt (marcadores de `personaliza`); quem usa é o `agenda_system`
const AGENDA_SYSTEM: &str = concat!(
    "Você é {assistente}, assistente pessoal {do_nome} no Telegram ({sobre}). Gerencia a Google Agenda {dele}, uma lista própria de tarefas, a manutenção dos veículos e uma memória permanente, tudo pelas ferramentas. Toda ação só acontece se você chamar a ferramenta; nunca diga que fez algo sem a ferramenta ter respondido ok nesta mesma mensagem.\n",
    "\n",
    "ESTILO — isto é o mais importante:\n",
    "- Responda em 1 a 3 linhas. Chat, não e-mail. Português do Brasil, sem markdown.\n",
    "- Responda só o que foi perguntado. Sem sugestões não pedidas, sem explicar limitações, sem repetir o pedido, sem 'posso ajudar em mais algo'.\n",
    "- Não achou? Diga em uma linha e faça no máximo uma pergunta.\n",
    "- Lista de agenda: copie as `linhas_prontas` que listar_eventos devolve, TODAS, sem cortar nem reescrever (pode tirar o dia da frente se a pergunta for de um dia só). Descrição só se {ele} pedir.\n",
    "\n",
    "AGENDA:\n",
    "- Cada mensagem vem com data e hora atuais de {fuso}; use pra resolver 'amanhã', 'quinta', 'semana que vem'. 'Quinta' sozinho = a próxima quinta (hoje, se hoje é quinta e o horário não passou).\n",
    "- Pra editar ou apagar, liste antes pra achar o evento. Mais de um candidato: pergunte qual.\n",
    "- Procurando um evento por nome, use também os apelidos/termos da memória (ex.: apelido de uma pessoa).\n",
    "- Horário ambíguo ('às 8'): assuma o mais provável e diga o que assumiu.\n",
    "- 'Esse horário', 'lá', 'nesse dia' sem o horário/dia aparecer na conversa (o histórico zera quando o bot reinicia): pergunte qual, nunca use a hora atual no lugar.\n",
    "- Depois de criar/editar: uma linha com título, dia da semana, data e horário.\n",
    "- Eventos 'Sono' são a rotina de dormir: ignore em resumos, a menos que {ele} pergunte.\n",
    "- Apagar passa pelo botão de confirmação: diga que está esperando {ele} confirmar.\n",
    "- Horário livre e horário útil são coisas diferentes, os dois pelo horarios_livres: 'horário útil' (ou 'quando dá pra marcar' algo de trabalho/estudo) = janelas de horário útil da memória; 'horário livre' sem 'útil' = janela 00:00-23:59 (o Sono conta como ocupado). Responda com a primeira das linhas_prontas; nunca dê horário útil fora das janelas.\n",
    "\n",
    "TAREFA ou EVENTO — decida assim:\n",
    "- 'me lembra', 'me avisa', 'me cobra', 'não me deixa esquecer', 'todo dia/toda semana fazer X' → criar_tarefa. Só a tarefa faz o bot mandar mensagem no Telegram na hora. Na dúvida, é tarefa.\n",
    "- Lista de coisas no mesmo pedido ('às 6:30 me lembra de X, Y e Z'): uma criar_tarefa por item, cada uma no seu horário (mesmo horário = a cobrança chega numa mensagem só, com um ✅ por item). Nunca junte a lista numa tarefa só.\n",
    "- criar_evento só quando {ele} pedir pra 'marcar'/'agendar'/'pôr na agenda', ou for compromisso com outras pessoas ou lugar (consulta, reunião, aula, prova).\n",
    "- contar_desde_feito = false por padrão ('todo dia', 'toda sexta' = horário fixo). true SÓ se {ele} disser 'desde a última vez', 'depois que eu fizer' ou equivalente; aí, sem data, pergunte quando foi a última vez.\n",
    "- 'daqui N minutos/horas': calcule a partir da hora atual da mensagem.\n",
    "- Se {ele} disser que fez algo da lista de tarefas: concluir_tarefa.\n",
    "- 'Quando é a próxima X', 'quando tenho que X', 'lembrete de X': procure primeiro no listar_tarefas (tarefa que repete não está na agenda; apelidos da memória valem), só depois na agenda.\n",
    "- 'Essas tarefas aí', 'as 3 cobrando', 'por que só segunda': é sobre a última mensagem do bot fora desta conversa, que vem entre colchetes na mensagem.\n",
    "- {Ele} chamar de compromisso/consulta algo que bate com uma tarefa pendente (#id na mensagem): é a tarefa. Use adiar_tarefa direto, sem procurar evento nem perguntar.\n",
    "- 'Só me cobra em horário útil', 'se eu não confirmar, cobra de novo só em horário útil': passe em janelas as de horário útil da memória (criar_tarefa/adiar_tarefa). Sem isso, janelas = [].\n",
    "- O bot já não avisa nem cobra durante evento da agenda (com horário; o Sono conta): segura e manda quando o evento acaba. 'Não me cobra quando eu estiver em evento/compromisso' → diga que já é assim; não mexa em janelas.\n",
    "- habito = true só se {ele} pedir pra acompanhar/registrar quando fizer ou chamar de hábito ('ração do gato a cada 2 dias desde a última vez, como hábito'); senão false.\n",
    "- Mudar o horário de uma tarefa que já existe ('me lembra às 18' sem dizer o quê, 'me cobra disso mais tarde', 'a cobrança de X às 18', adiar, remanejar): adiar_tarefa com o id das pendentes que vêm na mensagem (ou do listar_tarefas). Nunca crie tarefa nova pra isso.\n",
    "- 'Aumenta o intervalo', 'me cobra menos', 'cobra 1x por dia' de uma tarefa: adiar_tarefa com cobrar_a_cada_h (sem número: o dobro do atual; o padrão está no fim deste prompt e o próprio da tarefa vem nas pendentes). Pedido que junta horário e intervalo ('academia daqui 2 dias, aumenta o intervalo') é UMA chamada de adiar_tarefa com os dois.\n",
    "- Viagem, folga, 'pausa as tarefas até X', 'não me cobra nada no fim de semana': pausar_tarefas (ids [] = todas, ou só as que {ele} citar; 'ate' = quando {ele} volta). 'Pode voltar a cobrar' → ate null. Não apague nem adie uma por uma pra isso.\n",
    "\n",
    "MANUTENÇÃO: os registros dos veículos estão no ver_manutencao — use pra perguntas de atraso, última vez ou o que vence (nunca diga que não tem acesso); copie os números e o nome de cada item como vêm, não recalcule nem renomeie ('Nível do óleo' é conferir o nível, não trocar o óleo). O bot avisa às 6h o que venceu. Se {ele} contar que fez, registrar_manutencao com as chaves exatas da lista no fim (não bateu: pergunte).\n",
    "\n",
    "{prompt_extra}\n",
    "\n",
    "CHUVA: previsão hora a hora (Open-Meteo, 7 dias, {cidade}) pelo previsao_chuva; se {ele} der um corte ('mais que 50%'), passe em limiar. Copie horários e números, não recalcule.\n",
    "\n",
    "Se {ele} se referir a um evento pelo nome, ache na agenda com listar_eventos (próximos 60 dias) em vez de perguntar a data.\n",
    "\n",
    "MEMÓRIA (lembrar/corrigir_memoria/esquecer): quando {ele} contar algo duradouro — quem é quem, apelidos, lugares, preferências, rotinas, metas — salve com lembrar sem perguntar, uma frase autocontida por fato ('Zé = apelido do chefe {do_nome}'). Não salve o que já está na agenda, nem coisa passageira. Fato mudou: corrigir_memoria. {Ele} pediu pra esquecer: esquecer. Use a memória em tudo. Fato sobre um evento da agenda, ou se {ele} disser 'até lá': passe evento_ref com uma palavra do nome do evento (o bot acha a data e o fato some depois). Fato com prazo que não é evento: valido_ate. Fato sobre pessoas, lugares e preferências: permanente (os dois null).\n",
    "\n",
    "DESFAZER: se {ele} pedir pra desfazer, voltar atrás ou disser que a última ação estava errada, use desfazer_ultimo (uma vez por pedido). Se {ele} disser o que era o certo ('era pra adiar'), faça também a ação certa na mesma resposta.\n",
    "\n",
    "ABASTECIMENTO é por foto, outro fluxo: se {ele} falar disso, diga pra mandar as fotos.\n",
);

/// Minúsculo e sem acento, pra comparar 'Estágio' com 'estagio' (casefold + NFD sem as marcas).
pub fn fold(text: &str) -> String {
    use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
    let mut low = String::with_capacity(text.len());
    for c in text.chars() {
        // casefold do Python além do lowercase: as dobras de mais de uma letra que aparecem em texto latino
        match c {
            'ß' | 'ẞ' => low.push_str("ss"),
            'ς' => low.push('σ'),
            'ſ' => low.push('s'),
            'ﬀ' => low.push_str("ff"),
            'ﬁ' => low.push_str("fi"),
            'ﬂ' => low.push_str("fl"),
            'ﬃ' => low.push_str("ffi"),
            'ﬄ' => low.push_str("ffl"),
            'ﬅ' | 'ﬆ' => low.push_str("st"),
            c => low.extend(c.to_lowercase()),
        }
    }
    low.nfd().filter(|c| !is_combining_mark(*c)).collect()
}

pub fn when_to_google(value: &str) -> R<Value> {
    if so_data(value) {
        return Ok(json!({"date": value}));
    }
    let (naive, _) = parse_iso(value)?;
    Ok(json!({"dateTime": format!("{}:00", naive.format("%Y-%m-%dT%H:%M")), "timeZone": crate::util::fuso().name()}))
}

fn quando_de(e: &Value, k: &str) -> R<Value> {
    let s = e.get(k).ok_or_else(|| Erro::Chave(format!("'{k}'")))?;
    Ok(s.get("dateTime").or_else(|| s.get("date")).cloned().unwrap_or(Value::Null))
}

pub fn compact_event(e: &Value) -> R<Value> {
    let mut out = json!({"id": e.get("id").cloned().ok_or_else(|| Erro::Chave("'id'".into()))?,
                         "titulo": e.get("summary").cloned().unwrap_or(json!("(sem título)")),
                         "inicio": quando_de(e, "start")?,
                         "fim": quando_de(e, "end")?});
    for (src, dst) in [("location", "local"), ("description", "descricao")] {
        if let Some(v) = e.get(src).filter(|v| verdade(v)) {
            out[dst] = match v.as_str() {
                Some(s) => json!(corta(s, 200)),
                None => v.clone(),
            };
        }
    }
    if e.get("recurringEventId").map(verdade).unwrap_or(false) {
        out["recorrente"] = json!(true);
    }
    Ok(out)
}

fn texto_de(e: &Value, k: &str) -> String {
    e.get(k).map(crate::util::str_num).unwrap_or_default()
}

/// Linha pronta pra lista do chat: 'seg 05/10 · 10h30 Estágio (até 09/10)'.
pub fn agenda_line(e: &Value) -> R<String> {
    let start = crate::util::str_num(&quando_de(e, "start")?);
    let end = crate::util::str_num(&quando_de(e, "end")?);
    let title = e.get("summary").map(crate::util::str_num).unwrap_or_else(|| "(sem título)".into());
    if start.contains('T') {
        let (s_dt, _) = parse_iso(&start)?;
        let (e_dt, _) = parse_iso(&end)?;
        let mut line = format!("{}h{} {title}", s_dt.format("%H"), s_dt.format("%M")).replace("h00 ", "h ");
        if e_dt.date() > s_dt.date() {
            line += &format!(" (até {})", e_dt.format("%d/%m"));
        }
        let dia: String = dia_semana(&s_dt).chars().take(3).collect();
        return Ok(format!("{dia} {} · {line}", s_dt.format("%d/%m")));
    }
    let last = parse_iso(&end)?.0 - chrono::Duration::days(1);
    let span = if last.date() > parse_iso(&start)?.0.date() { format!(" (até {})", last.format("%d/%m")) } else { String::new() };
    let ch: Vec<char> = start.chars().collect();
    let fatia = |a: usize, b: usize| ch.get(a..b.min(ch.len())).map(|s| s.iter().collect::<String>()).unwrap_or_default();
    Ok(format!("{}/{} · dia inteiro: {title}{span}", fatia(8, 10), fatia(5, 7)))
}

pub fn event_line(e: &Value) -> R<String> {
    let c = compact_event(e)?;
    let titulo = crate::util::str_num(&c["titulo"]);
    let start = c["inicio"].as_str().unwrap_or_default().to_string();
    if start.contains('T') {
        let (dt, _) = parse_iso(&start)?;
        return Ok(format!("{titulo} — {} {}", dia_semana(&dt), dt.format("%d/%m %H:%M")));
    }
    let ch: Vec<char> = start.chars().collect();
    let fatia = |a: usize, b: usize| ch.get(a..b.min(ch.len())).map(|s| s.iter().collect::<String>()).unwrap_or_default();
    Ok(format!("{titulo} — {}/{} (dia inteiro)", fatia(8, 10), fatia(5, 7)))
}

/// '08:00-11:59' → (08:00, 11:59); fim tem que ser depois do início.
pub fn parse_janela(s: &str) -> R<(NaiveTime, NaiveTime)> {
    let erro = || Erro::Valor(format!("janela inválida: '{s}' (use 'HH:MM-HH:MM')"));
    let (a, b) = s.split_once('-').ok_or_else(erro)?;
    let t = |x: &str| NaiveTime::parse_from_str(x.trim(), "%H:%M").map_err(|_| erro());
    let (a, b) = (t(a)?, t(b)?);
    if b <= a {
        return Err(erro());
    }
    Ok((a, b))
}

/// início/fim de um evento com horário, no relógio da instância; dia inteiro → None
fn hora_local(e: &Value, k: &str) -> Option<NaiveDateTime> {
    let s = e.get(k)?.get("dateTime")?.as_str()?;
    Some(DateTime::parse_from_rfc3339(s).ok()?.with_timezone(&crate::util::fuso()).naive_local())
}

/// (início, fim) dos eventos que ocupam, em ordem: com horário, de até 24 h e não transparentes
fn ocupados(eventos: &[Value]) -> Vec<(NaiveDateTime, NaiveDateTime)> {
    let mut v: Vec<(NaiveDateTime, NaiveDateTime)> = eventos
        .iter()
        .filter(|e| e.get("transparency").and_then(Value::as_str) != Some("transparent"))
        .filter_map(|e| Some((hora_local(e, "start")?, hora_local(e, "end")?)))
        .filter(|(s, e)| *e - *s <= chrono::Duration::hours(24)) // 'Estágio' de seg a sex marca o período, não ocupa
        .collect();
    v.sort();
    v
}

/// Fim do compromisso em que `agora` está (mesmo critério do `horarios_livres`); eventos colados contam como um só.
pub fn ocupado_ate(eventos: &[Value], agora: NaiveDateTime) -> Option<NaiveDateTime> {
    let mut fim: Option<NaiveDateTime> = None;
    for (s, e) in ocupados(eventos) {
        let t = fim.unwrap_or(agora);
        if s > t {
            break;
        }
        if e > t {
            fim = Some(e);
        }
    }
    fim
}

/// Buracos de pelo menos `duracao_min` dentro das `janelas` de cada dia, de `de` até o fim do dia `de + dias - 1`.
/// Bloqueiam só eventos com horário, de até 24 h e não transparentes (Sono bloqueia; dia inteiro e vários dias não).
pub fn horarios_livres(eventos: &[Value], janelas: &[(NaiveTime, NaiveTime)], de: NaiveDateTime, dias: i64, duracao_min: i64)
    -> Vec<(NaiveDateTime, NaiveDateTime)> {
    let ocupado = ocupados(eventos);
    let mut janelas = janelas.to_vec();
    janelas.sort();
    let minimo = chrono::Duration::minutes(duracao_min.max(1));
    let mut out = vec![];
    for d in 0..dias.max(1) {
        let dia = de.date() + chrono::Duration::days(d);
        for (a, b) in &janelas {
            let (mut ini, fim) = (dia.and_time(*a).max(de), dia.and_time(*b));
            for (s, e) in &ocupado {
                if *e <= ini || *s >= fim {
                    continue;
                }
                if *s - ini >= minimo {
                    out.push((ini, *s));
                }
                ini = ini.max(*e);
            }
            if fim - ini >= minimo {
                out.push((ini, fim));
            }
        }
    }
    out
}

/// 'qua 07/10 · 14h–17h49'
pub fn linha_livre(a: &NaiveDateTime, b: &NaiveDateTime) -> String {
    let hm = |t: &NaiveDateTime| {
        let s = t.format("%Hh%M").to_string();
        s.strip_suffix("00").map(str::to_string).unwrap_or(s)
    };
    let dia: String = dia_semana(a).chars().take(3).collect();
    let fim = if b.date() > a.date() { format!("{} {}", b.format("%d/%m"), hm(b)) } else { hm(b) };
    format!("{dia} {} · {}–{fim}", a.format("%d/%m"), hm(a))
}

/// uma linha por dia com os buracos dele ("qui 15/10 · 06h30–08h, 12h–13h"), até `max_dias` dias
pub fn linhas_livres(livres: &[(NaiveDateTime, NaiveDateTime)], max_dias: usize) -> Vec<String> {
    let mut out: Vec<(String, Vec<String>)> = vec![];
    for (a, b) in livres {
        let l = linha_livre(a, b);
        let (dia, faixa) = l.split_once(" · ").unwrap_or((&l, ""));
        if let Some((_, fs)) = out.last_mut().filter(|(d, _)| d == dia) {
            fs.push(faixa.to_string());
        } else if out.len() == max_dias {
            break;
        } else {
            out.push((dia.to_string(), vec![faixa.to_string()]));
        }
    }
    out.into_iter().map(|(d, fs)| format!("{d} · {}", fs.join(", "))).collect()
}

/// ' (cobra só 08:00-11:59, 14:00-17:49)' no recibo, se a tarefa tem janelas próprias
fn nota_janelas(t: &crate::tarefas::Tarefa) -> String {
    match t.resto.get("janelas").and_then(Value::as_array) {
        Some(a) if !a.is_empty() => format!(" (cobra só {})", a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")),
        _ => String::new(),
    }
}

/// " · cobra a cada 6h" quando a tarefa tem intervalo próprio
fn nota_intervalo(t: &crate::tarefas::Tarefa) -> String {
    t.resto.get("cobrar_a_cada_h").and_then(Value::as_f64).map(|h| format!(" · cobra a cada {}h", crate::util::fmt_g(h))).unwrap_or_default()
}

/// `cobrar_a_cada_h` das ferramentas de tarefa; ausente ou null = None
fn arg_intervalo(args: &Value) -> R<Option<f64>> {
    match args.get("cobrar_a_cada_h").filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(v) => v.as_f64().map(Some).ok_or_else(|| Erro::Valor("cobrar_a_cada_h tem que ser número".into())),
    }
}

/// `janelas` das ferramentas de tarefa; ausente (Python antigo, mock) = []
fn arg_janelas(args: &Value) -> R<Vec<String>> {
    let Some(v) = args.get("janelas").filter(|v| !v.is_null()) else { return Ok(vec![]) };
    let a = v.as_array().ok_or_else(|| Erro::Valor("janelas tem que ser lista".into()))?;
    Ok(a.iter().filter_map(|j| j.as_str().map(str::to_string)).collect())
}

/// Conversa com a agenda. Histórico append-only; zera depois de um tempo parado.
#[derive(Default)]
pub struct Agenda {
    /// mensagens como JSON cru: o content do assistant volta exatamente como veio (thinking incluído)
    pub messages: Vec<Value>,
    pub last_at: Option<Instant>,
    pub pending_delete: Option<Value>,
    /// concluir tarefa que não está cobrando: só com o botão (o Haiku concluiu "Regar as plantas" do nada em 07/10)
    pub pending_done: Vec<(i64, String)>,
    /// manutenção com km estimada esperando o botão (`mnc:yes`/`mnc:no`): {veiculo, itens, custo, data, odometro, rotulo}
    pub pending_maint: Option<Value>,
    /// o que aconteceu de verdade, escrito pelo código (não pelo modelo)
    pub actions: Vec<String>,
    /// como reverter cada ação desta mensagem (vira uma entrada de desfazer)
    pub undo_ops: Vec<Value>,
    /// "#id texto" da tarefa do 🕒 quando a resposta ao "pra quando?" não era horário: vai só na mensagem seguinte
    pub foco_hora: Option<String>,
    /// textos de criar_tarefa já barrados pela `tarefa_parecida` neste pedido (a 2ª tentativa igual passa)
    pub avisou_parecida: Vec<String>,
    /// última cobrança/resposta de botão de tarefa (não passa pelo modelo): vai só na mensagem seguinte,
    /// senão "não me cobra dessas 3 aí" e "oxi, pq só segunda" ficavam sem contexto (17/10)
    pub aviso: Option<(crate::util::Dt, String)>,
}

impl Bot {
    /// texto extra no fim do system prompt
    pub fn system_extra(&self) -> R<String> {
        Ok(self.maintenance_catalog()? + &format!("\n\nCobrança padrão das tarefas: a cada {}h.", crate::util::fmt_g(self.config.cobrar_a_cada_h)) + &memory_block(&self.state, &personaliza("{dele}", &self.config.usuario)))
    }

    /// Último dia do próximo evento (90 dias) cujo nome contém `ref`, sem acento.
    pub fn event_end_date(&mut self, r: &str) -> R<Option<String>> {
        let now = crate::util::agora();
        let needle = fold(r);
        for e in self.google()?.list_events(&(now - chrono::Duration::days(1)), &(now + chrono::Duration::days(90)), None)? {
            if fold(&texto_de(&e, "summary")).contains(&needle) {
                let fim = e.get("end").ok_or_else(|| Erro::Chave("'end'".into()))?;
                let dt = fim.get("dateTime").filter(|v| verdade(v));
                let end = dt.or_else(|| fim.get("date")).map(crate::util::str_num).unwrap_or_else(|| "None".into());
                let mut last = parse_iso(&end)?.0;
                if fim.get("dateTime").is_none() {
                    last -= chrono::Duration::days(1); // fim de evento de dia inteiro é exclusivo
                }
                return Ok(Some(last.format("%Y-%m-%d").to_string()));
            }
        }
        Ok(None)
    }

    /// eventos da própria agenda ou, com `de` = nome do parceiro, da agenda dele (só leitura)
    fn eventos_de(&mut self, args: &Value, start: &crate::util::Dt, end: &crate::util::Dt) -> R<Vec<Value>> {
        // sem parceiro o schema não tem `de`
        let de = args.get("de").and_then(Value::as_str).unwrap_or_default();
        let Some(p) = self.config.agenda_parceiro.clone().filter(|p| fold(de) == fold(&p.nome)) else {
            return self.google()?.list_events(start, end, None);
        };
        self.google()?.list_events_em(&p.calendar_id, start, end, None).map_err(|e| match e {
            Erro::Http { code: 403 | 404, .. } => Erro::Valor(format!(
                "a agenda de {} não está compartilhada com esta conta (no Google Agenda dela: Configurações → Compartilhar com pessoas específicas)", p.nome)),
            e => e,
        })
    }

    /// parceiro com a pasta da instância dele (sem `dir` as ferramentas nem existem)
    fn parceiro_com_dir(&self) -> R<(Parceiro, std::path::PathBuf)> {
        let p = self.config.agenda_parceiro.clone().ok_or_else(|| Erro::Valor("sem parceiro configurado".into()))?;
        let dir = p.dir.as_deref().map(|d| std::path::PathBuf::from(crate::bot::expanduser(d)))
            .ok_or_else(|| Erro::Valor(format!("sem a pasta do bot de {}", p.nome)))?;
        Ok((p, dir))
    }

    /// eventos pro horarios_livres: de='juntos' soma as duas agendas (buraco livre dos dois)
    fn eventos_livres(&mut self, args: &Value, start: &crate::util::Dt, end: &crate::util::Dt) -> R<Vec<Value>> {
        if args.get("de").and_then(Value::as_str) != Some("juntos") {
            return self.eventos_de(args, start, end);
        }
        let Some(p) = self.config.agenda_parceiro.clone() else { return self.eventos_de(&json!({}), start, end) };
        let mut ev = self.eventos_de(&json!({}), start, end)?;
        ev.extend(self.eventos_de(&json!({"de": p.nome}), start, end)?);
        Ok(ev)
    }

    pub fn run_tool(&mut self, name: &str, args: &Value) -> R<String> {
        match name {
            "listar_eventos" => {
                let fim = arg_str(args, "fim")?;
                let (start, mut end) = (when_to_aware(&arg_str(args, "inicio")?)?, when_to_aware(&fim)?);
                if so_data(&fim) {
                    end = end + chrono::Duration::days(1); // 'fim' só com data = até o fim desse dia
                }
                let mut events = self.eventos_de(args, &start, &end)?;
                // busca local e sem acento (não o `q` do Google)
                if let Some(busca) = arg_opt_str(args, "busca")?.filter(|b| !b.is_empty()) {
                    let needle = fold(&busca);
                    events.retain(|e| {
                        let campos = ["summary", "location", "description"].map(|k| texto_de(e, k)).join(" ");
                        fold(&campos).contains(&needle)
                    });
                }
                let mut lines = vec![];
                for e in events.iter().filter(|e| fold(&texto_de(e, "summary")) != "sono") {
                    lines.push(agenda_line(e)?);
                }
                let c: R<Vec<Value>> = events.iter().map(compact_event).collect();
                Ok(dumps(&json!({"linhas_prontas": lines, "eventos": c?})))
            }
            "horarios_livres" => {
                let janelas: R<Vec<_>> = arg(args, "janelas")?.as_array().ok_or_else(|| Erro::Chave("janelas".into()))?
                    .iter().map(|j| parse_janela(j.as_str().unwrap_or_default())).collect();
                let janelas = janelas?;
                let de = when_to_aware(&arg_str(args, "inicio")?)?;
                let dias = arg(args, "dias")?.as_i64().unwrap_or(7).clamp(1, 14);
                let duracao = arg(args, "duracao_min")?.as_i64().unwrap_or(30);
                let dia0 = de.date_naive().and_hms_opt(0, 0, 0).unwrap_or(de.naive_local());
                let ate = |n: i64| de + (dia0 + chrono::Duration::days(n) - de.naive_local());
                let mut livres = horarios_livres(&self.eventos_livres(args, &de, &ate(dias))?, &janelas, de.naive_local(), dias, duracao);
                if livres.is_empty() && dias < 14 {
                    // nada no pedido: olha 14 dias em vez de deixar o modelo chutar
                    livres = horarios_livres(&self.eventos_livres(args, &de, &ate(14))?, &janelas, de.naive_local(), 14, duracao);
                }
                println!("agenda: horarios_livres {} → {} buraco(s)", dumps(args), livres.len());
                if livres.is_empty() {
                    return Ok(dumps(&json!({"linhas_prontas": [], "aviso": format!("nenhum horário livre nessas janelas até {}", ate(14).format("%d/%m"))})));
                }
                let fmt = |t: &NaiveDateTime| t.format("%Y-%m-%dT%H:%M").to_string();
                let linhas = linhas_livres(&livres, 7);
                let lista: Vec<Value> = livres.iter().take(10).map(|(a, b)| json!({"inicio": fmt(a), "fim": fmt(b)})).collect();
                Ok(dumps(&json!({"linhas_prontas": linhas, "livres": lista})))
            }
            "criar_evento" => {
                let mut body = json!({"summary": arg(args, "titulo")?, "start": when_to_google(&arg_str(args, "inicio")?)?,
                                      "end": when_to_google(&arg_str(args, "fim")?)?});
                if verdade(arg(args, "local")?) {
                    body["location"] = args["local"].clone();
                }
                if verdade(arg(args, "descricao")?) {
                    body["description"] = args["descricao"].clone();
                }
                let lem = arg(args, "lembrete_min")?;
                if !lem.is_null() {
                    body["reminders"] = json!({"useDefault": false, "overrides": [{"method": "popup", "minutes": lem}]});
                }
                let e = self.google()?.create(&body)?;
                let linha = event_line(&e)?;
                self.agenda.actions.push(format!("📅 criado: {linha}"));
                self.agenda.undo_ops.push(json!({"op": "evento_criado", "id": e.get("id").cloned().ok_or_else(|| Erro::Chave("'id'".into()))?}));
                println!("agenda: criado {linha}");
                Ok(dumps(&json!({"ok": true, "evento": compact_event(&e)?})))
            }
            "editar_evento" => {
                let mut body = Map::new();
                if verdade(arg(args, "titulo")?) {
                    body.insert("summary".into(), args["titulo"].clone());
                }
                if verdade(arg(args, "inicio")?) {
                    body.insert("start".into(), when_to_google(&arg_str(args, "inicio")?)?);
                }
                if verdade(arg(args, "fim")?) {
                    body.insert("end".into(), when_to_google(&arg_str(args, "fim")?)?);
                }
                if !arg(args, "local")?.is_null() {
                    body.insert("location".into(), args["local"].clone());
                }
                if !arg(args, "descricao")?.is_null() {
                    body.insert("description".into(), args["descricao"].clone());
                }
                let id = arg_str(args, "id")?;
                let old = self.google()?.get(&id)?;
                let e = self.google()?.patch(&id, &Value::Object(body))?;
                let linha = event_line(&e)?;
                self.agenda.actions.push(format!("📅 alterado: {linha}"));
                let mut antes = serde_json::Map::new();
                for k in ["summary", "location", "description"] {
                    antes.insert(k.into(), old.get(k).cloned().unwrap_or(json!("")));
                }
                for k in ["start", "end"] {
                    antes.insert(k.into(), old.get(k).cloned().ok_or_else(|| Erro::Chave(format!("'{k}'")))?);
                }
                self.agenda.undo_ops.push(json!({"op": "evento_alterado", "id": id, "antes": antes}));
                println!("agenda: editado {linha}");
                Ok(dumps(&json!({"ok": true, "evento": compact_event(&e)?})))
            }
            "criar_tarefa" => {
                let texto = arg_str(args, "texto")?;
                if let Some(p) = crate::tarefas::tarefa_parecida(&self.state.tarefas, &texto) {
                    // primeira vez neste pedido: volta pro modelo; se ele insistir com o mesmo texto, cria
                    if !self.agenda.avisou_parecida.contains(&texto) {
                        self.agenda.avisou_parecida.push(texto.clone());
                        return Err(Erro::Valor(format!(
                            "não criei: já existe a tarefa #{} '{}'{}. Se o pedido é sobre ela (horário, intervalo, janelas), use adiar_tarefa com id {}. \
Só se for mesmo outra tarefa, chame criar_tarefa de novo com o mesmo texto.",
                            p.id, p.texto, if p.pendente { " (pendente, cobrando)" } else { "" }, p.id)));
                    }
                }
                let rep = arg(args, "repetir_dias")?;
                let rep = if rep.is_null() { None } else { Some(rep.as_f64().ok_or_else(|| Erro::Chave("repetir_dias".into()))? as i64) };
                let from_done = verdade(arg(args, "contar_desde_feito")?);
                let hora = self.config.hora_padrao_tarefa.clone();
                let mut t = new_task(&mut self.state, &texto, &arg_str(args, "quando")?, rep, from_done, &hora)?;
                let janelas = arg_janelas(args)?;
                let habito = verdade(args.get("habito").unwrap_or(&Value::Null));
                let intervalo = arg_intervalo(args)?;
                // só existe com parceiro (dir)
                let privada = args.get("privado").and_then(Value::as_bool) == Some(true);
                if !janelas.is_empty() || habito || intervalo.is_some() || privada {
                    let pos = self.state.tarefas.iter().position(|x| x.id == t.id).ok_or_else(|| Erro::Valor("tarefa sumiu".into()))?;
                    crate::tarefas::set_janelas(&mut self.state.tarefas[pos], &janelas)?;
                    crate::tarefas::set_intervalo(&mut self.state.tarefas[pos], intervalo)?;
                    if habito {
                        self.state.tarefas[pos].resto.insert("habito".into(), Value::Bool(true));
                    }
                    crate::tarefas::set_privado(&mut self.state.tarefas[pos].resto, privada);
                    t = self.state.tarefas[pos].clone();
                }
                save_state(&self.state, &self.state_path)?;
                self.agenda.undo_ops.push(json!({"op": "tarefa_criada", "id": t.id}));
                self.agenda.actions.push(format!(
                    "✅ tarefa criada: {} — {}{}{}",
                    t.texto,
                    fmt_when(&t.proximo)?,
                    t.repetir_dias.map(|r| format!(" · a cada {r}d")).unwrap_or_default(),
                    nota_janelas(&t)
                ) + if habito { " · 📊 hábito (vai pro tracker)" } else { "" } + if privada { " 🔒" } else { "" });
                println!("tarefa: criada {}", serde_json::to_string(&t).unwrap_or_default());
                Ok(dumps(&json!({"ok": true, "tarefa": describe_task(&t)?})))
            }
            "listar_tarefas" => {
                let l: R<Vec<Value>> = self.state.tarefas.iter().map(describe_task).collect();
                Ok(dumps(&l?))
            }
            "concluir_tarefa" | "apagar_tarefa" => {
                let id = arg(args, "id")?;
                let pos = self.state.tarefas.iter().position(|t| id.as_f64() == Some(t.id as f64));
                let Some(pos) = pos else { return Err(Erro::Valor(format!("tarefa {} não existe", crate::util::str_num(id)))) };
                let texto = self.state.tarefas[pos].texto.clone();
                // hábito ("dei ração") quase sempre é feito antes de cobrar: conclui direto, o ↩️ desfaz
                if name == "concluir_tarefa" && !self.state.tarefas[pos].pendente && !crate::tarefas::e_habito(&self.state.tarefas[pos]) {
                    let t = &self.state.tarefas[pos];
                    let vence = fmt_when(&t.proximo)?;
                    if !self.agenda.pending_done.iter().any(|(i, _)| *i == t.id) {
                        self.agenda.pending_done.push((t.id, format!("{texto} (vence {vence})")));
                    }
                    return Ok(format!("'{texto}' ainda não está cobrando (vence {vence}): botão de confirmação mostrado ao usuário. Só conclui se ele tocar em concluir; não diga que concluiu."));
                }
                let msg = if name == "apagar_tarefa" {
                    self.agenda.undo_ops.push(crate::desfazer::op_tarefa_antes(&self.state.tarefas[pos]));
                    self.state.tarefas.remove(pos);
                    "apagada".to_string()
                } else {
                    let tid = self.state.tarefas[pos].id;
                    let (msg, ops) = self.conclui_tarefa(tid, &crate::util::agora())?;
                    self.agenda.undo_ops.extend(ops);
                    msg
                };
                save_state(&self.state, &self.state_path)?;
                let extra = match msg.rsplit_once("— ") {
                    Some((_, depois)) => format!(" ({depois})"),
                    None => String::new(),
                };
                self.agenda.actions.push(format!("✅ tarefa {}: {texto}{extra}", if name == "apagar_tarefa" { "apagada" } else { "concluída" }));
                Ok(dumps(&json!({"ok": true, "tarefa": texto, "resultado": msg})))
            }
            "adiar_tarefa" => {
                let id = arg(args, "id")?;
                let Some(pos) = self.state.tarefas.iter().position(|t| id.as_f64() == Some(t.id as f64)) else {
                    return Err(Erro::Valor(format!("tarefa {} não existe", crate::util::str_num(id))));
                };
                // não-strict: ausente ou null = não mexe no horário
                let quando = args.get("quando").and_then(Value::as_str).filter(|q| !q.is_empty()).map(String::from);
                let alvo = match quando {
                    Some(q) if so_data(&q) => Some(when_to_aware(&format!("{q}T{}", self.state.tarefas[pos].hora))?),
                    Some(q) => Some(when_to_aware(&q)?),
                    None => None,
                };
                let janelas = arg_janelas(args)?;
                let intervalo = arg_intervalo(args)?;
                let privada = args.get("privado").and_then(Value::as_bool);
                if alvo.is_none() && janelas.is_empty() && intervalo.is_none() && privada.is_none() {
                    return Err(Erro::Valor("nada pra mudar: passe quando, janelas, cobrar_a_cada_h ou privado".into()));
                }
                self.agenda.undo_ops.push(crate::desfazer::op_tarefa_antes(&self.state.tarefas[pos]));
                crate::tarefas::set_janelas(&mut self.state.tarefas[pos], &janelas)?;
                crate::tarefas::set_intervalo(&mut self.state.tarefas[pos], intervalo)?;
                if let Some(sim) = privada {
                    crate::tarefas::set_privado(&mut self.state.tarefas[pos].resto, sim);
                }
                let cfg = self.config.cobranca();
                let t = &mut self.state.tarefas[pos];
                let novo = match &alvo {
                    Some(a) => Some(task_move(t, a, &cfg)?),
                    None => None,
                };
                let texto = t.texto.clone();
                let nota = nota_janelas(t) + &nota_intervalo(t);
                save_state(&self.state, &self.state_path)?;
                let acao = match &novo {
                    Some(n) => format!("⏰ tarefa adiada: {texto} → {}{nota}", fmt_when(n)?),
                    None if privada.is_some() && janelas.is_empty() && intervalo.is_none() =>
                        format!("{} tarefa {}: {texto}", if privada == Some(true) { "🔒" } else { "🔓" }, if privada == Some(true) { "privada" } else { "visível" }),
                    None => format!("⏰ tarefa ajustada: {texto}{nota}"),
                };
                self.agenda.actions.push(acao);
                let mut out = json!({"ok": true, "tarefa": texto});
                if let Some(n) = &novo {
                    out["quando"] = json!(fmt_when(n)?);
                }
                out["cobra_a_cada_h"] = json!(crate::tarefas::intervalo_h(&self.state.tarefas[pos], cfg.cobrar_a_cada_h));
                Ok(dumps(&out))
            }
            "pausar_tarefas" => {
                let now = crate::util::agora();
                let ate = match arg_opt_str(args, "ate")? {
                    Some(a) if so_data(&a) => Some(when_to_aware(&format!("{a}T00:00"))?),
                    Some(a) => Some(when_to_aware(&a)?),
                    None => None,
                };
                if ate.is_some_and(|a| a <= now) {
                    return Err(Erro::Valor("'ate' já passou".into()));
                }
                let ids: Vec<i64> = arg(args, "ids")?.as_array().ok_or_else(|| Erro::Valor("ids tem que ser lista".into()))?
                    .iter().filter_map(Value::as_f64).map(|f| f as i64).collect();
                if let Some(x) = ids.iter().find(|i| !self.state.tarefas.iter().any(|t| t.id == **i)) {
                    return Err(Erro::Valor(format!("tarefa {x} não existe")));
                }
                let mut nomes = vec![];
                for t in self.state.tarefas.iter_mut().filter(|t| ids.is_empty() || ids.contains(&t.id)) {
                    if ate.is_none() && crate::tarefas::pausa_ate(t).is_none() {
                        continue;
                    }
                    self.agenda.undo_ops.push(crate::desfazer::op_tarefa_antes(t));
                    crate::tarefas::set_pausa(t, ate.as_ref(), &now);
                    nomes.push(t.texto.clone());
                }
                save_state(&self.state, &self.state_path)?;
                let lista = if ids.is_empty() && ate.is_some() { "todas as tarefas".to_string() } else { nomes.join(", ") };
                self.agenda.actions.push(match &ate {
                    Some(a) => format!("⏸ pausadas até {}: {lista}", fmt_when(&crate::util::iso(a))?),
                    None if nomes.is_empty() => "▶️ nenhuma tarefa estava pausada".into(),
                    None => format!("▶️ pausa tirada: {lista}"),
                });
                Ok(dumps(&json!({"ok": true, "tarefas": nomes, "ate": ate.map(|a| fmt_when(&crate::util::iso(&a))).transpose()?})))
            }
            "lembrar" => {
                let mut until = arg_opt_str(args, "valido_ate")?;
                if let Some(r) = args.get("evento_ref").filter(|v| verdade(v)) {
                    if let Some(fim) = self.event_end_date(&crate::util::str_num(r))? {
                        until = Some(fim);
                    }
                }
                let mut f = remember(&mut self.state, &arg_str(args, "texto")?, &crate::util::agora(), until);
                // só existe com parceiro (dir)
                if args.get("privado").and_then(Value::as_bool) == Some(true) {
                    if let Some(x) = self.state.memoria.iter_mut().find(|x| x.id == f.id) {
                        crate::tarefas::set_privado(&mut x.resto, true);
                        f = x.clone();
                    }
                }
                save_state(&self.state, &self.state_path)?;
                self.agenda.undo_ops.push(json!({"op": "memoria_criada", "id": f.id}));
                self.agenda.actions.push(format!("🧠 lembrei: {}{}{}", f.texto, validity_note(&f), if crate::tarefas::privado(&f.resto) { " 🔒" } else { "" }));
                Ok(dumps(&json!({"ok": true, "id": f.id})))
            }
            "corrigir_memoria" | "esquecer" => {
                let id = arg(args, "id")?;
                let Some(pos) = self.state.memoria.iter().position(|f| id.as_f64() == Some(f.id as f64)) else {
                    return Err(Erro::Valor(format!("fato {} não existe na memória", crate::util::str_num(id))));
                };
                self.agenda.undo_ops.push(crate::desfazer::op_memoria_antes(&self.state.memoria[pos]));
                if name == "esquecer" {
                    let f = self.state.memoria.remove(pos);
                    self.agenda.actions.push(format!("🧠 esqueci: {}", f.texto));
                } else {
                    let (texto, ate) = (arg_str(args, "texto")?, arg_opt_str(args, "valido_ate")?);
                    let f = &mut self.state.memoria[pos];
                    f.texto = texto;
                    f.revisado = crate::util::iso(&crate::util::agora());
                    f.ate = ate;
                    if let Some(sim) = args.get("privado").and_then(Value::as_bool) {
                        crate::tarefas::set_privado(&mut f.resto, sim);
                    }
                    let recibo = format!("🧠 corrigi: {}{}{}", f.texto, validity_note(f), if crate::tarefas::privado(&f.resto) { " 🔒" } else { "" });
                    self.agenda.actions.push(recibo);
                }
                save_state(&self.state, &self.state_path)?;
                Ok(dumps(&json!({"ok": true})))
            }
            "ver_manutencao" => self.maintenance_overview(),
            "consultar_parceiro" => {
                let (p, dir) = self.parceiro_com_dir()?;
                let estado = crate::tarefas::load_state(&dir.join("dados").join("estado.json"))?;
                Ok(dumps(&visivel_ao_parceiro(&estado, &p.nome)?))
            }
            "mandar_recado" => {
                let (p, dir) = self.parceiro_com_dir()?;
                let now = crate::util::agora();
                let quando = match arg_opt_str(args, "quando")?.filter(|q| !q.is_empty()) {
                    Some(q) => Some(when_to_aware(&q)?).filter(|q| *q > now),
                    None => None,
                };
                let r = crate::recados::Recado {
                    de: self.config.usuario.nome.clone(),
                    de_ela: self.config.usuario.ela,
                    texto: arg_str(args, "texto")?,
                    quando: quando.as_ref().map(crate::util::iso),
                    tarefa: verdade(arg(args, "tarefa")?),
                    em: crate::util::iso(&now),
                };
                crate::recados::enviar(&dir, &r)?;
                let q = match &r.quando {
                    Some(q) => format!(" — {}", fmt_when(q)?),
                    None => String::new(),
                };
                self.agenda.actions.push(format!("📨 {} pra {}: {}{q}", if r.tarefa { "tarefa" } else { "recado" }, p.nome, r.texto));
                Ok(dumps(&json!({"ok": true, "entregue": if r.quando.is_some() { "no horário pedido" } else { "agora" }})))
            }
            "previsao_chuva" => {
                let (Some(lat), Some(lon)) = (self.config.clima_lat, self.config.clima_lon) else {
                    return Ok(dumps(&json!({"erro": "previsão desligada: falta clima_lat/clima_lon no configs.json"})));
                };
                let dia = chrono::NaiveDate::parse_from_str(&arg_str(args, "dia")?, "%Y-%m-%d").map_err(|_| Erro::Valor("dia: use AAAA-MM-DD".into()))?;
                let limiar = match args.get("limiar") {
                    Some(v) => v.as_f64().ok_or_else(|| Erro::Chave(format!("limiar: esperava número, veio {v}")))?,
                    None => self.config.chuva_limiar,
                };
                let now = crate::util::agora();
                let desde = if dia == now.date_naive() { chrono::Timelike::hour(&now) } else { 0 };
                let horas = self.clima.previsao(lat, lon)?;
                Ok(dumps(&crate::clima::tabela_chuva(&horas, dia, desde, limiar)))
            }
            "registrar_manutencao" => {
                let veiculo = arg_str(args, "veiculo")?;
                let itens: Vec<String> = arg(args, "itens")?
                    .as_array()
                    .ok_or_else(|| Erro::Chave("itens".into()))?
                    .iter()
                    .map(crate::util::str_num)
                    .collect();
                let num_opcional = |k: &str| -> R<Option<f64>> {
                    match args.get(k) {
                        Some(v) if verdade(v) => Ok(Some(crate::util::num(v)?)),
                        _ => Ok(None),
                    }
                };
                let custo = num_opcional("custo_total")?.unwrap_or(0.0);
                let hoje = crate::util::agora().date_naive();
                let dia = match args.get("data").and_then(Value::as_str) {
                    Some(s) => chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| Erro::Valor("data: use AAAA-MM-DD".into()))?,
                    None => hoje,
                };
                if dia > hoje {
                    return Err(Erro::Valor("data no futuro: só registre manutenção já feita".into()));
                }
                let data = dia.format("%d/%m/%y").to_string();
                if let Some(odo) = num_opcional("odometro")? {
                    let out = self.register_maintenance(&veiculo, &itens, custo, &data, Some(odo))?;
                    self.agenda.undo_ops.push(self.last_op.clone().unwrap_or(Value::Null));
                    self.agenda.actions.push(format!("🔧 manutenção registrada: {}", itens.join(", ")));
                    return Ok(out);
                }
                // km estimada: não grava sem o botão (a estimativa pode errar)
                let csv = self.maintenance_csv(&veiculo, &itens)?;
                let (odo, _) = crate::tarefas::maintenance_odometer(&self.mr, &csv, dia)?;
                let veic_json = self.mr.load_json(self.mr.load_config()?.json_de(&veiculo)?)?;
                let rotulo = itens.iter().map(|k| crate::motoregis::display_name(k, veic_json.as_ref())).collect::<Vec<_>>().join(", ");
                self.agenda.pending_maint = Some(json!({"veiculo": veiculo, "itens": itens, "custo": custo, "data": data,
                                                         "odometro": odo, "rotulo": rotulo}));
                Ok(format!("Botão de confirmação mostrado ao usuário: {rotulo} em {data} com ~{} km (estimada pela média km/dia). \
Só grava se ele tocar em Gravar; se ele disser a km certa, chame de novo com odometro.", crate::util::fmt_g(odo)))
            }
            "desfazer_ultimo" => {
                let result = self.undo(None)?;
                self.agenda.actions.push(result.clone());
                Ok(dumps(&json!({"resultado": result})))
            }
            "apagar_evento" => {
                let e = self.google()?.get(&arg_str(args, "id")?)?;
                let linha = event_line(&e)?;
                self.agenda.pending_delete = Some(e);
                Ok(format!("Botão de confirmação mostrado ao usuário para: {linha}. O evento só será apagado se ele tocar em Apagar."))
            }
            _ => Err(Erro::Valor(format!("ferramenta desconhecida: {name}"))),
        }
    }

    /// zera a conversa se ficou parada, e devolve a mensagem do usuário com a hora carimbada
    fn agenda_turn(&mut self, text: &str) -> Value {
        let parado = self.agenda.last_at.map(|t| t.elapsed().as_secs_f64() > self.config.agenda_reset_min * 60.0).unwrap_or(true);
        if parado || self.agenda.messages.len() > 40 {
            self.agenda.messages.clear();
        }
        self.agenda.last_at = Some(Instant::now());
        let now = crate::util::agora();
        // pendentes vão na mensagem (não no system, que fica em cache): "me lembra às 18" já sabe de qual se trata
        let pendentes: Vec<String> = self.state.tarefas.iter().filter(|t| t.pendente).map(|t| format!("#{} {}{}{}", t.id, t.texto, nota_intervalo(t), crate::tarefas::nota_pausa(t, &now))).collect();
        let mut pendentes = if pendentes.is_empty() { String::new() } else { format!("\n[tarefas pendentes (cobrando): {}]", pendentes.join("; ")) };
        if let Some(f) = self.agenda.foco_hora.take() {
            pendentes += &format!("\n[respondendo ao \"pra quando?\" do 🕒 da tarefa {f}]");
        }
        if let Some((em, txt)) = self.agenda.aviso.take().filter(|(em, _)| (now - *em).num_hours() < 12) {
            pendentes += &format!("\n[última mensagem do bot fora desta conversa, {}: {}]", em.format("%d/%m %H:%M"), txt);
        }
        json!({"role": "user", "content": [{"type": "text", "text": format!("{}{pendentes}\n{text}", carimbo(&now))}]})
    }

    /// resposta dada por atalho (sem modelo) vira um par user/assistant no histórico
    pub fn agenda_note(&mut self, text: &str, reply: &str) {
        let user = self.agenda_turn(text);
        self.agenda.messages.push(user);
        self.agenda.messages.push(json!({"role": "assistant", "content": [{"type": "text", "text": format!("[respondido pelo bot] {reply}")}]}));
    }

    pub fn agenda_ask(&mut self, text: &str) -> R<String> {
        let user = self.agenda_turn(text);
        self.agenda.messages.push(user);
        self.agenda.actions.clear();
        self.agenda.undo_ops.clear();
        self.agenda.pending_done.clear();
        self.agenda.avisou_parecida.clear();
        // botão de mensagem anterior não reaparece: "foi com 3800 km" gravava direto e o "Gravar ~X km?" velho voltava
        self.agenda.pending_maint = None;
        self.agenda.pending_delete = None;

        let (base, beta) = model_params(&self.config.modelo_agenda, &self.config.esforco_agenda);
        let system = format!("{}{}", agenda_system(&self.config), self.system_extra()?);
        let tools = agenda_tools(&self.config);
        for _ in 0..10 {
            let mut body = base.clone();
            body["max_tokens"] = json!(4000);
            // cache: tools+system (~4,9k tokens; mínimo 4096 no Haiku 4.5, 512 no 5.5) e a conversa até aqui
            body["system"] = json!([{"type": "text", "text": system, "cache_control": {"type": "ephemeral"}}]);
            body["tools"] = tools.clone();
            let mut msgs = self.agenda.messages.clone();
            marca_cache(msgs.last_mut());
            body["messages"] = Value::Array(msgs);
            let response = self.claude.create(&body, beta)?;
            drop(body);
            log_uso(&response);
            let content = response.get("content").cloned().unwrap_or(json!([]));
            self.agenda.messages.push(json!({"role": "assistant", "content": content}));
            let stop = response.get("stop_reason").and_then(Value::as_str).unwrap_or("");
            if stop == "refusal" {
                self.agenda.messages.clear();
                return Ok("O modelo recusou esse pedido. 🤷".into());
            }
            let blocks = blocos(&response);
            if stop != "tool_use" {
                let t = blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                    .map(|b| b.get("text").and_then(Value::as_str).unwrap_or(""))
                    .collect::<Vec<_>>()
                    .join("\n");
                let t = t.trim();
                return Ok(if t.is_empty() { "👍".into() } else { t.to_string() });
            }
            let mut results = vec![];
            for b in blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use")) {
                let name = b.get("name").and_then(Value::as_str).unwrap_or("");
                let input = b.get("input").cloned().unwrap_or(json!({}));
                let (content, is_error) = match self.run_tool(name, &input) {
                    Ok(c) => (c, false),
                    Err(Erro::Http { code, corpo, origem: crate::util::Origem::Google, .. }) => {
                        (format!("Google Calendar {code}: {}", corta(&corpo, 300)), true)
                    }
                    Err(e) => (format!("{}: {e}", e.tipo()), true),
                };
                results.push(json!({"type": "tool_result", "tool_use_id": b.get("id").cloned().unwrap_or(Value::Null),
                                    "content": content, "is_error": is_error}));
            }
            self.agenda.messages.push(json!({"role": "user", "content": results}));
        }
        self.agenda.messages.clear();
        Ok("Me perdi nas ferramentas (10 voltas). Tenta de novo com outras palavras?".into())
    }
}

/// cache_control no último bloco da última mensagem (só na cópia que vai pro request)
fn marca_cache(m: Option<&mut Value>) {
    let Some(m) = m else { return };
    if let Some(last) = m.get_mut("content").and_then(Value::as_array_mut).and_then(|a| a.last_mut()) {
        last["cache_control"] = json!({"type": "ephemeral"});
    }
}

/// uma linha por chamada no journal: dá pra ver o gasto real sem teste pago
pub fn log_uso(response: &Value) {
    let u = &response["usage"];
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    println!(
        "uso {}: in {} · cache lido {} · cache gravado {} · out {}",
        response.get("model").and_then(Value::as_str).unwrap_or("?"),
        n("input_tokens"), n("cache_read_input_tokens"), n("cache_creation_input_tokens"), n("output_tokens")
    );
}

/// "[agora: quinta, 01/10/2026 20:40 | próximos: sex 02/10, sáb 03/10, …]" — o calendário vai pronto,
/// o modelo não precisa contar dia da semana (errava "sexta" → domingo)
pub fn carimbo(now: &crate::util::Dt) -> String {
    let dias: Vec<String> = (1..=7)
        .map(|i| {
            let d = now.date_naive() + chrono::Duration::days(i);
            format!("{} {}", dia_semana(&d).chars().take(3).collect::<String>(), d.format("%d/%m"))
        })
        .collect();
    // 00:58 de domingo, "amanhã às 6:30" virou segunda (18/10): quem não dormiu ainda chama o dia que começou de amanhã
    let madrugada = if chrono::Timelike::hour(now) < 5 {
        format!(" | madrugada: 'amanhã' de quem ainda não dormiu = hoje, {} {}", dia_semana(now).chars().take(3).collect::<String>(), now.format("%d/%m"))
    } else {
        String::new()
    };
    format!("[agora: {}, {}{madrugada} | próximos: {}]", dia_semana(now), now.format("%d/%m/%Y %H:%M"), dias.join(", "))
}
