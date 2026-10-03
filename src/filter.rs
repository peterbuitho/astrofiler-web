//! Search, type filter and sort order of the Images page (the same rules as
//! the desktop app).

use astrofiler::db::FitsFile;
use astrofiler::names;
use astrofiler::util::FrameKind;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Default, Clone)]
pub struct Filter {
    pub q: String,
    /// "all", "light", "calibration" or "stacked".
    pub kind: String,
    /// "object", "date", "type", "filter", "exposure" or "telescope".
    pub sort: String,
    pub desc: bool,
}

/// Indexes into `files` of the files that match, in display order.
pub fn apply(
    files: &[FitsFile],
    object_names: &BTreeMap<String, String>,
    f: &Filter,
) -> Vec<usize> {
    let q = f.q.to_lowercase();
    // Common names, looked up once per object rather than once per file.
    let objects: HashSet<&str> = files.iter().filter_map(|f| f.object.as_deref()).collect();
    let common: HashMap<&str, String> = objects
        .into_iter()
        .map(|o| (o, names::common_name(o, object_names).unwrap_or_default()))
        .collect();
    // Commas separate alternatives: "M 76, M 52 LP" shows files matching
    // either. A part starting with an object ("M 76", "m76", "NGC 7000
    // Ha") shows exactly that object; matching "m" and "76" as separate
    // words would also find every file with 76 in its time or temperature.
    let object_keys: HashSet<String> = common.keys().map(|o| names::key(o)).collect();
    let clauses: Vec<(Option<String>, Vec<&str>)> = q
        .split(',')
        .map(|part| {
            let mut terms: Vec<&str> = part.split_whitespace().collect();
            let mut object = None;
            for n in (1..=terms.len()).rev() {
                let k = names::key(&terms[..n].join(" "));
                if object_keys.contains(&k) {
                    object = Some(k);
                    terms.drain(..n);
                    break;
                }
            }
            (object, terms)
        })
        .filter(|(o, t)| o.is_some() || !t.is_empty())
        .collect();
    let mut idx: Vec<usize> = files
        .iter()
        .enumerate()
        .filter(|(_, file)| {
            let kind = FrameKind::classify(file.image_type.as_deref().unwrap_or(""));
            let type_ok = match f.kind.as_str() {
                "light" => kind == Some(FrameKind::Light) && !file.stacked,
                "calibration" => !matches!(kind, Some(FrameKind::Light) | None),
                "stacked" => file.stacked,
                _ => true,
            };
            type_ok && {
                let object = file.object.as_deref().unwrap_or("");
                let key = names::key(object);
                let hay = format!(
                    "{} {} {} {} {} {} {}",
                    object,
                    common.get(object).map(String::as_str).unwrap_or(""),
                    file.filter.as_deref().unwrap_or(""),
                    file.telescope.as_deref().unwrap_or(""),
                    file.instrument.as_deref().unwrap_or(""),
                    file.date.as_deref().unwrap_or(""),
                    file.name
                )
                .to_lowercase();
                clauses.is_empty()
                    || clauses.iter().any(|(o, terms)| {
                        o.as_ref().is_none_or(|k| *k == key)
                            && terms.iter().all(|t| hay.contains(t))
                    })
            }
        })
        .map(|(i, _)| i)
        .collect();
    let exp = |f: &FitsFile| {
        f.exptime
            .as_deref()
            .and_then(|e| e.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    idx.sort_by(|&a, &b| {
        let (x, y) = (&files[a], &files[b]);
        let o = match f.sort.as_str() {
            "date" => x.date.cmp(&y.date),
            "type" => x.image_type.cmp(&y.image_type).then(x.date.cmp(&y.date)),
            "filter" => x.filter.cmp(&y.filter).then(x.date.cmp(&y.date)),
            "exposure" => exp(x).total_cmp(&exp(y)),
            "telescope" => x.telescope.cmp(&y.telescope).then(x.date.cmp(&y.date)),
            _ => x.object.cmp(&y.object).then(x.date.cmp(&y.date)),
        };
        if f.desc {
            o.reverse()
        } else {
            o
        }
    });
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(object: &str, typ: &str, filter: &str, date: &str) -> FitsFile {
        FitsFile {
            id: format!("{object}-{date}"),
            name: format!("/repo/{object}/{date}.fits"),
            object: Some(object.into()),
            image_type: Some(typ.into()),
            filter: Some(filter.into()),
            date: Some(date.into()),
            ..Default::default()
        }
    }

    #[test]
    fn object_search_is_exact_and_commas_are_alternatives() {
        let files = vec![
            file("M 76", "LIGHT", "LP", "2026-09-01T22:00:00"),
            file("M 7", "LIGHT", "LP", "2026-09-02T22:00:00"),
            file("NGC 281", "LIGHT", "Ha", "2026-07-06T22:00:00"),
            file("Dark", "DARK", "", "2026-09-03T22:00:00"),
        ];
        let names = BTreeMap::new();
        let run = |q: &str, kind: &str| {
            let f = Filter {
                q: q.into(),
                kind: kind.into(),
                ..Default::default()
            };
            apply(&files, &names, &f)
        };
        // "m7" is the object M 7, not every file with a 7 in it.
        assert_eq!(run("m7", "all"), vec![1]);
        assert_eq!(run("M 76, ngc281 ha", "all"), vec![0, 2]);
        assert_eq!(run("barbell", "all"), vec![0]);
        assert_eq!(run("", "calibration"), vec![3]);
        assert_eq!(run("", "light").len(), 3);
    }
}
