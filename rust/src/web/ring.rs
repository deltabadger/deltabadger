//! The navbar's tracker icon: TrackerHelper#allocation_icon_arcs, one dashed circle per priced
//! holding. Pinned by the "bot_pages" vectors.
use super::colors;
use super::format::{float_round, float_to_s};
use crate::ruby::BigDec;

const CIRCUMFERENCE: f64 = 2.0 * std::f64::consts::PI * 9.0; // ICON_RADIUS
const MIN_SPAN: f64 = 2.0 + 2.0; // ICON_GAP + ICON_STROKE

pub struct Arc {
    pub color: String,
    pub dash: String,
    pub offset: String,
}

/// TrackerHelper#ring_slices: positive values, largest first; everything under 2 % is gathered
/// into one neutral slice, last, unless it is a single holding.
/// ponytail: Ruby's sort_by is not stable, so two holdings worth exactly the same may swap places
/// there; here they keep the order the query gave them.
fn slices(mut values: Vec<(BigDec, Option<String>)>) -> Vec<(BigDec, Option<String>)> {
    values.retain(|(value, _)| value.is_positive());
    values.sort_by(|a, b| b.0.cmp(&a.0));
    let total = values.iter().fold(BigDec::zero(), |sum, (value, _)| &sum + value);
    if !total.is_positive() { return vec![]; }
    let floor = BigDec::parse("0.02").unwrap_or_else(|_| BigDec::zero()); // RING_MIN_SHARE
    let (small, large): (Vec<_>, Vec<_>) = values.iter().cloned().partition(|(value, _)| value.div(&total).is_some_and(|share| share < floor));
    if small.len() == 1 { return values; }
    let folded = small.iter().fold(BigDec::zero(), |sum, (value, _)| &sum + value);
    let mut out = large;
    if folded.is_positive() { out.push((folded, Some(colors::NEUTRAL.to_string()))); }
    out
}

/// `icon_arcs`: the slices placed largest first, each at least a dot wide, until the ring is full
/// (TrackerHelper#ring_places), then walked clockwise from twelve o'clock (#dashed_ring). `None`
/// where Rails raises on a colour it cannot read.
pub fn icon_arcs(values: Vec<(BigDec, Option<String>)>) -> Option<Vec<Arc>> {
    let slices = slices(values);
    let total = slices.iter().fold(BigDec::zero(), |sum, (value, _)| &sum + value);
    if !total.is_positive() { return Some(vec![]); }
    let mut order: Vec<usize> = (0..slices.len()).collect();
    order.sort_by(|a, b| slices[*b].0.cmp(&slices[*a].0));
    let mut spans: Vec<Option<f64>> = vec![None; slices.len()];
    let mut used = 0.0;
    for index in order {
        if used > CIRCUMFERENCE { break; }
        let span = (slices[index].0.div(&total)?.to_f() * CIRCUMFERENCE).max(MIN_SPAN);
        spans[index] = Some(span);
        used += span;
    }
    let scale = CIRCUMFERENCE / used;
    let mut walked = -CIRCUMFERENCE / 4.0;
    let mut arcs = vec![];
    for (index, span) in spans.iter().enumerate() {
        let Some(span) = span.map(|span| span * scale) else { continue };
        let length = (span - MIN_SPAN).max(0.0);
        let color = slices[index].1.as_deref().filter(|c| !c.trim().is_empty()).unwrap_or(colors::NEUTRAL);
        arcs.push(Arc {
            color: colors::ensure_contrast(color)?,
            dash: format!("{} {}", float_to_s(float_round(length, 2)), float_to_s(float_round(CIRCUMFERENCE - length, 2))),
            offset: float_to_s(-float_round(walked + MIN_SPAN / 2.0, 2)),
        });
        walked += span;
    }
    Some(arcs)
}
