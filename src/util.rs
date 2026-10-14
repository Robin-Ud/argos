//! Utilidades: erro, formatação de número igual ao Python, datas ISO, url-encode, JSON estilo json.dumps.
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use serde_json::Value;
use std::fmt;
use std::io;

// ==========================================
// ERRO (o equivalente das exceções do argos.py)
// ==========================================

#[derive(Debug)]
pub enum Erro {
    /// resposta HTTP >= 400 (urllib.error.HTTPError / anthropic.APIStatusError)
    Http { code: u16, motivo: String, corpo: String, origem: Origem },
    /// falha de conexão / timeout (URLError)
    Rede(String),
    /// ValueError do Python (validação, número inválido, data inválida)
    Valor(String),
    /// KeyError / IndexError / TypeError: dado com formato inesperado
    Chave(String),
    /// RuntimeError e o resto
    Msg(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Origem {
    Google,
    Anthropic,
    Telegram,
    Clima,
    Inmet,
    Anp,
    Osm,
}

pub type R<T> = Result<T, Erro>;

impl Erro {
    /// nome da classe da exceção, pro `f'{type(e).__name__}: {e}'` da agenda
    pub fn tipo(&self) -> &'static str {
        match self {
            Erro::Http { origem: Origem::Anthropic, .. } => "APIStatusError",
            Erro::Http { .. } => "HTTPError",
            Erro::Rede(_) => "URLError",
            Erro::Valor(_) => "ValueError",
            Erro::Chave(_) => "KeyError",
            Erro::Msg(_) => "RuntimeError",
        }
    }
}

impl fmt::Display for Erro {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Erro::Http { code, corpo, origem: Origem::Anthropic, .. } => write!(f, "Error code: {code} - {corpo}"),
            Erro::Http { code, motivo, .. } => write!(f, "HTTP Error {code}: {motivo}"),
            Erro::Rede(s) => write!(f, "<urlopen error {s}>"),
            Erro::Valor(s) | Erro::Chave(s) | Erro::Msg(s) => write!(f, "{s}"),
        }
    }
}

impl From<io::Error> for Erro {
    fn from(e: io::Error) -> Self {
        Erro::Msg(format!("[Errno] {e}"))
    }
}

impl From<serde_json::Error> for Erro {
    fn from(e: serde_json::Error) -> Self {
        Erro::Valor(format!("JSON inválido: {e}"))
    }
}


/// row[i] com IndexError igual ao Python
pub fn col(row: &[String], i: usize) -> R<&str> {
    row.get(i).map(|s| s.as_str()).ok_or_else(|| Erro::Chave("list index out of range".into()))
}

