//! Postos (só Rust): preço semanal por posto da ANP (xlsx "revendas_lpc") + rendimento do próprio
//! histórico → onde abastecer sai mais barato por km. O xlsx (12 MB descompactado) só é aberto no
//! processo à parte `argos --anp`, que grava `dados/postos.json` pequeno; o bot só lê esse JSON.
use crate::agenda::fold;
use crate::rede::http;
use crate::util::{fmt_f, py_float, Erro, Origem, R};
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::Path;

pub const ANP_LPC: &str = "https://www.gov.br/anp/pt-br/assuntos/precos-e-defesa-da-concorrencia/precos/levantamento-de-precos-de-combustiveis-ultimas-semanas-pesquisadas";
pub const NOMINATIM: &str = "https://nominatim.openstreetmap.org";
/// trechos cheio-a-cheio mínimos pra confiar no km/L de um posto ou bandeira
pub const MIN_TRECHOS: usize = 3;

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Preco {
    pub preco: f64,
    /// YYYY-MM-DD da coleta
    pub data: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Posto {
    pub nome: String,
    pub bandeira: String,
    pub endereco: String,
    pub bairro: String,
    pub cep: String,
    /// produto da ANP ("GASOLINA COMUM", "ETANOL"…) → último preço visto
    pub precos: BTreeMap<String, Preco>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// de onde veio a coordenada: "rua"/"cep" (Nominatim, aproximada) ou "voce" (localização confirmada)
    #[serde(default)]
    pub coord: String,
    /// como a pessoa chama o posto ("perto do mercado"), aprendido quando ela escolhe o posto depois de citar
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apelidos: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Postos {
    /// "2026-09-20_2026-09-26" (do nome do arquivo da ANP); vazio = nunca baixou
    pub semana: String,
    pub postos: BTreeMap<String, Posto>,
}

pub fn carregar(path: &Path) -> R<Postos> {
    match std::fs::read(path) {
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Postos::default()),
        Err(e) => Err(e.into()),
    }
}

pub fn gravar(p: &Postos, path: &Path) -> R<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(p)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// CNPJ só com dígitos, 14 casas (o xlsx guarda como número e perde o zero da frente)
pub fn cnpj(s: &str) -> Option<String> {
    let d: String = s.chars().filter(char::is_ascii_digit).collect();
    match d.len() {
        14 => Some(d),
        12 | 13 => Some(format!("{d:0>14}")),
        _ => None,
    }
}

/// célula do xlsx: número ("9308519000106", "9.308519000106E12") ou texto com pontuação
fn cnpj_celula(s: &str) -> Option<String> {
    if s.contains(['/', '-']) {
        return cnpj(s);
    }
    let n = py_float(s).ok().filter(|n| *n >= 1.0)?;
    cnpj(&format!("{n:.0}"))
}

pub fn cnpj_fmt(c: &str) -> String {
    if c.len() != 14 {
        return c.to_string();
    }
    format!("{}.{}.{}/{}-{}", &c[..2], &c[2..5], &c[5..8], &c[8..12], &c[12..])
}

/// "VIBRA" → "Vibra", "BRANCA" → "bandeira branca"
pub fn bandeira_fmt(b: &str) -> String {
    if b.eq_ignore_ascii_case("branca") || b.is_empty() {
        return "bandeira branca".into();
    }
    // o nome que aparece na placa do posto, não o da distribuidora
    match b.to_uppercase().as_str() {
        "RAIZEN" => "Shell".into(),
        "VIBRA" => "BR".into(),
        _ => titulo(b),
    }
}

/// "AUTO POSTO VILA FATIMA" → "Auto Posto Vila Fatima"; sigla curta, número e "J.S." ficam como estão
fn titulo(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let letras = w.chars().filter(|c| c.is_alphabetic()).count();
            if letras <= 2 && !matches!(w.to_lowercase().as_str(), "de" | "da" | "do" | "e") || w.contains('.') || w.chars().any(|c| c.is_ascii_digit()) {
                return w.to_string();
            }
            let w = w.to_lowercase();
            if matches!(w.as_str(), "de" | "da" | "do" | "das" | "dos" | "e") {
                return w;
            }
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// razão social → nome de placa: sem LTDA/EIRELI/ME, sem "comércio de combustíveis", "auto posto" vira "posto"
fn nome_curto(nome: &str) -> String {
    let mut ws: Vec<&str> = nome.split_whitespace().filter(|w| !matches!(w.trim_end_matches('.').to_uppercase().as_str(), "LTDA" | "EIRELI" | "ME" | "EPP" | "S/A" | "S.A")).collect();
    let up = ws.join(" ").to_uppercase();
    for frase in ["COMERCIAL DE COMBUSTÍVEIS", "COMERCIAL DE COMBUSTIVEIS", "COMÉRCIO DE COMBUSTÍVEIS", "COMERCIO DE COMBUSTIVEIS"] {
        if let Some(i) = up.find(frase) {
            let antes = up[..i].split_whitespace().count();
            ws.drain(antes..antes + frase.split_whitespace().count());
            break;
        }
    }
    if ws.len() > 2 && ws[0].eq_ignore_ascii_case("AUTO") && ws[1].eq_ignore_ascii_case("POSTO") {
        ws.remove(0);
    }
    if ws.is_empty() {
        return titulo(nome);
    }
    titulo(&ws.join(" "))
}

/// "Posto Vila Fatima" (bandeira branca) · "Paulista US (BR)"
pub fn nome_posto(p: &Posto) -> String {
    let n = nome_curto(&p.nome);
    if p.bandeira.eq_ignore_ascii_case("branca") || p.bandeira.is_empty() {
        n
    } else {
        format!("{n} ({})", bandeira_fmt(&p.bandeira))
    }
}

/// gasolina_comum → GASOLINA COMUM (o nome do produto na ANP)
pub fn produto_anp(combustivel: &str) -> String {
    combustivel.replace('_', " ").to_uppercase()
}

// ==========================================
// xlsx: zip (inflate do miniz_oxide) + XML lido à mão
// ==========================================

fn u16le(b: &[u8], i: usize) -> R<usize> {
    b.get(i..i + 2).map(|s| u16::from_le_bytes([s[0], s[1]]) as usize).ok_or_else(|| Erro::Valor("zip truncado".into()))
}

fn u32le(b: &[u8], i: usize) -> R<usize> {
    b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize).ok_or_else(|| Erro::Valor("zip truncado".into()))
}

/// conteúdo de um arquivo dentro do zip (stored ou deflate); None = não existe
pub fn zip_ler(zip: &[u8], nome: &str) -> R<Option<Vec<u8>>> {
    // fim do diretório central: assinatura PK\x05\x06 nos últimos 64 KB
    let ini = zip.len().saturating_sub(22 + 65_535);
    let eocd = (ini..zip.len().saturating_sub(21)).rev().find(|&i| zip[i..i + 4] == [0x50, 0x4b, 0x05, 0x06]).ok_or_else(|| Erro::Valor("não é zip".into()))?;
    let (n, mut p) = (u16le(zip, eocd + 10)?, u32le(zip, eocd + 16)?);
    for _ in 0..n {
        if u32le(zip, p)? != 0x0201_4b50 {
            return Err(Erro::Valor("diretório do zip inválido".into()));
        }
        let (metodo, tam) = (u16le(zip, p + 10)?, u32le(zip, p + 20)?);
        let (ln, le, lc, local) = (u16le(zip, p + 28)?, u16le(zip, p + 30)?, u16le(zip, p + 32)?, u32le(zip, p + 42)?);
        let este = zip.get(p + 46..p + 46 + ln).ok_or_else(|| Erro::Valor("zip truncado".into()))?;
        if este == nome.as_bytes() {
            let dados = local + 30 + u16le(zip, local + 26)? + u16le(zip, local + 28)?;
            let bruto = zip.get(dados..dados + tam).ok_or_else(|| Erro::Valor("zip truncado".into()))?;
            return match metodo {
                0 => Ok(Some(bruto.to_vec())),
                8 => miniz_oxide::inflate::decompress_to_vec(bruto).map(Some).map_err(|e| Erro::Valor(format!("inflate: {e:?}"))),
                m => Err(Erro::Valor(format!("zip: método {m} não suportado"))),
            };
        }
        p += 46 + ln + le + lc;
    }
    Ok(None)
}

fn xml_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let fim = rest[i..].find(';').map(|j| i + j);
        let Some(f) = fim else {
            out.push_str(&rest[i..]);
            return out;
        };
        let ent = &rest[i + 1..f];
        let c = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
            _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match c {
            Some(c) => out.push(c),
            None => out.push_str(&rest[i..=f]),
        }
        rest = &rest[f + 1..];
    }
    out.push_str(rest);
    out
}

