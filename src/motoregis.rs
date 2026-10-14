//! Os pedaços do motoregis.py que o bot usa, reimplementados com a mesma saída (CSV byte a byte
//! e o mesmo texto de relatório). Fonte da verdade continua sendo o MotorCostTracker/motoregis.py:
//! se ele mudar, este arquivo tem que acompanhar (o teste `relatorio_igual_ao_python` avisa).
use crate::util::{col, fmt_f, fmt_g, fmt_pct_sinal, num, parse_ddmmyy, py_float, py_int, str_num, Erro, R};
use chrono::{Datelike, NaiveDate};
use serde_json::Value;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

pub type Linhas = Vec<Vec<String>>;

pub struct Motoregis {
    /// DATA_DIR do motoregis (pasta do MotorCostTracker)
    pub dir: PathBuf,
}

/// configs.json do MotorCostTracker
pub struct MotorConfig {
    pub veiculos_index: Vec<(String, String)>,
    pub combustiveis: Vec<String>,
    pub raw: Value,
}

impl MotorConfig {
    pub fn json_de(&self, veiculo: &str) -> R<&str> {
        self.veiculos_index
            .iter()
            .find(|(n, _)| n == veiculo)
            .map(|(_, j)| j.as_str())
            .ok_or_else(|| Erro::Chave(crate::util::repr_str(veiculo)))
    }
}

