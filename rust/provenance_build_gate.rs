//! Native Cargo counterpart of settings_r4_gate.py, using its single shared contract.
use std::{collections::BTreeMap,env,fs,path::Path};
use serde_json::Value;
fn text<'a>(v:&'a Value,key:&str)->Result<&'a str,String>{v[key].as_str().ok_or_else(||format!("invalid source gate contract: {key}"))}
fn files(dir:&Path,out:&mut Vec<std::path::PathBuf>)->Result<(),String>{
    for entry in fs::read_dir(dir).map_err(|_|"cannot read source gate directory")?{
        let path=entry.map_err(|_|"cannot read source gate entry")?.path();
        if path.is_dir(){files(&path,out)?;}else if path.extension().is_some_and(|e|e=="rs"){out.push(path);}
    }Ok(())
}
fn function_name(line:&str)->Option<String>{
    let (_,tail)=line.split_once("fn ")?;
    let name=tail.split(['(','<',' ']).next()?;
    if name.is_empty(){None}else{Some(name.into())}
}
pub fn check(root:&Path)->Result<(),String>{
    println!("cargo:rerun-if-env-changed=S1_R4_MUTATION");
    println!("cargo:rerun-if-changed={}",root.join("script/rust/settings_r4_contract.json").display());
    println!("cargo:rerun-if-changed={}",root.join("rust/src").display());
    if env::var_os("S1_R4_MUTATION").is_some(){
        if env::var("PROFILE").ok().as_deref()!=Some("debug"){return Err("mutation mode is forbidden in a release build".into());}
        println!("cargo:warning=R4 debug runtime-mutation proof: structural rejection is proved independently");return Ok(());
    }
    let body=fs::read_to_string(root.join("script/rust/settings_r4_contract.json")).map_err(|_|"source gate contract missing")?;
    let contract:Value=serde_json::from_str(&body).map_err(|_|"source gate contract malformed")?;
    let exemptions=contract["exemptions"].as_array().ok_or("invalid exemptions")?;
    if exemptions.len()!=1 || text(&exemptions[0],"reason")?!="market data, not account state; RULING-R4A"{return Err("exactly one historical-price exemption is required".into());}
    let constructors=contract["constructors"].as_array().ok_or("invalid constructors")?;
    let tokens=contract["read_tokens"].as_array().ok_or("invalid read tokens")?;
    let required=contract["required"].as_object().ok_or("invalid consumer contracts")?;
    let mut paths=vec![];files(&root.join("rust/src"),&mut paths)?;
    let mut sources=BTreeMap::new();
    for path in paths{
        let name=path.strip_prefix(root.join("rust/src")).map_err(|_|"invalid source path")?.components().map(|c|c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        let body=fs::read_to_string(path).map_err(|_|"cannot read provenance source")?;
        let source=body.split("\n#[cfg(test)]").next().ok_or("empty source")?;
        let mut function=String::new();let mut awaited=false;
        for line in source.lines(){
            let line=line.trim();if line.starts_with("//"){continue;}
            if let Some(name)=function_name(line){function=name;awaited=false;}
            for token in tokens{
                let token=token.as_str().ok_or("invalid digest-read token")?;
                let definition=format!("fn {token}");
                for (offset,_) in line.match_indices(token){
                    if offset>=3 && line[..offset].ends_with("fn "){continue;}
                    if line.contains(&definition)&&line.matches(token).count()==1{continue;}
                    let comparison=name==text(&contract["comparison"],"file")?&&function==text(&contract["comparison"],"function")?;
                    if comparison{continue;}
                    let constructed=constructors.iter().any(|c|c["file"]==name&&c["function"]==function);
                    if !constructed{return Err(format!("{name}:{function}: digest read outside handle construction/current_for"));}
                    if awaited{return Err(format!("{name}:{function}: producer read after I/O"));}
                }
            }
            if line.contains(".await"){awaited=true;}
        }
        if name!="web/settings/validator.rs" && source.contains("validator::check("){return Err(format!("{name}: raw credential validation bypasses immutable handle"));}
        if contract["wait_consumers"].as_array().ok_or("invalid wait consumers")?.iter().any(|v|v.as_str()==Some(&name)) {
            let compact:String=source.lines().filter(|line|!line.trim_start().starts_with("//")).collect::<String>().chars().filter(|ch|!ch.is_whitespace()).collect();
            if ["else{true","else{returntrue","else{Ok(true","_=>true","_=>Ok(true"].iter().any(|pattern|compact.contains(pattern)) {
                return Err(format!("{name}: permissive unknown wait provenance default"));
            }
        }
        sources.insert(name,body);
    }
    for (name,anchors) in required{
        let owned;
        let source=if name=="build.rs"{owned=fs::read_to_string(root.join("rust/build.rs")).map_err(|_|"cannot read build gate caller")?;&owned}else{sources.get(name).ok_or_else(||format!("missing consumer source: {name}"))?};
        for anchor in anchors.as_array().ok_or("invalid consumer anchors")?{
            if !source.contains(anchor.as_str().ok_or("invalid consumer anchor")?){return Err(format!("{name}: missing structural provenance contract"));}
        }
    }
    println!("cargo:warning=R4 provenance gate passed; market data, not account state; RULING-R4A");Ok(())
}