/// textos de todos os <t>…</t> (ou <t …>…</t>) em `s`, concatenados
fn textos_t(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<t") {
        let depois = &rest[i + 2..];
        if !(depois.starts_with('>') || depois.starts_with(' ')) {
            rest = depois;
            continue;
        }
        let Some(a) = depois.find('>') else { break };
        if depois[..a].ends_with('/') {
            rest = &depois[a + 1..];
            continue;
        }
        let corpo = &depois[a + 1..];
        let Some(f) = corpo.find("</t>") else { break };
        out.push_str(&xml_unescape(&corpo[..f]));
        rest = &corpo[f + 4..];
    }
    out
}

fn shared_strings(xml: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = xml;
    while let Some(i) = rest.find("<si") {
        let corpo = &rest[i..];
        let Some(f) = corpo.find("</si>") else { break };
        out.push(textos_t(&corpo[..f]));
        rest = &corpo[f + 5..];
    }
    out
}

fn atributo<'a>(tag: &'a str, nome: &str) -> Option<&'a str> {
    let k = format!(" {nome}=\"");
    let i = tag.find(&k)? + k.len();
    Some(&tag[i..i + tag[i..].find('"')?])
}

/// "AB12" → 27 (coluna zero-based)
fn coluna(r: &str) -> usize {
    r.bytes().take_while(u8::is_ascii_alphabetic).fold(0, |acc, b| acc * 26 + (b.to_ascii_uppercase() - b'A' + 1) as usize) - 1
}