/// uma linha do csv.writer (QUOTE_MINIMAL), sem o fim de linha
pub fn csv_line(row: &[String]) -> String {
    row.iter()
        .map(|c| {
            if c.contains([',', '"', '\r', '\n']) || (row.len() == 1 && c.is_empty()) {
                format!("\"{}\"", c.replace('"', "\"\""))
            } else {
                c.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// json_path.replace('.json', '.csv')
pub fn csv_de(json_path: &str) -> String {
    json_path.replace(".json", ".csv")
}

/// months_between: meses completos — 27/04 → 01/10 são 5, não 6
pub fn months_between(start: NaiveDate, end: NaiveDate) -> i32 {
    let months = (end.year() - start.year()) * 12 + (end.month() as i32 - start.month() as i32);
    if end.day() < start.day() { months - 1 } else { months }
}

impl Motoregis {
    pub fn new(dir: impl AsRef<Path>) -> Motoregis {
        Motoregis { dir: dir.as_ref().to_path_buf() }
    }

    pub fn caminho(&self, p: &str) -> PathBuf {
        self.dir.join(p)
    }

    // ==========================================
    // ZONE 1: DATA ENGINES
    // ==========================================

    /// None se não existe ou não é JSON válido (igual ao load_json)
    pub fn load_json(&self, p: &str) -> R<Option<Value>> {
        match fs::read(self.caminho(p)) {
            Ok(b) => Ok(serde_json::from_slice(&b).ok()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn read_csv(&self, p: &str) -> R<Linhas> {
        let texto = match fs::read_to_string(self.caminho(p)) {
            Ok(t) => t,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        // modo texto do Python: \r\n e \r viram fim de linha
        let texto = texto.replace("\r\n", "\n").replace('\r', "\n");
        Ok(texto
            .split_inclusive('\n')
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| l.split(',').map(String::from).collect())
            .collect())
    }

    /// csv.writer(f).writerow(row): QUOTE_MINIMAL e fim de linha \r\n
    pub fn append_record(&self, p: &str, row: &[String]) -> R<()> {
        let mut linha = csv_line(row);
        linha.push_str("\r\n");
        let mut f = fs::OpenOptions::new().append(true).create(true).open(self.caminho(p))?;
        f.write_all(linha.as_bytes())?;
        Ok(())
    }

    // ==========================================
    // ZONE 3: ORCHESTRATION (só o que o bot usa)
    // ==========================================

    pub fn load_config(&self) -> R<MotorConfig> {
        let raw = self.load_json("configs.json")?.ok_or_else(|| Erro::Msg("❌ configs.json não encontrado.".into()))?;
        let idx = raw.get("veiculos_index").and_then(Value::as_object).ok_or_else(|| Erro::Chave("'veiculos_index'".into()))?;
        let veiculos_index = idx.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect();
        let combustiveis = raw
            .get("combustiveis")
            .and_then(Value::as_array)
            .ok_or_else(|| Erro::Chave("'combustiveis'".into()))?
            .iter()
            .map(|v| str_num(v))
            .collect();
        Ok(MotorConfig { veiculos_index, combustiveis, raw })
    }

    pub fn get_last_odometer(&self, p: &str) -> R<f64> {
        let data = self.read_csv(p)?;
        if data.len() <= 1 {
            return Ok(0.0);
        }
        Ok(col(data.last().unwrap(), 1).and_then(py_float).unwrap_or(0.0))
    }

    /// (odômetro, litros, data, último foi cheio)
    pub fn get_last_fuel_record(&self, p: &str) -> R<(Option<f64>, Option<f64>, Option<String>, bool)> {
        let data = self.read_csv(p)?;
        for row in data.iter().skip(1).rev() {
            let r: R<_> = (|| {
                if col(row, 2)? == "1" && py_float(col(row, 4)?)? > 0.0 {
                    let was_full = !row.get(5).map(|s| s.as_str()).unwrap_or("").contains("|parcial");
                    return Ok(Some((py_float(&row[1])?, py_float(&row[4])?, row[0].clone(), was_full)));
                }
                Ok(None)
            })();
            if let Ok(Some((o, l, d, f))) = r {
                return Ok((Some(o), Some(l), Some(d), f));
            }
        }
        Ok((None, None, None, true))
    }

    pub fn full_tank_km_history(&self, p: &str) -> R<Vec<f64>> {
        let data = self.read_csv(p)?;
        let fuel: Vec<&Vec<String>> = data.iter().skip(1).filter(|r| r.len() > 5 && r[2] == "1").collect();
        let mut deltas = vec![];
        for i in 1..fuel.len() {
            let (curr, prev) = (fuel[i], fuel[i - 1]);
            if curr[5].contains("|cheio") && prev[5].contains("|cheio") && !curr[5].contains("gap") {
                if let (Ok(a), Ok(b)) = (py_float(&curr[1]), py_float(&prev[1])) {
                    if a - b > 0.0 {
                        deltas.push(a - b);
                    }
                }
            }
        }
        Ok(deltas)
    }

    /// check_maintenance: devolve o que o Python imprimiria
    pub fn check_maintenance(&self, csv_path: &str, json_path: &str, current_odometer: f64, current_date: &str) -> R<String> {
        let mut out = String::new();
        let Some(vehicle) = self.load_json(json_path)? else { return Ok(out) };
        let data = self.read_csv(csv_path)?;
        if data.len() <= 1 {
            return Ok(out);
        }
        let maintenance = vehicle.get("manutencoes").cloned().unwrap_or(Value::Object(Default::default()));
        let current = parse_ddmmyy(current_date)?;
        let first_date = parse_ddmmyy(col(&data[1], 0)?)?;
        let first_odometer = py_float(col(&data[1], 1)?)?;
        let mut alerts: Vec<(String, Vec<String>)> = vec![];
        let mut alerta = |nome: String, info: String| match alerts.iter_mut().find(|(n, _)| *n == nome) {
            Some((_, v)) => v.push(info),
            None => alerts.push((nome, vec![info])),
        };

        for (name, interval) in itens(&maintenance, "por_quilometragem") {
            let ref_odo = match last_maintenance(&data, name)? {
                Some(l) => py_float(col(l, 1)?)?,
                None => first_odometer,
            };
            let km_since = current_odometer - ref_odo;
            if km_since >= num(interval)? {
                alerta(display_name(name, Some(&vehicle)), format!("{} km atrás (a cada {} km)", fmt_g(km_since), str_num(interval)));
            }
        }
        for (name, months) in itens(&maintenance, "por_tempo_meses") {
            let ref_date = ref_data(&data, name, first_date)?;
            let delta = months_between(ref_date, current);
            if delta as f64 >= num(months)? {
                alerta(display_name(name, Some(&vehicle)), format!("{} meses atrás (a cada {} meses)", delta, str_num(months)));
            }
        }
        for (name, days) in itens(&maintenance, "por_tempo_dias") {
            let ref_date = ref_data(&data, name, first_date)?;
            let delta = (current - ref_date).num_days();
            if delta as f64 >= num(days)? {
                alerta(display_name(name, Some(&vehicle)), format!("{} dias atrás (a cada {} dias)", delta, str_num(days)));
            }
        }

        if !alerts.is_empty() {
            out.push_str("\n--- Manutenções pendentes ---\n\n");
            for (nome, infos) in &alerts {
                out.push_str(&format!("  ⚠️  {nome}\n"));
                for info in infos {
                    out.push_str(&format!("      {info}\n"));
                }
            }
            out.push('\n');
        }
        Ok(out)
    }

    /// show_cost_analysis: devolve o que o Python imprimiria
    pub fn show_cost_analysis(&self, csv_path: &str, vehicle_name: &str, config: &MotorConfig) -> R<String> {
        let mut out = String::new();
        let mut p = |s: &str| {
            out.push_str(s);
            out.push('\n');
        };
        let data = self.read_csv(csv_path)?;
        if data.len() <= 1 {
            p("  Sem registros suficientes.");
            return Ok(out);
        }
        let records = &data[1..];
        let sym = config.raw.get("currency_symbol").map(str_num).unwrap_or_else(|| "R$".into());
        let json_path = config.json_de(vehicle_name)?;
        let vehicle = self.load_json(json_path)?.unwrap_or(Value::Object(Default::default()));
        let revisao_km = vehicle.get("intervalo_revisao_km").cloned().unwrap_or(Value::Null);

        let odos: R<(f64, f64)> = (|| Ok((py_float(col(&records[0], 1)?)?, py_float(col(records.last().unwrap(), 1)?)?)))();
        let Ok((first_odo, last_odo)) = odos else {
            p("  Erro ao ler odômetros.");
            return Ok(out);
        };
        let total_km = last_odo - first_odo;
        if total_km <= 0.0 {
            p("  Km total insuficiente.");
            return Ok(out);
        }

        let (mut fuel_cost, mut maint_cost) = (0.0, 0.0);
        for row in records {
            let r: R<(f64, i64)> = (|| Ok((py_float(col(row, 3)?)?, py_int(col(row, 2)?)?)))();
            if let Ok((cost, cat)) = r {
                if cat == 1 {
                    fuel_cost += cost;
                } else if cat == 2 || cat == 4 {
                    maint_cost += cost;
                }
            }
        }

        let fuel_avg = fuel_cost / total_km * 10.0;
        let maint_denom = if crate::util::verdade(&revisao_km) { num(&revisao_km)? } else { total_km };
        let maint_per_10 = maint_cost / maint_denom * 10.0;

        let fuel: Vec<&Vec<String>> = records.iter().filter(|r| r.len() > 5 && r[2] == "1").collect();
        let par = |c: &Vec<String>, pr: &Vec<String>| c[5].contains("|cheio") && pr[5].contains("|cheio") && !c[5].contains("gap");

        // km/L, km e dias médios entre abastecimentos: pares cheio-a-cheio
        let (mut total_km_pairs, mut total_liters_pairs, mut total_days_pairs, mut pair_count) = (0.0, 0.0, 0i64, 0i64);
        for i in 1..fuel.len() {
            let (curr, prev) = (fuel[i], fuel[i - 1]);
            if par(curr, prev) {
                let _ = (|| -> R<()> {
                    let km = py_float(&curr[1])? - py_float(&prev[1])?;
                    let liters = py_float(&curr[4])?;
                    if km > 0.0 && liters > 0.0 {
                        total_km_pairs += km;
                        total_liters_pairs += liters;
                        let days = (parse_ddmmyy(&curr[0])? - parse_ddmmyy(&prev[0])?).num_days();
                        total_days_pairs += days;
                        pair_count += 1;
                    }
                    Ok(())
                })();
            }
        }
        let fuel_kml_avg = (total_liters_pairs > 0.0).then(|| total_km_pairs / total_liters_pairs);
        let avg_km_per_pair = (pair_count > 0).then(|| total_km_pairs / pair_count as f64);
        let avg_days_per_pair = (pair_count > 0).then(|| total_days_pairs as f64 / pair_count as f64);

        let (mut last_fuel_per_10, mut last_kml, mut last_km, mut last_days, mut last_tank_cost) = (None, None, None, None::<i64>, None);
        for i in (1..fuel.len()).rev() {
            let (curr, prev) = (fuel[i], fuel[i - 1]);
            if par(curr, prev) {
                let _ = (|| -> R<()> {
                    let km = py_float(&curr[1])? - py_float(&prev[1])?;
                    let liters = py_float(&curr[4])?;
                    let cost = py_float(&curr[3])?;
                    if km > 0.0 && liters > 0.0 {
                        last_fuel_per_10 = Some(cost / km * 10.0);
                        last_kml = Some(km / liters);
                        last_km = Some(km);
                        last_days = Some((parse_ddmmyy(&curr[0])? - parse_ddmmyy(&prev[0])?).num_days());
                        last_tank_cost = Some(cost);
                    }
                    Ok(())
                })();
                break;
            }
        }

        let first_date = parse_ddmmyy(col(&records[0], 0)?)?;
        let last_date = parse_ddmmyy(col(records.last().unwrap(), 0)?)?;
        let total_days = (last_date - first_date).num_days();
        let amortized_maint = maint_per_10 / 10.0 * total_km;
        let cost_per_day = (total_days > 0).then(|| (fuel_cost + amortized_maint) / total_days as f64);
        let cost_per_day_last = match (last_tank_cost, last_days) {
            (Some(c), Some(d)) if d != 0 => Some(c / d as f64),
            _ => None,
        };

        let row = |p: &mut dyn FnMut(&str), dt: &str, at: &str, label: &str, ds: String, alt: String, last: Option<f64>, geral: Option<f64>| {
            let mut line = format!("  ({dt}){label} {ds}");
            if let (Some(l), Some(g)) = (last, geral) {
                let dev = if g != 0.0 { (l - g) / g } else { 0.0 };
                line += &format!(" ({}", fmt_pct_sinal(dev));
                if dev.abs() > 0.10 {
                    line += &format!(" - ({at}){alt}");
                }
                line += ")";
            }
            p(&line);
        };
        let ou = |v: Option<f64>, f: &dyn Fn(f64) -> String| v.map(f).unwrap_or_default();

        p("\n--- Resumo ---");
        p(&format!("  {} → {}  ({} km)\n", records[0][0], records.last().unwrap()[0], fmt_g(total_km)));
        if let Some(k) = last_kml {
            row(&mut p, "u", "g", "Km/L:", format!("{} km/L", fmt_f(k, 2)), ou(fuel_kml_avg, &|v| format!("{} km/L", fmt_f(v, 2))), Some(k), fuel_kml_avg);
        }
        if let Some(d) = last_days {
            row(&mut p, "u", "g", "Dias rodados:", format!("{d} dias"), ou(avg_days_per_pair, &|v| format!("{} dias", fmt_f(v, 1))), Some(d as f64), avg_days_per_pair);
        }
        if let Some(k) = last_km {
            row(&mut p, "u", "g", "Km rodados:", format!("{} km", fmt_g(k)), ou(avg_km_per_pair, &|v| format!("{} km", fmt_f(v, 0))), Some(k), avg_km_per_pair);
        }
        p("");
        if let Some(c) = cost_per_day {
            row(&mut p, "g", "u", "Custo/dia:", format!("{sym}{}/dia", fmt_f(c, 2)), ou(cost_per_day_last, &|v| format!("{sym}{}/dia", fmt_f(v, 2))), cost_per_day_last, Some(c));
        }
        row(&mut p, "g", "u", "Custo/10km:", format!("{sym}{}/10km", fmt_f(fuel_avg, 2)), ou(last_fuel_per_10, &|v| format!("{sym}{}/10km", fmt_f(v, 2))), last_fuel_per_10, Some(fuel_avg));
        Ok(out)
    }
}

/// .items() de uma categoria de manutenção (vazio se não existe)
fn itens<'a>(maintenance: &'a Value, cat: &str) -> Vec<(&'a str, &'a Value)> {
    maintenance.get(cat).and_then(Value::as_object).map(|o| o.iter().map(|(k, v)| (k.as_str(), v)).collect()).unwrap_or_default()
}

/// última linha categoria 2 com esse item (de trás pra frente)
fn last_maintenance<'a>(data: &'a Linhas, name: &str) -> R<Option<&'a Vec<String>>> {
    for row in data.iter().skip(1).rev() {
        if col(row, 2)? == "2" && col(row, 5)? == name {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

fn ref_data(data: &Linhas, name: &str, first: NaiveDate) -> R<NaiveDate> {
    match last_maintenance(data, name)? {
        Some(l) => parse_ddmmyy(col(l, 0)?),
        None => Ok(first),
    }
}

/// _compute_urgency: maior razão (feito desde a última vez / intervalo) entre km, dias e meses
pub fn compute_urgency(name: &str, vehicle: &Value, data: &Linhas, current_odometer: f64, current: NaiveDate) -> R<f64> {
    let empty = Value::Object(Default::default());
    let maintenance = vehicle.get("manutencoes").unwrap_or(&empty);
    let first_date = if data.len() > 1 { parse_ddmmyy(col(&data[1], 0)?)? } else { current };
    let first_odometer = if data.len() > 1 { py_float(col(&data[1], 1)?)? } else { current_odometer };
    let last = last_maintenance(data, name)?;
    let mut ratios = vec![];
    let em = |cat: &str| maintenance.get(cat).and_then(|c| c.get(name));

    if let Some(interval) = em("por_quilometragem") {
        let interval = num(interval)?;
        let ref_odo = match last {
            Some(l) => py_float(col(l, 1)?)?,
            None => first_odometer,
        };
        ratios.push(if interval > 0.0 { (current_odometer - ref_odo) / interval } else { 0.0 });
    }
    let ref_date = || -> R<NaiveDate> {
        match last {
            Some(l) => parse_ddmmyy(col(l, 0)?),
            None => Ok(first_date),
        }
    };
    if let Some(days) = em("por_tempo_dias") {
        let days = num(days)?;
        let r = ref_date()?;
        ratios.push(if days > 0.0 { (current - r).num_days() as f64 / days } else { 0.0 });
    }
    if let Some(months) = em("por_tempo_meses") {
        let months = num(months)?;
        let r = ref_date()?;
        let delta = months_between(r, current);
        ratios.push(if months > 0.0 { delta as f64 / months } else { 0.0 });
    }
    Ok(ratios.into_iter().fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| if x > m { x } else { m }))).unwrap_or(0.0))
}

/// display_name: nomes_display do veículo ou 'chave_com_underscore' → 'Chave com underscore'
pub fn display_name(key: &str, vehicle: Option<&Value>) -> String {
    let padrao = || {
        let s = key.split('_').collect::<Vec<_>>().join(" ");
        let mut cs = s.chars();
        match cs.next() {
            Some(c) => c.to_uppercase().collect::<String>() + &cs.as_str().to_lowercase(),
            None => String::new(),
        }
    };
    match vehicle {
        Some(v) if crate::util::verdade(v) && v.get("nomes_display").is_some() => {
            v["nomes_display"].get(key).map(str_num).unwrap_or_else(padrao)
        }
        _ => padrao(),
    }
}