/// trunca por caractere (o `s[:n]` do Python)
pub fn corta(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ==========================================
// NÚMEROS NO FORMATO DO PYTHON
// ==========================================

fn especial(x: f64) -> Option<String> {
    if x.is_nan() {
        Some("nan".into())
    } else if x.is_infinite() {
        Some(if x > 0.0 { "inf".into() } else { "-inf".into() })
    } else {
        None
    }
}

/// f'{x:.Nf}' (arredondamento exato, metade-pro-par igual ao Python)
pub fn fmt_f(x: f64, prec: usize) -> String {
    especial(x).unwrap_or_else(|| format!("{:.*}", prec, x))
}

/// f'{x:+.Nf}'
pub fn fmt_f_sinal(x: f64, prec: usize) -> String {
    let s = fmt_f(x, prec);
    if s.starts_with('-') || x.is_nan() { s } else { format!("+{s}") }
}

/// f'{x:g}' — 6 algarismos significativos, sem zeros à direita, expoente fora de [-4, 6)
pub fn fmt_g(x: f64) -> String {
    if let Some(s) = especial(x) {
        return s;
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let e = format!("{:.5e}", x);
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    if (-4..6).contains(&exp) {
        let s = format!("{:.*}", (5 - exp) as usize, x);
        tira_zeros(&s)
    } else {
        let m = tira_zeros(mant);
        format!("{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

/// f'{x:+g}'
pub fn fmt_g_sinal(x: f64) -> String {
    let s = fmt_g(x);
    if s.starts_with('-') || x.is_nan() { s } else { format!("+{s}") }
}

/// f'{x:+.0%}'
pub fn fmt_pct_sinal(x: f64) -> String {
    format!("{}%", fmt_f_sinal(x * 100.0, 0))
}

fn tira_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// str(float) / repr(float): menor representação que volta igual, com '.0' e 'e+XX' do Python
pub fn repr_f64(x: f64) -> String {
    if let Some(s) = especial(x) {
        return s;
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let e = format!("{:e}", x.abs()); // "3.7865e3": dígitos mínimos
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let sinal = if x < 0.0 { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let n = digits.len() as i32;
        let s = if exp >= 0 {
            if n > exp + 1 {
                format!("{}.{}", &digits[..(exp + 1) as usize], &digits[(exp + 1) as usize..])
            } else {
                format!("{}{}.0", digits, "0".repeat((exp + 1 - n) as usize))
            }
        } else {
            format!("0.{}{}", "0".repeat((-exp - 1) as usize), digits)
        };
        format!("{sinal}{s}")
    } else {
        let m = if digits.len() > 1 { format!("{}.{}", &digits[..1], &digits[1..]) } else { digits };
        format!("{sinal}{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    }
}

/// str() de um número vindo do JSON (int fica int, float ganha '.0')
pub fn str_num(v: &Value) -> String {
    match v {
        Value::Number(n) if n.is_f64() => repr_f64(n.as_f64().unwrap()),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "True".into() } else { "False".into() },
        Value::Null => "None".into(),
        outro => outro.to_string(),
    }
}

/// número do JSON como f64 (TypeError se não for número)
pub fn num(v: &Value) -> R<f64> {
    v.as_f64().ok_or_else(|| Erro::Chave(format!("esperava número, veio {v}")))
}

/// repr() de string do Python ('aspas simples')
pub fn repr_str(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c)
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// repr() de list[str]
pub fn repr_lista(itens: &[String]) -> String {
    format!("[{}]", itens.iter().map(|s| repr_str(s)).collect::<Vec<_>>().join(", "))
}

/// float(s) do Python: aceita espaços em volta, '_' entre dígitos, inf/nan
pub fn py_float(s: &str) -> R<f64> {
    let t = s.trim();
    let erro = || Erro::Valor(format!("could not convert string to float: {}", repr_str(s)));
    let limpo = sem_underscore(t).ok_or_else(erro)?;
    let l = limpo.to_ascii_lowercase();
    let corpo = l.trim_start_matches(['+', '-']);
    if !matches!(corpo, "inf" | "infinity" | "nan") && !corpo.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | '+' | '-')) {
        return Err(erro());
    }
    limpo.parse::<f64>().map_err(|_| erro())
}

/// int(s) do Python (base 10)
pub fn py_int(s: &str) -> R<i64> {
    let t = s.trim();
    let erro = || Erro::Valor(format!("invalid literal for int() with base 10: {}", repr_str(s)));
    let limpo = sem_underscore(t).ok_or_else(erro)?;
    let l = limpo.strip_prefix('+').unwrap_or(&limpo);
    if l.is_empty() || l.starts_with('+') {
        return Err(erro());
    }
    l.parse::<i64>().map_err(|_| erro())
}

fn sem_underscore(t: &str) -> Option<String> {
    if !t.contains('_') {
        return Some(t.to_string());
    }
    let c: Vec<char> = t.chars().collect();
    for (i, ch) in c.iter().enumerate() {
        if *ch == '_' && !(i > 0 && i + 1 < c.len() && c[i - 1].is_ascii_digit() && c[i + 1].is_ascii_digit()) {
            return None;
        }
    }
    Some(t.replace('_', ""))
}

/// round(x, n) do Python (arredonda o valor binário exato, metade-pro-par)
pub fn py_round(x: f64, n: usize) -> f64 {
    format!("{:.*}", n, x).parse().unwrap_or(x)
}

// ==========================================
// DATAS
// ==========================================

/// datetime.strptime(s, '%d/%m/%y') — dia/mês com 1 ou 2 dígitos, ano com 2
pub fn parse_ddmmyy(s: &str) -> R<NaiveDate> {
    let erro = || Erro::Valor(format!("time data {} does not match format '%d/%m/%y'", repr_str(s)));
    let p: Vec<&str> = s.split('/').collect();
    if p.len() != 3 {
        return Err(erro());
    }
    let ok12 = |x: &str| (1..=2).contains(&x.len()) && x.bytes().all(|b| b.is_ascii_digit());
    if !ok12(p[0]) || !ok12(p[1]) || p[2].len() != 2 || !p[2].bytes().all(|b| b.is_ascii_digit()) {
        return Err(erro());
    }
    let (d, m, y): (u32, u32, i32) = (p[0].parse().unwrap(), p[1].parse().unwrap(), p[2].parse().unwrap());
    let ano = if y < 69 { 2000 + y } else { 1900 + y };
    NaiveDate::from_ymd_opt(ano, m, d).ok_or_else(erro)
}


pub type Dt = DateTime<FixedOffset>;

static FUSO: std::sync::OnceLock<Tz> = std::sync::OnceLock::new();

#[cfg(test)]
thread_local! {
    static FUSO_TESTE: std::cell::Cell<Option<Tz>> = const { std::cell::Cell::new(None) };
}

/// fuso desta instância (configs.json → `fuso`, definido pela CLI com `define_fuso`); sem nada, São Paulo
pub fn fuso() -> Tz {
    #[cfg(test)]
    if let Some(t) = FUSO_TESTE.with(|f| f.get()) {
        return t;
    }
    *FUSO.get().unwrap_or(&Tz::America__Sao_Paulo)
}

/// uma vez, quando o processo sobe (os testes usam `fuso_no_teste`, que vale só na thread do teste)
pub fn define_fuso(tz: Tz) {
    let _ = FUSO.set(tz);
}

#[cfg(test)]
pub fn fuso_no_teste(tz: Option<Tz>) {
    FUSO_TESTE.with(|f| f.set(tz));
}

/// "America/Sao_Paulo" → "São Paulo": como o fuso aparece no prompt
pub fn nome_do_fuso(tz: Tz) -> String {
    let cidade = tz.name().rsplit('/').next().unwrap_or(tz.name());
    let com_acento = [("Sao_Paulo", "São Paulo"), ("Cuiaba", "Cuiabá"), ("Belem", "Belém"), ("Maceio", "Maceió"), ("Araguaina", "Araguaína"),
                      ("Santarem", "Santarém"), ("Eirunepe", "Eirunepé"), ("Noronha", "Fernando de Noronha"), ("Lisbon", "Lisboa")];
    match com_acento.iter().find(|(a, _)| *a == cidade) {
        Some((_, b)) => b.to_string(),
        None => cidade.replace('_', " "),
    }
}

/// datetime.now(TZ), com precisão de microssegundo como no Python
pub fn agora() -> Dt {
    let now = Utc::now();
    let now = now.with_nanosecond(now.nanosecond() / 1000 * 1000).unwrap_or(now);
    now.with_timezone(&fuso()).fixed_offset()
}

/// datetime.fromtimestamp(ts, TZ)
pub fn de_timestamp(ts: i64) -> Dt {
    Utc.timestamp_opt(ts, 0).single().unwrap_or_default().with_timezone(&fuso()).fixed_offset()
}

/// anexa o fuso da instância a um horário sem fuso (dt.replace(tzinfo=TZ))
pub fn no_fuso(naive: NaiveDateTime) -> Dt {
    let tz = fuso();
    match tz.from_local_datetime(&naive).earliest() {
        Some(dt) => dt.fixed_offset(),
        // horário que não existe (pulo do horário de verão): mesmo relógio, offset de antes
        None => {
            use chrono::Offset;
            let antes = tz.from_local_datetime(&(naive - chrono::Duration::hours(2))).earliest();
            let off = antes.map(|d| d.offset().fix()).unwrap_or(FixedOffset::west_opt(3 * 3600).unwrap());
            off.from_local_datetime(&naive).single().unwrap()
        }
    }
}

/// datetime.fromisoformat: devolve o horário de parede e o offset (se tinha)
pub fn parse_iso(s: &str) -> R<(NaiveDateTime, Option<FixedOffset>)> {
    let erro = || Erro::Valor(format!("Invalid isoformat string: {}", repr_str(s)));
    let cs: Vec<char> = s.chars().collect();
    if cs.len() < 10 {
        return Err(erro());
    }
    let data: String = cs[..10].iter().collect();
    let d = NaiveDate::parse_from_str(&data, "%Y-%m-%d").map_err(|_| erro())?;
    if !data.as_bytes()[..4].iter().all(|b| b.is_ascii_digit()) {
        return Err(erro());
    }
    if cs.len() == 10 {
        return Ok((d.and_hms_opt(0, 0, 0).unwrap(), None));
    }
    // separador: qualquer caractere único (o Python 3.11+ aceita)
    let resto: String = cs[11..].iter().collect();
    let (hora, tz) = match resto.find(['+', '-', 'Z']) {
        Some(i) => (&resto[..i], Some(&resto[i..])),
        None => (resto.as_str(), None),
    };
    let t = parse_hora(hora).ok_or_else(erro)?;
    let off = match tz {
        None => None,
        Some("Z") => Some(FixedOffset::east_opt(0).unwrap()),
        Some(z) => {
            let sinal = if z.starts_with('-') { -1 } else { 1 };
            let z = &z[1..];
            let z: String = z.chars().filter(|c| *c != ':').collect();
            if !(z.len() == 4 || z.len() == 6) || !z.bytes().all(|b| b.is_ascii_digit()) {
                return Err(erro());
            }
            let h: i32 = z[..2].parse().unwrap();
            let m: i32 = z[2..4].parse().unwrap();
            let sec: i32 = if z.len() == 6 { z[4..6].parse().unwrap() } else { 0 };
            if h > 23 || m > 59 {
                return Err(erro());
            }
            Some(FixedOffset::east_opt(sinal * (h * 3600 + m * 60 + sec)).ok_or_else(erro)?)
        }
    };
    Ok((d.and_time(t), off))
}

fn parse_hora(h: &str) -> Option<NaiveTime> {
    let (hms, frac) = match h.split_once(['.', ',']) {
        Some((a, b)) => (a, Some(b)),
        None => (h, None),
    };
    let partes: Vec<&str> = if hms.contains(':') {
        hms.split(':').collect()
    } else {
        // formato básico: HH, HHMM, HHMMSS
        (0..hms.len()).step_by(2).map(|i| hms.get(i..i + 2).unwrap_or("x")).collect()
    };
    if partes.is_empty() || partes.len() > 3 || partes.iter().any(|p| p.len() != 2 || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let n: Vec<u32> = partes.iter().map(|p| p.parse().unwrap()).collect();
    let micro = match frac {
        None => 0,
        Some(f) if !f.is_empty() && f.len() <= 9 && f.bytes().all(|b| b.is_ascii_digit()) && n.len() == 3 => {
            let f6: String = format!("{:0<6}", f).chars().take(6).collect();
            f6.parse().ok()?
        }
        Some(_) => return None,
    };
    NaiveTime::from_hms_micro_opt(n[0], *n.get(1).unwrap_or(&0), *n.get(2).unwrap_or(&0), micro)
}

/// datetime.fromisoformat pra valores com fuso (sem fuso → o da instância)
pub fn from_iso(s: &str) -> R<Dt> {
    let (naive, off) = parse_iso(s)?;
    Ok(match off {
        Some(o) => o.from_local_datetime(&naive).single().unwrap(),
        None => no_fuso(naive),
    })
}

/// dt.isoformat() do Python: microssegundos só se != 0, offset ±HH:MM
pub fn iso(dt: &Dt) -> String {
    let mut s = dt.format("%Y-%m-%dT%H:%M:%S").to_string();
    let micro = dt.nanosecond() / 1000;
    if micro != 0 {
        s.push_str(&format!(".{:06}", micro));
    }
    let off = dt.offset().local_minus_utc();
    let (sinal, a) = if off < 0 { ('-', -off) } else { ('+', off) };
    s.push_str(&format!("{sinal}{:02}:{:02}", a / 3600, a % 3600 / 60));
    if a % 60 != 0 {
        s.push_str(&format!(":{:02}", a % 60));
    }
    s
}

pub const WEEKDAYS: [&str; 7] = ["segunda", "terça", "quarta", "quinta", "sexta", "sábado", "domingo"];

pub fn dia_semana<T: Datelike>(d: &T) -> &'static str {
    WEEKDAYS[d.weekday().num_days_from_monday() as usize]
}

/// re.fullmatch(r'\d{4}-\d{2}-\d{2}', s)
pub fn so_data(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

// ==========================================
// URL E JSON
// ==========================================

fn pct(s: &str, seguro: &[u8], espaco_mais: bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"_.-~".contains(&b) || seguro.contains(&b) {
            out.push(b as char);
        } else if b == b' ' && espaco_mais {
            out.push('+');
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// urllib.parse.quote (safe='/')
pub fn quote(s: &str) -> String {
    pct(s, b"/", false)
}

/// urllib.parse.unquote_plus: `+` vira espaço e `%XX` volta ao byte (o retorno do login do Google)
pub fn unquote_plus(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(x)) => {
                out.push(x);
                i += 3;
                continue;
            }
            (b'+', _) => out.push(b' '),
            (c, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// SHA-256 (FIPS 180-4), só pro PKCE do login: não vale uma crate
pub fn sha256(dados: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = dados.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((dados.len() as u64) * 8).to_be_bytes());
    for bloco in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([bloco[4 * i], bloco[4 * i + 1], bloco[4 * i + 2], bloco[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let t1 = hh.wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g)).wrapping_add(K[i]).wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22)).wrapping_add((a & b) ^ (a & c) ^ (b & c));
            (hh, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// urllib.parse.urlencode (quote_plus)
pub fn urlencode(campos: &[(&str, String)]) -> String {
    campos.iter().map(|(k, v)| format!("{}={}", pct(k, b"", true), pct(v, b"", true))).collect::<Vec<_>>().join("&")
}

/// json.dumps(x, ensure_ascii=False): separadores ', ' e ': ' como no Python
pub fn dumps<T: Serialize>(v: &T) -> String {
    struct Py;
    impl serde_json::ser::Formatter for Py {
        fn begin_array_value<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
            if first { Ok(()) } else { w.write_all(b", ") }
        }
        fn begin_object_key<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> {
            if first { Ok(()) } else { w.write_all(b", ") }
        }
        fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> {
            w.write_all(b": ")
        }
    }
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, Py);
    v.serialize(&mut ser).expect("serializar JSON");
    String::from_utf8(buf).expect("JSON é UTF-8")
}

/// truthiness do Python pra valores JSON
pub fn verdade(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// args['chave'] com KeyError
pub fn arg<'a>(args: &'a Value, k: &str) -> R<&'a Value> {
    args.get(k).ok_or_else(|| Erro::Chave(repr_str(k)))
}

pub fn arg_str(args: &Value, k: &str) -> R<String> {
    match arg(args, k)? {
        Value::String(s) => Ok(s.clone()),
        v => Err(Erro::Chave(format!("{k}: esperava texto, veio {v}"))),
    }
}

pub fn arg_opt_str(args: &Value, k: &str) -> R<Option<String>> {
    match arg(args, k)? {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        v => Err(Erro::Chave(format!("{k}: esperava texto, veio {v}"))),
    }
}

// ==========================================
// MEMÓRIA
// ==========================================

extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}

/// devolve pro sistema a memória livre do heap (glibc) — chamado depois de fotos/agenda
pub fn devolve_memoria() {
    // SAFETY: malloc_trim da glibc não tem pré-condição
    unsafe {
        malloc_trim(0);
    }
}

/// linha VmHWM / VmRSS do /proc/self/status
pub fn memoria() -> String {
    let st = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    st.lines().filter(|l| l.starts_with("VmHWM") || l.starts_with("VmRSS")).map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join(" · ")
}

/// pasta temporária única (testes e --selftest), apagada no drop
pub struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub fn new(prefixo: &str) -> TempDir {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
        let p = std::env::temp_dir().join(format!("{prefixo}{}.{}.{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst), nanos));
        std::fs::create_dir_all(&p).expect("criar pasta temporária");
        TempDir(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formato_g_igual_python() {
        for (x, s) in [(3810.0, "3810"), (1922.5, "1922.5"), (0.0001, "0.0001"), (0.00001, "1e-05"), (1234567.0, "1.23457e+06"),
                       (123456.0, "123456"), (-500.0, "-500"), (2.5e-5, "2.5e-05"), (999999.5, "1e+06"), (3786.5, "3786.5")] {
            assert_eq!(fmt_g(x), s, "{x}");
        }
        assert_eq!(fmt_g_sinal(200.0), "+200");
        assert_eq!(fmt_g_sinal(-10.0), "-10");
    }

    #[test]
    fn formato_f_e_pct() {
        assert_eq!(fmt_f(0.125, 2), "0.12"); // metade-pro-par igual ao Python
        assert_eq!(fmt_f(0.375, 2), "0.38");
        assert_eq!(fmt_f(2.5, 0), "2");
        assert_eq!(fmt_f(-0.4, 0), "-0");
        assert_eq!(fmt_pct_sinal(-0.004), "-0%");
        assert_eq!(fmt_pct_sinal(0.1234), "+12%");
        assert_eq!(fmt_pct_sinal(0.0), "+0%");
        assert_eq!(py_round(2.675, 2), 2.67);
    }

    #[test]
    fn repr_float() {
        for (x, s) in [(3786.0, "3786.0"), (3786.5, "3786.5"), (1e16, "1e+16"), (1.5e-5, "1.5e-05"), (0.0001, "0.0001"), (123.456, "123.456"), (1e15, "1000000000000000.0")] {
            assert_eq!(repr_f64(x), s);
        }
    }

    #[test]
    fn iso_ida_e_volta() {
        let dt = from_iso("2026-10-03T15:00").unwrap();
        assert_eq!(iso(&dt), "2026-10-03T15:00:00-03:00");
        let dt = from_iso("2026-10-02T21:00:00.123400-03:00").unwrap();
        assert_eq!(iso(&dt), "2026-10-02T21:00:00.123400-03:00");
        assert!(from_iso("amanhã").is_err());
        assert!(from_iso("2026-10-03 15:00").is_ok());
    }

    #[test]
    fn floats_do_python() {
        assert_eq!(py_float(" 1_000.5 ").unwrap(), 1000.5);
        assert!(py_float("1.2.3").is_err());
        assert_eq!(py_float("1.2.3").unwrap_err().to_string(), "could not convert string to float: '1.2.3'");
        assert_eq!(py_int("2").unwrap(), 2);
        assert!(py_int("2.0").is_err());
    }

    #[test]
    fn dumps_estilo_python() {
        let v = serde_json::json!({"ok": true, "odometro_usado": 3786.0, "git": "push ok", "l": [1, 2]});
        assert_eq!(dumps(&v), r#"{"ok": true, "odometro_usado": 3786.0, "git": "push ok", "l": [1, 2]}"#);
    }
}

/// HTML do Telegram → texto puro (pro histórico da conversa)
pub fn tira_tags(html: &str) -> String {
    static T: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let t = T.get_or_init(|| regex::Regex::new("<[^>]+>").unwrap());
    t.replace_all(html, "").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// grava credencial só legível pelo dono (0600), trocando o que tinha
pub fn grava_secreto(path: &std::path::Path, conteudo: &str) -> R<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let erro = |e: std::io::Error| Erro::Msg(format!("{}: {e}", path.display()));
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path).map_err(erro)?;
    f.write_all(conteudo.as_bytes()).map_err(erro)
}

/// código de pareamento de 6 dígitos (/dev/urandom)
pub fn codigo_pareamento() -> R<String> {
    use std::io::Read;
    let mut b = [0u8; 4];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(format!("{:06}", u32::from_le_bytes(b) % 1_000_000))
}