/// linhas da planilha como texto (shared strings resolvidas, células vazias = "")
pub fn xlsx_linhas(xlsx: &[u8]) -> R<Vec<Vec<String>>> {
    let ss = match zip_ler(xlsx, "xl/sharedStrings.xml")? {
        Some(b) => shared_strings(&String::from_utf8_lossy(&b)),
        None => vec![],
    };
    let folha = zip_ler(xlsx, "xl/worksheets/sheet1.xml")?.ok_or_else(|| Erro::Valor("xlsx sem sheet1".into()))?;
    let xml = String::from_utf8_lossy(&folha);
    let mut linhas = vec![];
    let mut rest: &str = &xml;
    while let Some(i) = rest.find("<row") {
        let corpo = &rest[i..];
        let fim = corpo.find("</row>").unwrap_or(corpo.len());
        let row = &corpo[..fim];
        let mut cels: Vec<String> = vec![];
        let mut r = row;
        while let Some(c) = r.find("<c ") {
            let cel = &r[c..];
            let Some(a) = cel.find('>') else { break };
            let tag = &cel[..a];
            let (valor, prox) = if tag.ends_with('/') {
                (String::new(), a + 1)
            } else {
                let f = cel.find("</c>").unwrap_or(cel.len());
                let dentro = &cel[a + 1..f];
                let v = match dentro.find("<v>").and_then(|i| dentro[i + 3..].find("</v>").map(|j| &dentro[i + 3..i + 3 + j])) {
                    Some(v) if atributo(tag, "t") == Some("s") => v.parse::<usize>().ok().and_then(|k| ss.get(k).cloned()).unwrap_or_default(),
                    Some(v) => xml_unescape(v),
                    None => textos_t(dentro),
                };
                (v, (f + 4).min(cel.len()))
            };
            let col = atributo(tag, "r").map(coluna).unwrap_or(cels.len());
            if cels.len() <= col {
                cels.resize(col + 1, String::new());
            }
            cels[col] = valor;
            r = &cel[prox..];
        }
        linhas.push(cels);
        rest = &corpo[fim.min(corpo.len())..];
        if fim == corpo.len() {
            break;
        }
    }
    Ok(linhas)
}

