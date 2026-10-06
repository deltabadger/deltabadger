//! ActiveModel::Type::String casting for permitted scalar column values.
use serde_json::Value;
pub fn cast(value:&Value)->Option<String>{
    match value{
        Value::Null=>None,
        Value::Bool(true)=>Some("t".into()),
        Value::Bool(false)=>Some("f".into()),
        Value::String(s)=>Some(s.clone()),
        Value::Number(n)=>Some(if n.is_f64(){super::format::float_to_s(n.as_f64()?)}else{n.to_string()}),
        Value::Array(_)|Value::Object(_)=>None,
    }
}
#[cfg(test)]
mod tests{
    #[test]
    fn active_model_string_scalars(){
        use serde_json::json;
        for (v,want) in [(json!(true),Some("t")),(json!(false),Some("f")),(json!(123),Some("123")),(json!(1.0),Some("1.0")),(json!(1.25),Some("1.25")),(json!(null),None)]{assert_eq!(super::cast(&v).as_deref(),want);}
    }
}
