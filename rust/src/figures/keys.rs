//! Bot::Composition::HoldingKeys: the key each holding of a composition bot is known by. A holding is one asset
//! (an id) or, for rows recorded before orders stored their asset, one symbol string. A key is the asset's symbol;
//! where two holdings would share one, every one of them takes a suffix (its asset id, or `?` for a string),
//! repeated until all keys differ.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Identity { Asset(i64), Text(String) }

/// `candidates` is `(identity, candidate key)` in the order the walk first met each holding.
pub fn call(candidates: &[(Identity, String)]) -> Result<Vec<(Identity, String)>, super::num::NumError> {
    use std::collections::HashMap;
    use super::budget;
    budget::charge(candidates.len() as u64, 0)?;
    let mut keys = candidates.to_vec();
    loop {
        let mut groups: Vec<Vec<usize>> = vec![];
        let mut places = HashMap::new();
        for (i, (_, key)) in keys.iter().enumerate() {
            budget::charge(1, 0)?;
            let at = *places.entry(key.as_str()).or_insert_with(|| { groups.push(vec![]); groups.len() - 1 });
            groups[at].push(i);
        }
        let mut clashed = false;
        for owners in groups {
            budget::charge(1, 0)?;
            if owners.len() < 2 { continue; }
            clashed = true;
            let mut unresolved = vec![];
            for &i in &owners {
                budget::charge(1, 0)?;
                if let Identity::Text(s) = &keys[i].0 { unresolved.push(s.clone()); }
            }
            budget::charge((unresolved.len() as u64).saturating_mul(u64::from(unresolved.len().max(1).ilog2()) + 1), 0)?;
            unresolved.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            let ranks: HashMap<_, _> = unresolved.iter().enumerate().map(|(i, s)| (s, i + 1)).collect();
            for i in owners {
                budget::charge(1, 0)?;
                let tag = match &keys[i].0 {
                    Identity::Asset(id) => id.to_string(),
                    Identity::Text(_) if unresolved.len() == 1 => "?".to_string(),
                    Identity::Text(s) => format!("?{}", ranks[s]),
                };
                keys[i].1 = format!("{}#{tag}", keys[i].1);
            }
        }
        if !clashed { return Ok(keys); }
    }
}

/// `value.presence`: nil for a nil or blank string.
pub fn presence(value: Option<&str>) -> Option<&str> { value.filter(|s| !s.trim().is_empty()) }

/// The candidate key of an asset: its symbol, else its name, else `#<id>`.
pub fn candidate(id: i64, symbol: Option<&str>, name: Option<&str>) -> String {
    presence(symbol).or(presence(name)).map_or_else(|| format!("#{id}"), str::to_string)
}
