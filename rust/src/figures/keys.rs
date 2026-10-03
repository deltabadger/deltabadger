//! Bot::Composition::HoldingKeys: the key each holding of a composition bot is known by. A holding is one asset
//! (an id) or, for rows recorded before orders stored their asset, one symbol string. A key is the asset's symbol;
//! where two holdings would share one, every one of them takes a suffix (its asset id, or `?` for a string),
//! repeated until all keys differ.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Identity { Asset(i64), Text(String) }

/// `candidates` is `(identity, candidate key)` in the order the walk first met each holding.
pub fn call(candidates: &[(Identity, String)]) -> Vec<(Identity, String)> {
    let mut keys = candidates.to_vec();
    loop {
        // The clashes are read off one state of the keys and then all renamed, as Ruby's group_by reads them.
        let mut renamed = keys.clone();
        let mut clashed = false;
        for (at, (_, key)) in keys.iter().enumerate() {
            if keys.iter().position(|(_, other)| other == key) != Some(at) { continue; } // each key once, at its first owner
            let owners: Vec<usize> = keys.iter().enumerate().filter(|(_, (_, other))| other == key).map(|(i, _)| i).collect();
            if owners.len() < 2 { continue; }
            clashed = true;
            let mut unresolved: Vec<&String> = owners.iter().filter_map(|&i| match &keys[i].0 { Identity::Text(s) => Some(s), Identity::Asset(_) => None }).collect();
            unresolved.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            for &i in &owners {
                let tag = match &keys[i].0 {
                    Identity::Asset(id) => id.to_string(),
                    Identity::Text(_) if unresolved.len() == 1 => "?".to_string(),
                    Identity::Text(s) => format!("?{}", unresolved.iter().position(|u| *u == s).map_or(0, |p| p + 1)),
                };
                renamed[i].1 = format!("{key}#{tag}");
            }
        }
        if !clashed { return keys; }
        keys = renamed;
    }
}

/// `value.presence`: nil for a nil or blank string.
pub fn presence(value: Option<&str>) -> Option<&str> { value.filter(|s| !s.trim().is_empty()) }

/// The candidate key of an asset: its symbol, else its name, else `#<id>`.
pub fn candidate(id: i64, symbol: Option<&str>, name: Option<&str>) -> String {
    presence(symbol).or(presence(name)).map_or_else(|| format!("#{id}"), str::to_string)
}