/// data da coleta: serial do Excel (46286) ou "21/09/2026"
fn data_coleta(s: &str) -> Option<String> {
    if let Ok(n) = s.trim().parse::<f64>() {
        let d = NaiveDate::from_ymd_opt(1899, 12, 30)? + Duration::days(n as i64);
        return Some(d.format("%Y-%m-%d").to_string());
    }
    NaiveDate::parse_from_str(s.trim(), "%d/%m/%Y").ok().map(|d| d.format("%Y-%m-%d").to_string())
}

/// linhas do xlsx "revendas_lpc" do município → postos (sem coordenada)
pub fn postos_da_planilha(linhas: &[Vec<String>], municipio: &str, estado: &str) -> R<BTreeMap<String, Posto>> {
    let hi = linhas.iter().position(|l| l.first().map(|c| c.trim() == "CNPJ").unwrap_or(false)).ok_or_else(|| Erro::Valor("planilha da ANP sem cabeçalho CNPJ".into()))?;
    let cab: Vec<String> = linhas[hi].iter().map(|c| fold(c.trim())).collect();
    let col = |nome: &str| cab.iter().position(|c| c == nome).ok_or_else(|| Erro::Valor(format!("planilha da ANP sem a coluna {nome}")));
    let (c_cnpj, c_razao, c_fant, c_end, c_num, c_bairro, c_cep) = (col("cnpj")?, col("razao")?, col("fantasia")?, col("endereco")?, col("numero")?, col("bairro")?, col("cep")?);
    let (c_mun, c_uf, c_band, c_prod, c_preco, c_data) = (col("municipio")?, col("estado")?, col("bandeira")?, col("produto")?, col("preco de revenda")?, col("data da coleta")?);
    let (mun, uf) = (fold(municipio), fold(estado));
    let mut out: BTreeMap<String, Posto> = BTreeMap::new();
    let get = |l: &Vec<String>, i: usize| l.get(i).map(|s| s.trim().to_string()).unwrap_or_default();
    for l in &linhas[hi + 1..] {
        if fold(&get(l, c_mun)) != mun || fold(&get(l, c_uf)) != uf {
            continue;
        }
        let Some(id) = cnpj_celula(&get(l, c_cnpj)) else { continue };
        let (Ok(preco), Some(data)) = (py_float(&get(l, c_preco)), data_coleta(&get(l, c_data))) else { continue };
        let p = out.entry(id).or_insert_with(|| {
            let fant = get(l, c_fant);
            let num = get(l, c_num);
            Posto {
                nome: if fant.is_empty() { get(l, c_razao) } else { fant },
                bandeira: get(l, c_band),
                endereco: if num.is_empty() { get(l, c_end) } else { format!("{}, {num}", get(l, c_end)) },
                bairro: get(l, c_bairro),
                cep: py_float(&get(l, c_cep)).map(|n| format!("{n:08.0}")).unwrap_or_else(|_| get(l, c_cep).chars().filter(char::is_ascii_digit).collect()),
                ..Default::default()
            }
        });
        p.precos.insert(get(l, c_prod).to_uppercase(), Preco { preco, data });
    }
    Ok(out)
}

/// junta a semana nova no que já existe: preço e cadastro novos por cima; coordenada fica
/// (principalmente a confirmada por você); posto que saiu da amostra fica com o preço antigo
pub fn mesclar(base: &mut Postos, semana: &str, novos: BTreeMap<String, Posto>) {
    for (id, n) in novos {
        match base.postos.get_mut(&id) {
            Some(p) => {
                let (lat, lon, coord, apelidos) = (p.lat, p.lon, std::mem::take(&mut p.coord), std::mem::take(&mut p.apelidos));
                let mut precos = std::mem::take(&mut p.precos);
                precos.extend(n.precos.clone());
                *p = Posto { precos, lat, lon, coord, apelidos, ..n };
            }
            None => {
                base.postos.insert(id, n);
            }
        }
    }
    base.semana = semana.to_string();
}

// ==========================================
// rede (só no `argos --anp`)
// ==========================================

