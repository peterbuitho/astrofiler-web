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

/// Whether a comparison key looks like a catalogue number: M76, NGC7000, C36.
fn is_designation(key: &str) -> bool {
    const CATALOGUES: [&str; 14] = [
        "M", "NGC", "IC", "C", "SH2", "LDN", "LBN", "B", "ABELL", "VDB", "PGC", "UGC", "MEL", "CR",
    ];
    let digits = key.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    let prefix = &key[..key.len() - digits.len()];
    CATALOGUES.contains(&prefix)
        && digits.chars().next().is_some_and(|c| c.is_ascii_digit())
        && digits
            .trim_end_matches(|c: char| c.is_ascii_alphabetic())
            .chars()
            .all(|c| c.is_ascii_digit())
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
    // A mosaic is found by its own name as well as by each panel's.
    let object_keys: HashSet<String> = common
        .keys()
        .flat_map(|o| [names::key(o), names::key(&names::mosaic(o).0)])
        .collect();
    let clauses: Vec<(Option<String>, Vec<String>)> = q
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
            let mut terms: Vec<String> = terms.into_iter().map(String::from).collect();
            // A catalogue designation that is not in the repository ("C 36")
            // is looked for as one word ("c36"), not as loose words: "36"
            // alone is in many times and temperatures. A C8 telescope is
            // still found by its name.
            if object.is_none() {
                for n in 1..=terms.len().min(2) {
                    let k = names::key(&terms[..n].join(" "));
                    if is_designation(&k) {
                        terms.splice(..n, [k.to_lowercase()]);
                        break;
                    }
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
                let mosaic = names::key(&names::mosaic(object).0);
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
                        o.as_ref().is_none_or(|k| *k == key || *k == mosaic)
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
            file("HD 199479(1)", "LIGHT", "LP", "2026-09-09T22:00:00"),
            file("HD 199479(2)", "LIGHT", "LP", "2026-09-09T23:00:00"),
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
        // An object that is not catalogued finds nothing, not random files.
        assert!(run("c36", "all").is_empty());
        assert!(run("C 36", "all").is_empty());
        assert!(run("ngc 7000 ha", "all").is_empty());
        assert_eq!(run("", "calibration"), vec![3]);
        assert_eq!(run("", "light").len(), 5);
        // A mosaic's name finds all its panels; a panel's name just that one.
        assert_eq!(run("hd 199479", "all"), vec![4, 5]);
        assert_eq!(run("HD 199479(2)", "all"), vec![5]);
    }

    #[test]
    fn a_designation_that_is_no_object_is_still_a_word_to_find() {
        let mut c8 = file("M 31", "LIGHT", "L", "2026-09-01T22:00:00");
        c8.telescope = Some("C8".into());
        let files = vec![c8, file("M 33", "LIGHT", "L", "2026-09-08T22:00:00")];
        let run = |q: &str| {
            let f = Filter {
                q: q.into(),
                ..Default::default()
            };
            apply(&files, &BTreeMap::new(), &f)
        };
        assert_eq!(run("c8"), vec![0]);
        assert_eq!(run("C 8"), vec![0]);
        assert!(run("c 36").is_empty());
    }
}
