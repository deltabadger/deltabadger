use super::*;
use crate::figures::{json::J, totals::{GlobalPnl, History}};
use crate::web::{i18n::escape, format::{float_round, float_to_s}};

fn coordinate(n: f64) -> String {
    let text = format!("{:.2}",float_round(n,2));
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}
fn spark(history: &History) -> (String, String, f64, f64, f64, bool) {
    let scale = history.percent.iter().fold(0.1_f64,|max,n|max.max(n.abs()));
    let points: Vec<(f64,f64)> = history.percent.iter().enumerate().map(|(i,n)|(i as f64*(100.0/(history.percent.len()-1) as f64),(1.0-n/scale)*100.0)).collect();
    let secants:Vec<_> = points.windows(2).map(|p|(p[1].1-p[0].1)/(p[1].0-p[0].0)).collect();
    let mut slopes = vec![secants.first().copied().unwrap_or(0.0)];
    slopes.extend(secants.windows(2).map(|s|(s[0]+s[1])/2.0));
    slopes.push(secants.last().copied().unwrap_or(0.0));
    for (i,secant) in secants.iter().enumerate() {
        if *secant == 0.0 { slopes[i]=0.0;slopes[i+1]=0.0;continue; }
        let (alpha,beta)=(slopes[i]/secant,slopes[i+1]/secant);
        if alpha < 0.0 { slopes[i]=0.0; }
        if beta < 0.0 { slopes[i+1]=0.0; }
        if alpha.powi(2)+beta.powi(2)>9.0 {
            let tau=3.0/(alpha.powi(2)+beta.powi(2)).sqrt();
            slopes[i]=tau*alpha*secant;slopes[i+1]=tau*beta*secant;
        }
    }
    let first=points.first().copied().unwrap_or((0.0,100.0));
    let mut path=format!("M{},{} ",coordinate(first.0),coordinate(first.1));
    let segments=points.windows(2).enumerate().map(|(i,p)| {
        let third=(p[1].0-p[0].0)/3.0;
        format!("C{},{} {},{} {},{}",coordinate(p[0].0+third),coordinate(p[0].1+slopes[i]*third),coordinate(p[1].0-third),coordinate(p[1].1-slopes[i+1]*third),coordinate(p[1].0),coordinate(p[1].1))
    }).collect::<Vec<_>>();
    path.push_str(&segments.join(" "));
    let area=format!("{path} L100,100 L0,100 Z");
    (path,area,scale,float_round(points.last().map_or(100.0,|p|p.1)/2.0,3),float_round((history.days/30.0).min(1.0)*100.0,2),history.percent.last().is_none_or(|n|*n>=0.0))
}
pub(super) fn render(pnl: Option<&GlobalPnl>, history: Option<&History>, unavailable: bool, hidden: bool) -> Result<String, FiguresError> {
    if unavailable { return Ok(format!("<div id=\"global-pnl\">\n{NO_VALUE}\n</div>")); }
    let Some(pnl)=pnl else { return Ok("<div id=\"global-pnl\"></div>".into()); };
    let history=history.filter(|h|h.percent.len()>1);
    let mut attrs=String::new();let mut curve=String::new();
    if let Some(h)=history {
        let (path,area,scale,end,width,gain)=spark(h);
        let floats=|ns:&Vec<f64>|J::arr(ns,|n|J::Float(*n)).write();
        attrs=format!(" data-controller=\"pnl-spark\" data-pnl-spark-percent-value=\"{}\" data-pnl-spark-at-value=\"{}\" data-pnl-spark-scale-value=\"{}\" data-pnl-spark-rate-value=\"1.0\" data-pnl-spark-unit-value=\"$\" data-pnl-spark-delimiter-value=\",\" data-pnl-spark-suffixed-value=\"false\"",escape(&floats(&h.percent)),escape(&J::arr(&h.at,|n|J::Int(*n)).write()),J::Float(scale).write());
        if !hidden { attrs.push_str(&format!(" data-pnl-spark-profit-value=\"{}\"",escape(&floats(&h.profit_usd)))); }
        curve=format!("<div class=\"dash-intro__spark\">\n<div class=\"dash-intro__spark__curve\" style=\"width: {}%\" data-pnl-spark-target=\"curve\" data-action=\"pointermove->pnl-spark#move pointerleave->pnl-spark#leave\">\n<svg viewBox=\"0 0 100 200\" preserveAspectRatio=\"none\" aria-hidden=\"true\">\n<defs>\n<clipPath id=\"dash-spark-above\"><rect x=\"0\" y=\"-100\" width=\"100\" height=\"200\" /></clipPath>\n<clipPath id=\"dash-spark-below\"><rect x=\"0\" y=\"100\" width=\"100\" height=\"200\" /></clipPath>\n<path id=\"dash-spark-area\" d=\"{}\" />\n<path id=\"dash-spark-curve\" d=\"{}\" vector-effect=\"non-scaling-stroke\" />\n</defs>\n",float_to_s(width),escape(&area),escape(&path));
        for kind in ["area","line"] {
            for (gain,half) in [("gain","above"),("loss","below")] {
                curve.push_str(&format!("<use href=\"#dash-spark-{}\" class=\"dash-intro__spark__{kind} is-{gain}\" clip-path=\"url(#dash-spark-{half})\" />\n",if kind=="area"{"area"}else{"curve"}));
            }
        }
        curve.push_str(&format!("</svg>\n<span class=\"dash-intro__spark__end is-{}\" style=\"top: {}%\"></span>\n<span class=\"dash-intro__spark__dot is-gain\" data-pnl-spark-target=\"dot\" hidden></span>\n</div>\n</div>\n<span class=\"dash-intro__date\" data-pnl-spark-target=\"date\" hidden></span>\n",if gain{"gain"}else{"loss"},float_to_s(end)));
    }
    let percent=format!("{}{}",if pnl.percent.is_positive(){"+"}else{""},percent(&pnl.percent,2)?);
    let controller=if hidden{""}else{" data-controller=\"pnl-format\" data-action=\"click->pnl-format#toggle\" style=\"cursor: pointer;\""};
    let amount=if hidden{String::new()}else{format!("<span class=\"pnl-amount\" data-pnl-spark-target=\"amount\">{}{}</span>\n",if pnl.profit_usd.is_positive(){"+"}else{""},dollars(&pnl.profit_usd,0)?)};
    Ok(format!("<div id=\"global-pnl\"{attrs}>\n{curve}<div class=\"dash-intro__figure\" data-pnl-spark-target=\"figure\">\n<h1 class=\"header header--1\"{controller}>\n<span class=\"pnl-percent\" data-pnl-spark-target=\"percent\">{percent}</span>\n{amount}</h1>\n</div>\n</div>"))
}