fn baixar(agent: &ureq::Agent, url: &str, origem: Origem) -> R<Vec<u8>> {
    let r = agent.get(url).timeout(std::time::Duration::from_secs(120)).call();
    let mut b = vec![];
    http(r, origem)?.into_reader().take(64 << 20).read_to_end(&mut b)?;
    Ok(b)
}

/// link do xlsx de revendas mais recente na página da ANP
pub fn link_mais_recente(html: &str) -> Option<String> {
    let re = regex::Regex::new(r#"href="([^"]*revendas_lpc_[^"]*\.xlsx)""#).ok()?;
    re.captures_iter(html).map(|c| c[1].to_string()).max_by_key(|u| u.rsplit('/').next().unwrap_or("").to_string())
}

/// "…/revendas_lpc_2026-09-20_2026-09-26.xlsx" → "2026-09-20_2026-09-26"
pub fn semana_do_link(url: &str) -> String {
    let f = url.rsplit('/').next().unwrap_or(url);
    f.trim_start_matches("revendas_lpc_").trim_end_matches(".xlsx").to_string()
}

fn nominatim(agent: &ureq::Agent, base: &str, params: &[(&str, String)]) -> R<Option<(f64, f64)>> {
    let url = format!("{base}/search?format=json&limit=1&countrycodes=br&{}", crate::util::urlencode(params));
    let r = agent.get(&url).timeout(std::time::Duration::from_secs(20)).call();
    let v = crate::rede::ler_json(http(r, Origem::Osm)?)?;
    let first = v.get(0);
    let num = |k: &str| first.and_then(|f| f.get(k)).and_then(|x| x.as_str()).and_then(|s| s.parse::<f64>().ok());
    Ok(num("lat").zip(num("lon")))
}

pub struct Fontes<'a> {
    pub anp: &'a str,
    pub nominatim: &'a str,
    /// pausa entre consultas ao Nominatim (política de uso: 1 por segundo)
    pub pausa: std::time::Duration,
}

/// `argos --anp`: baixa a semana nova (se houver), mescla, acha coordenada de quem não tem, grava.
/// Devolve um resumo pro log.
pub fn atualizar(agent: &ureq::Agent, f: &Fontes, municipio: &str, estado: &str, path: &Path) -> R<String> {
    let mut base = carregar(path)?;
    let html = String::from_utf8_lossy(&baixar(agent, f.anp, Origem::Anp)?).to_string();
    let link = link_mais_recente(&html).ok_or_else(|| Erro::Msg("não achei o xlsx de revendas na página da ANP".into()))?;
    let link = if link.starts_with("http") { link } else { format!("{}/{}", f.anp.trim_end_matches('/'), link.trim_start_matches('/')) };
    let semana = semana_do_link(&link);
    let mut resumo = vec![];
    if semana != base.semana {
        let xlsx = baixar(agent, &link, Origem::Anp)?;
        let linhas = xlsx_linhas(&xlsx)?;
        drop(xlsx);
        let novos = postos_da_planilha(&linhas, municipio, estado)?;
        resumo.push(format!("semana {semana}: {} postos em {municipio}", novos.len()));
        mesclar(&mut base, &semana, novos);
    } else {
        resumo.push(format!("semana {semana} já baixada"));
    }
    let mut achados = 0;
    let sem: Vec<String> = base.postos.iter().filter(|(_, p)| p.lat.is_none()).map(|(k, _)| k.clone()).collect();
    for id in &sem {
        let p = base.postos[id].clone();
        std::thread::sleep(f.pausa);
        let mut achou = nominatim(agent, f.nominatim, &[("street", p.endereco.replace(',', "")), ("city", municipio.into()), ("state", estado.into())])?.map(|c| (c, "rua"));
        if achou.is_none() && p.cep.len() == 8 {
            std::thread::sleep(f.pausa);
            achou = nominatim(agent, f.nominatim, &[("postalcode", format!("{}-{}", &p.cep[..5], &p.cep[5..]))])?.map(|c| (c, "cep"));
        }
        if let Some(((lat, lon), fonte)) = achou {
            let q = base.postos.get_mut(id).unwrap();
            (q.lat, q.lon, q.coord) = (Some(lat), Some(lon), fonte.into());
            achados += 1;
        }
    }
    if !sem.is_empty() {
        resumo.push(format!("coordenadas: {achados}/{}", sem.len()));
    }
    gravar(&base, path)?;
    Ok(resumo.join(" · "))
}

// ==========================================
// distância, candidatos e ranking (puros)
// ==========================================

pub fn distancia_km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    6371.0 * 2.0 * h.sqrt().asin()
}

/// os `n` postos com coordenada mais perto de `loc`: (cnpj, km)
pub fn mais_proximos(p: &Postos, loc: (f64, f64), n: usize) -> Vec<(String, f64)> {
    let mut v: Vec<(String, f64)> = p.postos.iter().filter_map(|(k, x)| Some((k.clone(), distancia_km(loc, (x.lat?, x.lon?))))).collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    v.truncate(n);
    v
}

/// "350 m" / "1,2 km"
pub fn dist_fmt(km: f64) -> String {
    if km < 1.0 {
        format!("{} m", ((km * 1000.0) / 50.0).round() as i64 * 50)
    } else {
        format!("{} km", fmt_f(km, 1).replace('.', ","))
    }
}

/// tag `posto=<cnpj>` do campo detalhes
pub fn posto_da_linha(detalhes: &str) -> Option<&str> {
    detalhes.split('|').find_map(|t| t.strip_prefix("posto="))
}

/// km/L de cada trecho cheio-a-cheio (mesmo par do motoregis: dois abastecimentos cheios seguidos,
/// o segundo sem gap) com `combustivel` no primeiro. O trecho é do posto do PRIMEIRO abastecimento:
/// foi o combustível dele que rodou esses km. → (cnpj ou None, km/L)
pub fn trechos(linhas: &[Vec<String>], combustivel: &str) -> Vec<(Option<String>, f64)> {
    let fuel: Vec<&Vec<String>> = linhas.iter().skip(1).filter(|r| r.len() > 5 && r[2] == "1").collect();
    let mut out = vec![];
    for w in fuel.windows(2) {
        let (prev, curr) = (w[0], w[1]);
        if !(prev[5].contains("|cheio") && curr[5].contains("|cheio")) || curr[5].contains("gap") || prev[5].split('|').next() != Some(combustivel) {
            continue;
        }
        if let (Ok(a), Ok(b), Ok(l)) = (py_float(&curr[1]), py_float(&prev[1]), py_float(&curr[4])) {
            if a - b > 0.0 && l > 0.0 {
                out.push((posto_da_linha(&prev[5]).map(String::from), (a - b) / l));
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fonte {
    Posto,
    Bandeira,
    Geral,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Opcao {
    pub cnpj: String,
    pub nome: String,
    pub preco: f64,
    pub data: String,
    pub km_l: Option<f64>,
    pub fonte: Fonte,
    /// trechos usados na estimativa
    pub n: usize,
    /// R$ por 100 km (None sem nenhum histórico)
    pub custo_100: Option<f64>,
    pub dist: Option<f64>,
}

fn media(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

/// postos com preço do `combustivel`, ordenados por custo/100 km (preço ÷ km/L estimado: o do posto
/// com MIN_TRECHOS+, senão o da bandeira, senão a média geral). Com `perto`, só os até `raio_km`.
pub fn ranking(linhas: &[Vec<String>], p: &Postos, combustivel: &str, perto: Option<(f64, f64)>, raio_km: f64) -> Vec<Opcao> {
    let ts = trechos(linhas, combustivel);
    let geral: Vec<f64> = ts.iter().map(|t| t.1).collect();
    let mut por_posto: HashMap<&str, Vec<f64>> = HashMap::new();
    let mut por_bandeira: HashMap<String, Vec<f64>> = HashMap::new();
    for (c, k) in &ts {
        if let Some(c) = c.as_deref() {
            por_posto.entry(c).or_default().push(*k);
            if let Some(x) = p.postos.get(c) {
                por_bandeira.entry(x.bandeira.to_uppercase()).or_default().push(*k);
            }
        }
    }
    let prod = produto_anp(combustivel);
    let mut out: Vec<Opcao> = p
        .postos
        .iter()
        .filter_map(|(id, x)| {
            let pr = x.precos.get(&prod)?;
            let dist = match (perto, x.lat, x.lon) {
                (Some(l), Some(a), Some(b)) => Some(distancia_km(l, (a, b))),
                (Some(_), _, _) => return None,
                _ => None,
            };
            if dist.map(|d| d > raio_km).unwrap_or(false) {
                return None;
            }
            let band = por_bandeira.get(&x.bandeira.to_uppercase()).filter(|v| v.len() >= MIN_TRECHOS);
            let (km_l, fonte, n) = match (por_posto.get(id.as_str()).filter(|v| v.len() >= MIN_TRECHOS), band) {
                (Some(v), _) => (Some(media(v)), Fonte::Posto, v.len()),
                (None, Some(v)) => (Some(media(v)), Fonte::Bandeira, v.len()),
                _ if !geral.is_empty() => (Some(media(&geral)), Fonte::Geral, geral.len()),
                _ => (None, Fonte::Geral, 0),
            };
            Some(Opcao {
                cnpj: id.clone(),
                nome: nome_posto(x),
                preco: pr.preco,
                data: pr.data.clone(),
                km_l,
                fonte,
                n,
                custo_100: km_l.map(|k| pr.preco / k * 100.0),
                dist,
            })
        })
        .collect();
    out.sort_by(|a, b| a.custo_100.unwrap_or(a.preco).total_cmp(&b.custo_100.unwrap_or(b.preco)).then(a.preco.total_cmp(&b.preco)));
    out
}

fn virgula(x: f64, casas: usize) -> String {
    fmt_f(x, casas).replace('.', ",")
}

fn ddmm(iso: &str) -> String {
    NaiveDate::parse_from_str(iso, "%Y-%m-%d").map(|d| d.format("%d/%m").to_string()).unwrap_or_else(|_| iso.to_string())
}

/// mensagem do ranking (top `n`)
pub fn texto_ranking(ops: &[Opcao], combustivel: &str, n: usize, perto: bool, raio_km: f64) -> String {
    let titulo = format!("⛽ <b>Onde abastecer</b> · {}", combustivel.replace('_', " "));
    if ops.is_empty() {
        return if perto {
            format!("{titulo}\nNenhum posto da pesquisa da ANP a até {} km daqui.", virgula(raio_km, 0))
        } else {
            format!("{titulo}\nAinda não tenho preços da ANP. (O bot baixa toda manhã; dá pra forçar com <code>argos --anp</code>.)")
        };
    }
    let mostra = &ops[..ops.len().min(n)];
    let recente = mostra.iter().map(|o| o.data.as_str()).max().unwrap_or("");
    // preço de semana anterior (o mesclar guarda) só é marcado se for 1 semana+ mais velho
    let velho = |d: &str| -> bool {
        let p = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
        matches!((p(d), p(recente)), (Some(a), Some(b)) if (b - a).num_days() >= 7)
    };
    let mut linhas = vec![titulo];
    for (i, o) in mostra.iter().enumerate() {
        let mut l = format!("{}. <b>{}</b> — R$ {}", i + 1, crate::rascunho::esc(&o.nome), virgula(o.preco, 2));
        if let Some(c) = o.custo_100 {
            l += &format!(" → R$ {}/100 km", virgula(c, 2));
        }
        if let Some(d) = o.dist {
            l += &format!(" · {}", dist_fmt(d));
        }
        // rendimento próprio (posto/bandeira) vai na linha; a média geral, que é igual pra todos, no rodapé
        if let (Some(k), Fonte::Posto | Fonte::Bandeira) = (o.km_l, &o.fonte) {
            l += &format!(" · {} km/L {}", virgula(k, 1), if o.fonte == Fonte::Posto { "aqui" } else { "na bandeira" });
        }
        if velho(&o.data) {
            l += &format!(" <i>(preço de {})</i>", ddmm(&o.data));
        }
        linhas.push(l);
    }
    let mut rodape = vec![];
    if let Some(o) = mostra.iter().find(|o| o.fonte == Fonte::Geral && o.km_l.is_some()) {
        rodape.push(format!("sua média {} km/L ({} {})", virgula(o.km_l.unwrap(), 1), o.n, if o.n == 1 { "tanque" } else { "tanques" }));
    }
    rodape.push(format!("ANP de {}", ddmm(recente)));
    linhas.push(format!("\n<i>{}</i>", rodape.join(" · ")));
    linhas.join("\n")
}

/// veículo e combustível pro ranking: o de mais abastecimentos e o combustível mais usado nele
pub fn mais_usado(por_veiculo: &[(String, Vec<Vec<String>>)]) -> Option<(String, String, usize)> {
    let mut melhor: Option<(String, String, usize)> = None;
    for (nome, linhas) in por_veiculo {
        let mut cont: BTreeMap<&str, usize> = BTreeMap::new();
        for r in linhas.iter().skip(1).filter(|r| r.len() > 5 && r[2] == "1") {
            *cont.entry(r[5].split('|').next().unwrap_or("")).or_default() += 1;
        }
        let total: usize = cont.values().sum();
        if let Some((f, _)) = cont.iter().max_by_key(|(_, n)| **n) {
            if melhor.as_ref().map(|m| total > m.2).unwrap_or(true) {
                melhor = Some((nome.clone(), f.to_string(), total));
            }
        }
    }
    melhor
}

/// guarda a localização confirmada como coordenada do posto (melhor que a da rua)
pub fn fixar_coord(path: &Path, id: &str, loc: (f64, f64)) -> R<()> {
    let mut p = carregar(path)?;
    if let Some(x) = p.postos.get_mut(id) {
        (x.lat, x.lon, x.coord) = (Some(loc.0), Some(loc.1), "voce".into());
        gravar(&p, path)?;
    }
    Ok(())
}

/// palavras que não identificam posto nenhum ("é o posto perto do cena" → ["perto", "cena"])
const VAZIAS: &[&str] = &["e", "o", "a", "os", "as", "do", "da", "dos", "das", "de", "no", "na", "em", "um", "uma", "posto", "foi", "ali", "aqui", "esse", "este", "aquele", "que", "eu", "abasteci", "abasteco", "la", "com"];

/// o que sobra do texto pra comparar: fold, sem pontuação e sem palavra vazia
pub fn termos(texto: &str) -> Vec<String> {
    fold(texto)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 2 && !VAZIAS.contains(w))
        .map(String::from)
        .collect()
}

/// postos citados pelo nome num texto livre: apelido aprendido igual vale sozinho; senão, todos os termos têm
/// que aparecer em nome + bandeira + endereço + bairro. Mais de 3 candidatos = vago demais, devolve vazio.
pub fn por_nome(p: &Postos, texto: &str) -> Vec<String> {
    let t = termos(texto);
    if t.is_empty() {
        return vec![];
    }
    let chave = t.join(" ");
    let por_apelido: Vec<String> = p.postos.iter().filter(|(_, x)| x.apelidos.iter().any(|a| termos(a).join(" ") == chave)).map(|(k, _)| k.clone()).collect();
    if !por_apelido.is_empty() {
        return por_apelido;
    }
    let achados: Vec<String> = p
        .postos
        .iter()
        .filter(|(_, x)| {
            let campos: Vec<String> = termos(&format!("{} {} {} {}", x.nome, x.bandeira, x.endereco, x.bairro));
            t.iter().all(|w| campos.iter().any(|c| c == w || (w.len() >= 4 && c.starts_with(w.as_str()))))
        })
        .map(|(k, _)| k.clone())
        .collect();
    if achados.len() > 3 {
        vec![]
    } else {
        achados
    }
}

/// guarda o apelido no posto (sem repetir)
pub fn fixar_apelido(path: &Path, id: &str, apelido: &str) -> R<()> {
    let mut p = carregar(path)?;
    if let Some(x) = p.postos.get_mut(id) {
        let chave = termos(apelido).join(" ");
        if !chave.is_empty() && !x.apelidos.iter().any(|a| termos(a).join(" ") == chave) {
            x.apelidos.push(apelido.trim().to_string());
            gravar(&p, path)?;
        }
    }
    Ok(())
}
