//! Symbols, renderers (single symbol, unique values, graduated colours, graduated symbols) and
//! labeling. A renderer maps each feature to a [`Symbol`] and describes its legend.

use rustgis_data::{Dataset, FieldType, GeometryType};
use serde::{Deserialize, Serialize};

use crate::color::{Ramp, Rgba};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Marker {
    #[default]
    Circle,
    Square,
    Triangle,
    Diamond,
    Star,
}

impl Marker {
    pub const ALL: [Marker; 5] = [Marker::Circle, Marker::Square, Marker::Triangle, Marker::Diamond, Marker::Star];
    pub fn label(self) -> &'static str {
        match self {
            Marker::Circle => "Circle",
            Marker::Square => "Square",
            Marker::Triangle => "Triangle",
            Marker::Diamond => "Diamond",
            Marker::Star => "Star",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    pub fill: Rgba,
    pub stroke: Rgba,
    /// Outline / line width in points.
    pub stroke_width: f32,
    /// Marker size in points.
    pub size: f32,
    pub marker: Marker,
}

/// Default palette for new layers (cycled by layer id).
const LAYER_COLORS: [[u8; 3]; 8] = [
    [0x4A, 0x90, 0xD9],
    [0xE0, 0x7B, 0x39],
    [0x5B, 0xB0, 0x6E],
    [0xC2, 0x55, 0x9B],
    [0xE6, 0xB8, 0x3A],
    [0x7D, 0x6B, 0xD1],
    [0x3C, 0xB1, 0xB8],
    [0xB9, 0x5F, 0x4A],
];

impl Symbol {
    pub fn default_for(g: GeometryType, seed: u32) -> Symbol {
        let c = LAYER_COLORS.get(seed as usize % LAYER_COLORS.len()).copied().unwrap_or([0x4A, 0x90, 0xD9]);
        let base = Rgba::rgb(c[0], c[1], c[2]);
        match g {
            GeometryType::Point => {
                Symbol { fill: base, stroke: Rgba::rgb(255, 255, 255), stroke_width: 1.0, size: 7.0, marker: Marker::Circle }
            }
            GeometryType::Polyline => Symbol { fill: base, stroke: base, stroke_width: 1.6, size: 0.0, marker: Marker::Circle },
            _ => Symbol { fill: base.with_alpha(170), stroke: base.shade(0.6), stroke_width: 0.7, size: 0.0, marker: Marker::Circle },
        }
    }

    pub fn with_fill(mut self, c: Rgba, g: GeometryType) -> Symbol {
        match g {
            GeometryType::Polyline => {
                self.fill = c;
                self.stroke = c;
            }
            GeometryType::Polygon => {
                self.fill = c.with_alpha(self.fill.a().max(150));
                self.stroke = c.shade(0.6);
            }
            _ => self.fill = c,
        }
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassMethod {
    #[default]
    NaturalBreaks,
    EqualInterval,
    Quantile,
}

impl ClassMethod {
    pub const ALL: [ClassMethod; 3] = [ClassMethod::NaturalBreaks, ClassMethod::EqualInterval, ClassMethod::Quantile];
    pub fn label(self) -> &'static str {
        match self {
            ClassMethod::NaturalBreaks => "Natural Breaks (Jenks)",
            ClassMethod::EqualInterval => "Equal Interval",
            ClassMethod::Quantile => "Quantile",
        }
    }
    pub fn from_name(s: &str) -> Option<ClassMethod> {
        let s = s.trim().to_ascii_lowercase();
        Some(match s.as_str() {
            "naturalbreaks" | "natural_breaks" | "jenks" | "natural breaks" => ClassMethod::NaturalBreaks,
            "equalinterval" | "equal_interval" | "equal interval" => ClassMethod::EqualInterval,
            "quantile" => ClassMethod::Quantile,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Renderer {
    Single(Symbol),
    Unique {
        field: String,
        classes: Vec<(String, Symbol)>,
        other: Symbol,
        ramp: Ramp,
    },
    /// Upper bounds of each class (ascending) and one symbol per class.
    Graduated {
        field: String,
        breaks: Vec<f64>,
        symbols: Vec<Symbol>,
        ramp: Ramp,
        method: ClassMethod,
        min: f64,
    },
    /// Marker size grows with the value.
    Proportional {
        field: String,
        base: Symbol,
        min_size: f32,
        max_size: f32,
        min: f64,
        max: f64,
    },
}

/// The legend label of a Unique Values renderer's fallback class.
pub const OTHER_VALUES: &str = "<all other values>";

/// One legend row.
#[derive(Clone, Debug, PartialEq)]
pub struct LegendEntry {
    pub label: String,
    pub symbol: Symbol,
    /// The Unique Values fallback row (`<all other values>`), told apart from a class whatever
    /// its label: a legend can leave it out without touching a class that bears the same name.
    pub fallback: bool,
}

/// Most unique-value classes created automatically.
pub const MAX_UNIQUE: usize = 64;

impl Renderer {
    pub fn kind_label(&self) -> &'static str {
        match self {
            Renderer::Single(_) => "Single Symbol",
            Renderer::Unique { .. } => "Unique Values",
            Renderer::Graduated { .. } => "Graduated Colors",
            Renderer::Proportional { .. } => "Proportional Symbols",
        }
    }

    pub fn field(&self) -> Option<&str> {
        match self {
            Renderer::Single(_) => None,
            Renderer::Unique { field, .. } | Renderer::Graduated { field, .. } | Renderer::Proportional { field, .. } => Some(field),
        }
    }

    /// The first symbol (the single symbol, or a representative).
    pub fn base_symbol(&self) -> Symbol {
        match self {
            Renderer::Single(s) => *s,
            Renderer::Unique { other, classes, .. } => classes.first().map(|c| c.1).unwrap_or(*other),
            Renderer::Graduated { symbols, .. } => symbols.last().copied().unwrap_or(Symbol::default_for(GeometryType::Polygon, 0)),
            Renderer::Proportional { base, .. } => *base,
        }
    }

    /// Builds a unique-values renderer on `field`, from the rows `rows` keeps (a row past its end
    /// counts, so `&[]` keeps them all).
    pub fn unique(ds: &Dataset, rows: &[bool], field: &str, ramp: Ramp, base: Symbol) -> Result<Renderer, String> {
        let fi = ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?;
        let vals = ds.unique_values(fi, rows, MAX_UNIQUE);
        let n = vals.len();
        let classes = vals.into_iter().enumerate().map(|(k, v)| (v, base.with_fill(ramp.pick(k, n), ds.geometry_type))).collect();
        let other = base.with_fill(Rgba::rgb(0xB0, 0xB0, 0xB0), ds.geometry_type);
        Ok(Renderer::Unique { field: ds.fields.get(fi).map(|f| f.name.clone()).unwrap_or_default(), classes, other, ramp })
    }

    /// Builds a graduated-colours renderer on a numeric `field`, classing the rows `rows` keeps (a
    /// row past its end counts, so `&[]` keeps them all).
    pub fn graduated(
        ds: &Dataset,
        rows: &[bool],
        field: &str,
        classes: usize,
        method: ClassMethod,
        ramp: Ramp,
        base: Symbol,
    ) -> Result<Renderer, String> {
        let fi = ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?;
        if !ds.fields.get(fi).is_some_and(|f| f.ty.is_numeric()) {
            return Err(format!("'{field}' is not a numeric field"));
        }
        let mut vals: Vec<f64> = ds.kept_features(rows).filter_map(|f| f.attrs.get(fi).and_then(|v| v.as_f64())).collect();
        if vals.is_empty() {
            return Err(format!("'{field}' has no numeric values"));
        }
        vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let k = classes.clamp(1, 12);
        let breaks = class_breaks(&vals, k, method);
        let n = breaks.len();
        let symbols = (0..n).map(|i| base.with_fill(ramp.pick(i, n), ds.geometry_type)).collect();
        let min = vals.first().copied().unwrap_or(0.0);
        Ok(Renderer::Graduated { field: ds.fields.get(fi).map(|f| f.name.clone()).unwrap_or_default(), breaks, symbols, ramp, method, min })
    }

    /// Builds a proportional-symbols renderer on a numeric `field`, sized over the rows `rows` keeps
    /// (a row past its end counts, so `&[]` keeps them all).
    pub fn proportional(ds: &Dataset, rows: &[bool], field: &str, base: Symbol) -> Result<Renderer, String> {
        let fi = ds.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?;
        if !ds.fields.get(fi).is_some_and(|f| f.ty.is_numeric()) {
            return Err(format!("'{field}' is not a numeric field"));
        }
        let (min, max) = ds.numeric_range(fi, rows).ok_or_else(|| format!("'{field}' has no numeric values"))?;
        let mut base = base;
        if base.size == 0.0 {
            base.size = 6.0;
        }
        Ok(Renderer::Proportional {
            field: ds.fields.get(fi).map(|f| f.name.clone()).unwrap_or_default(),
            base,
            min_size: 4.0,
            max_size: 28.0,
            min,
            max,
        })
    }

    /// Symbol for a feature row.
    pub fn symbol_for(&self, ds: &Dataset, row: usize) -> Symbol {
        match self {
            Renderer::Single(s) => *s,
            Renderer::Unique { field, classes, other, .. } => {
                let Some(fi) = ds.field_index(field) else { return *other };
                let v = ds.value(row, fi).to_string();
                classes.iter().find(|(k, _)| *k == v).map(|c| c.1).unwrap_or(*other)
            }
            Renderer::Graduated { field, breaks, symbols, .. } => {
                let fallback = symbols.first().copied().unwrap_or(Symbol::default_for(ds.geometry_type, 0));
                let Some(v) = ds.field_index(field).and_then(|fi| ds.value(row, fi).as_f64()) else {
                    return Symbol { fill: Rgba::new(0xC8, 0xC8, 0xC8, 140), ..fallback };
                };
                let i = breaks.iter().position(|b| v <= *b).unwrap_or(breaks.len().saturating_sub(1));
                symbols.get(i).copied().unwrap_or(fallback)
            }
            Renderer::Proportional { field, base, min_size, max_size, min, max } => {
                let Some(v) = ds.field_index(field).and_then(|fi| ds.value(row, fi).as_f64()) else { return *base };
                let t = if max > min { ((v - min) / (max - min)).clamp(0.0, 1.0) } else { 0.5 };
                // Area-proportional (Flannery-free square root scaling).
                Symbol { size: min_size + (max_size - min_size) * t.sqrt() as f32, ..*base }
            }
        }
    }

    /// The legend for the features `rows` keeps (a row past its end counts, so `&[]` keeps them
    /// all): a Unique Values class only when some kept feature has it, and `<all other values>`
    /// only when some kept feature matches no class (#207). The classes may come from a style file
    /// or from before a definition query changed, so they can name values no kept feature has. The
    /// other renderers list their classes as they are.
    pub fn legend_for(&self, ds: &Dataset, rows: &[bool]) -> Vec<LegendEntry> {
        let Renderer::Unique { field, classes, other, .. } = self else { return self.legend() };
        // A value matches its first class, as in `symbol_for`.
        let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (i, (k, _)) in classes.iter().enumerate() {
            index.entry(k.as_str()).or_insert(i);
        }
        let fi = ds.field_index(field);
        let mut used = vec![false; classes.len()];
        let mut other_used = false;
        // Distinct classes still unseen: once every class and "other" are in, the rest can't add any.
        let mut unseen = index.len() + 1;
        // One buffer for every value's text, rather than a string per feature.
        let mut text = String::new();
        for r in (0..ds.features.len()).filter(|r| rows.get(*r).copied().unwrap_or(true)) {
            let class = fi.and_then(|fi| {
                use std::fmt::Write;
                text.clear();
                write!(text, "{}", ds.value(r, fi)).ok()?;
                index.get(text.as_str()).copied()
            });
            let seen = match class.and_then(|i| used.get_mut(i)) {
                Some(u) => std::mem::replace(u, true),
                None => std::mem::replace(&mut other_used, true),
            };
            if !seen {
                unseen -= 1;
                if unseen == 0 {
                    break;
                }
            }
        }
        let mut v: Vec<LegendEntry> = classes
            .iter()
            .zip(&used)
            .filter(|(_, u)| **u)
            .map(|((k, s), _)| LegendEntry { label: k.clone(), symbol: *s, fallback: false })
            .collect();
        if other_used {
            v.push(LegendEntry { label: OTHER_VALUES.into(), symbol: *other, fallback: true });
        }
        v
    }

    pub fn legend(&self) -> Vec<LegendEntry> {
        match self {
            Renderer::Single(_) => Vec::new(),
            Renderer::Unique { classes, other, .. } => {
                let mut v: Vec<LegendEntry> =
                    classes.iter().map(|(k, s)| LegendEntry { label: k.clone(), symbol: *s, fallback: false }).collect();
                v.push(LegendEntry { label: OTHER_VALUES.into(), symbol: *other, fallback: true });
                v
            }
            Renderer::Graduated { breaks, symbols, min, .. } => {
                let mut lo = *min;
                breaks
                    .iter()
                    .zip(symbols)
                    .map(|(b, s)| {
                        let e = LegendEntry { label: format!("{} – {}", fmt_num(lo), fmt_num(*b)), symbol: *s, fallback: false };
                        lo = *b;
                        e
                    })
                    .collect()
            }
            Renderer::Proportional { base, min_size, max_size, min, max, .. } => vec![
                LegendEntry { label: fmt_num(*min), symbol: Symbol { size: *min_size, ..*base }, fallback: false },
                LegendEntry {
                    label: fmt_num((min + max) / 2.0),
                    symbol: Symbol { size: min_size + (max_size - min_size) * 0.707, ..*base },
                    fallback: false,
                },
                LegendEntry { label: fmt_num(*max), symbol: Symbol { size: *max_size, ..*base }, fallback: false },
            ],
        }
    }

    /// Recolours every class from a ramp.
    pub fn apply_ramp(&mut self, ramp: Ramp, g: GeometryType) {
        match self {
            Renderer::Unique { classes, ramp: r, .. } => {
                let n = classes.len();
                for (k, c) in classes.iter_mut().enumerate() {
                    c.1 = c.1.with_fill(ramp.pick(k, n), g);
                }
                *r = ramp;
            }
            Renderer::Graduated { symbols, ramp: r, .. } => {
                let n = symbols.len();
                for (k, s) in symbols.iter_mut().enumerate() {
                    *s = s.with_fill(ramp.pick(k, n), g);
                }
                *r = ramp;
            }
            _ => {}
        }
    }

    /// Applies `f` to every symbol.
    pub fn map_symbols(&mut self, f: impl Fn(&mut Symbol)) {
        match self {
            Renderer::Single(s) => f(s),
            Renderer::Unique { classes, other, .. } => {
                classes.iter_mut().for_each(|c| f(&mut c.1));
                f(other);
            }
            Renderer::Graduated { symbols, .. } => symbols.iter_mut().for_each(f),
            Renderer::Proportional { base, .. } => f(base),
        }
    }
}

/// Compact number formatting for legends: 1.2M, 35K, 0.42.
pub fn fmt_num(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.1}B", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e4 {
        format!("{:.0}K", v / 1e3)
    } else if a >= 100.0 || v.fract() == 0.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// Upper class bounds for sorted values.
pub fn class_breaks(sorted: &[f64], k: usize, method: ClassMethod) -> Vec<f64> {
    let n = sorted.len();
    let (Some(&lo), Some(&hi)) = (sorted.first(), sorted.last()) else { return Vec::new() };
    if k <= 1 || hi <= lo {
        return vec![hi];
    }
    let mut b = match method {
        ClassMethod::EqualInterval => (1..=k).map(|i| lo + (hi - lo) * i as f64 / k as f64).collect::<Vec<_>>(),
        ClassMethod::Quantile => (1..=k).map(|i| sorted.get(((n * i) / k).saturating_sub(1).min(n - 1)).copied().unwrap_or(hi)).collect(),
        ClassMethod::NaturalBreaks => jenks(sorted, k),
    };
    b.dedup_by(|a, b| (*a - *b).abs() < f64::EPSILON);
    if let Some(l) = b.last_mut() {
        *l = hi;
    }
    b
}

/// Jenks natural breaks (Fisher's dynamic programming), on a sample of at most 1000 values.
fn jenks(sorted: &[f64], k: usize) -> Vec<f64> {
    let data: Vec<f64> = if sorted.len() > 1000 {
        (0..1000).filter_map(|i| sorted.get(i * sorted.len() / 1000)).copied().collect()
    } else {
        sorted.to_vec()
    };
    let n = data.len();
    let k = k.min(n);
    if k < 2 {
        return vec![data.last().copied().unwrap_or(0.0)];
    }
    // lower[i][j]: first index of class j for data[0..=i]; var[i][j]: within-class variance.
    let mut lower = vec![vec![0usize; k + 1]; n + 1];
    let mut var = vec![vec![f64::INFINITY; k + 1]; n + 1];
    for j in 1..=k {
        lower[1][j] = 1;
        var[1][j] = 0.0;
    }
    for l in 2..=n {
        let (mut s1, mut s2, mut w) = (0.0, 0.0, 0.0);
        let mut v = 0.0;
        for m in 1..=l {
            let i3 = l - m + 1;
            let val = data[i3 - 1];
            s2 += val * val;
            s1 += val;
            w += 1.0;
            v = s2 - (s1 * s1) / w;
            let i4 = i3 - 1;
            if i4 != 0 {
                for j in 2..=k {
                    if var[l][j] >= v + var[i4][j - 1] {
                        lower[l][j] = i3;
                        var[l][j] = v + var[i4][j - 1];
                    }
                }
            }
        }
        lower[l][1] = 1;
        var[l][1] = v;
    }
    let mut breaks = vec![0.0; k];
    breaks[k - 1] = data[n - 1];
    let mut kk = n;
    for j in (2..=k).rev() {
        let id = lower[kk][j].saturating_sub(2);
        breaks[j - 2] = data[id];
        kk = lower[kk][j].saturating_sub(1).max(1);
    }
    breaks
}

/// Label settings for a layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Labeling {
    pub enabled: bool,
    pub field: String,
    pub size: f32,
    pub color: Rgba,
    pub halo: bool,
    pub bold: bool,
    /// Labels draw only when zoomed in beyond 1:`min_scale`, if set.
    #[serde(default)]
    pub min_scale: Option<f64>,
    /// Labels draw only when zoomed out beyond 1:`max_scale`, if set.
    #[serde(default)]
    pub max_scale: Option<f64>,
    /// Labels draw only when the viewer is at least this far away (metres), if set.
    #[serde(default)]
    pub closest: Option<f64>,
    /// Labels draw only when the viewer is at most this far away (metres), if set.
    #[serde(default)]
    pub farthest: Option<f64>,
    /// Where a label sits relative to its feature.
    #[serde(default)]
    pub placement: LabelPlacement,
}

/// Where labels sit relative to their feature's label point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelPlacement {
    /// Above points, centred on lines and polygons.
    #[default]
    Auto,
    Center,
    Above,
    Below,
    Left,
    Right,
    AboveRight,
    AboveLeft,
    BelowRight,
    BelowLeft,
}

impl LabelPlacement {
    pub const ALL: [LabelPlacement; 10] = [
        LabelPlacement::Auto,
        LabelPlacement::Center,
        LabelPlacement::Above,
        LabelPlacement::Below,
        LabelPlacement::Left,
        LabelPlacement::Right,
        LabelPlacement::AboveRight,
        LabelPlacement::AboveLeft,
        LabelPlacement::BelowRight,
        LabelPlacement::BelowLeft,
    ];
    pub fn code(self) -> &'static str {
        match self {
            LabelPlacement::Auto => "auto",
            LabelPlacement::Center => "center",
            LabelPlacement::Above => "above",
            LabelPlacement::Below => "below",
            LabelPlacement::Left => "left",
            LabelPlacement::Right => "right",
            LabelPlacement::AboveRight => "aboveRight",
            LabelPlacement::AboveLeft => "aboveLeft",
            LabelPlacement::BelowRight => "belowRight",
            LabelPlacement::BelowLeft => "belowLeft",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            LabelPlacement::Auto => "Automatic",
            LabelPlacement::Center => "Centered",
            LabelPlacement::Above => "Above",
            LabelPlacement::Below => "Below",
            LabelPlacement::Left => "Left",
            LabelPlacement::Right => "Right",
            LabelPlacement::AboveRight => "Above Right",
            LabelPlacement::AboveLeft => "Above Left",
            LabelPlacement::BelowRight => "Below Right",
            LabelPlacement::BelowLeft => "Below Left",
        }
    }
    pub fn from_code(s: &str) -> Option<LabelPlacement> {
        LabelPlacement::ALL.into_iter().find(|p| p.code().eq_ignore_ascii_case(s.trim()))
    }
    /// The label centre's offset from the label point, in points, for a label `w` wide and `h`
    /// tall next to a marker of size `marker` (0 for lines and polygons).
    pub fn offset(self, w: f32, h: f32, marker: f32, is_point: bool) -> (f32, f32) {
        let gap = marker / 2.0 + 2.0;
        let (dx, dy) = (w / 2.0 + gap, h / 2.0 + gap);
        match self {
            LabelPlacement::Auto if is_point => (0.0, -(marker / 2.0 + h * 0.6)),
            LabelPlacement::Auto | LabelPlacement::Center => (0.0, 0.0),
            LabelPlacement::Above => (0.0, -dy),
            LabelPlacement::Below => (0.0, dy),
            LabelPlacement::Left => (-dx, 0.0),
            LabelPlacement::Right => (dx, 0.0),
            LabelPlacement::AboveRight => (dx, -dy),
            LabelPlacement::AboveLeft => (-dx, -dy),
            LabelPlacement::BelowRight => (dx, dy),
            LabelPlacement::BelowLeft => (-dx, dy),
        }
    }
}

impl Labeling {
    /// Whether labels draw at a scale denominator and viewing distance (metres).
    pub fn in_range(&self, scale: f64, distance: f64) -> bool {
        self.min_scale.is_none_or(|m| scale <= m)
            && self.max_scale.is_none_or(|m| scale >= m)
            && self.closest.is_none_or(|d| distance >= d)
            && self.farthest.is_none_or(|d| distance <= d)
    }

    /// Off, on the first text field that looks like a name.
    pub fn default_for(ds: &Dataset) -> Labeling {
        let field = ds
            .fields
            .iter()
            .find(|f| f.ty == FieldType::Text && f.name.to_ascii_lowercase().contains("name"))
            .or_else(|| ds.fields.iter().find(|f| f.ty == FieldType::Text))
            .or_else(|| ds.fields.first())
            .map(|f| f.name.clone())
            .unwrap_or_default();
        Labeling {
            enabled: false,
            field,
            size: 10.0,
            color: Rgba::rgb(0x2B, 0x2B, 0x2B),
            halo: true,
            bold: false,
            min_scale: None,
            max_scale: None,
            closest: None,
            farthest: None,
            placement: LabelPlacement::Auto,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_data::{Field, Value};

    #[test]
    fn the_legend_lists_only_classes_kept_features_have() {
        let mut d = Dataset::new("t", GeometryType::Polygon, vec![Field::new("g", FieldType::Text)]);
        for g in ["a", "b", "a", "z"] {
            d.push(None, vec![Value::Text(g.into())]);
        }
        let s = |c| Symbol::default_for(GeometryType::Polygon, c);
        // "c" is no feature's value; "z" matches no class.
        let r = Renderer::Unique {
            field: "g".into(),
            classes: vec![("a".into(), s(0)), ("b".into(), s(1)), ("c".into(), s(2))],
            other: s(3),
            ramp: Ramp::Ember,
        };
        let labels = |rows: &[bool]| r.legend_for(&d, rows).into_iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(labels(&[]), ["a", "b", "<all other values>"]);
        assert_eq!(labels(&[true, false, true, false]), ["a"], "<all other values> only when used");
        assert_eq!(labels(&[false, true]), ["a", "b", "<all other values>"], "rows past the end (a, z) count as kept");
        assert_eq!(labels(&[false, false, false, true]), ["<all other values>"]);
        assert!(labels(&[false; 4]).is_empty());
        // The full legend is unchanged.
        assert_eq!(r.legend().len(), 4);
        // A class listed twice: the first matches, and the scan still stops once all are seen.
        let twice = Renderer::Unique {
            field: "g".into(),
            classes: vec![("a".into(), s(0)), ("a".into(), s(1)), ("b".into(), s(2))],
            other: s(3),
            ramp: Ramp::Ember,
        };
        let shown = twice.legend_for(&d, &[]);
        assert_eq!(shown.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(), ["a", "b", "<all other values>"]);
        assert_eq!(shown[0].symbol, s(0));
        // A field the data no longer has: every feature is "other".
        let gone = Renderer::Unique { field: "nope".into(), classes: vec![("a".into(), s(0))], other: s(3), ramp: Ramp::Ember };
        assert_eq!(gone.legend_for(&d, &[]).len(), 1);
        // The fallback row is marked as such, whatever a class is called.
        let named = Renderer::Unique {
            field: "g".into(),
            classes: vec![("a".into(), s(0)), (OTHER_VALUES.into(), s(1))],
            other: s(3),
            ramp: Ramp::Ember,
        };
        let mut d2 = d.clone();
        d2.push(None, vec![Value::Text(OTHER_VALUES.into())]);
        let rows = named.legend_for(&d2, &[]);
        assert_eq!(
            rows.iter().map(|e| (e.label.as_str(), e.fallback)).collect::<Vec<_>>(),
            [("a", false), (OTHER_VALUES, false), (OTHER_VALUES, true)]
        );
        // Other renderers list their classes as they are.
        assert_eq!(Renderer::Single(s(0)).legend_for(&d, &[false; 4]), Vec::new());
    }

    #[test]
    fn breaks() {
        let v: Vec<f64> = vec![1.0, 2.0, 3.0, 10.0, 11.0, 12.0, 50.0, 51.0, 52.0];
        let j = class_breaks(&v, 3, ClassMethod::NaturalBreaks);
        assert_eq!(j, vec![3.0, 12.0, 52.0]);
        assert_eq!(class_breaks(&v, 3, ClassMethod::EqualInterval).len(), 3);
        assert_eq!(class_breaks(&[], 3, ClassMethod::Quantile), Vec::<f64>::new());
        assert_eq!(class_breaks(&[5.0, 5.0], 4, ClassMethod::NaturalBreaks), vec![5.0]);
    }

    #[test]
    fn graduated_symbols_by_value() {
        let mut d = Dataset::new("t", GeometryType::Polygon, vec![Field::new("v", FieldType::Double)]);
        for v in [1.0, 2.0, 50.0, 60.0] {
            d.push(None, vec![Value::Float(v)]);
        }
        d.push(None, vec![Value::Null]);
        let r =
            Renderer::graduated(&d, &[], "v", 2, ClassMethod::NaturalBreaks, Ramp::Ember, Symbol::default_for(GeometryType::Polygon, 0))
                .unwrap();
        assert_ne!(r.symbol_for(&d, 0).fill, r.symbol_for(&d, 3).fill);
        assert_eq!(r.legend().len(), 2);
        let _ = r.symbol_for(&d, 4);
        let _ = r.symbol_for(&d, 999);
        assert!(
            Renderer::graduated(&d, &[], "nope", 2, ClassMethod::Quantile, Ramp::Ember, Symbol::default_for(GeometryType::Polygon, 0))
                .is_err()
        );
    }

    #[test]
    fn classes_come_from_the_kept_rows() {
        let mut d = Dataset::new("t", GeometryType::Polygon, vec![Field::new("v", FieldType::Double)]);
        for v in [1.0, 2.0, 3.0, 1000.0] {
            d.push(None, vec![Value::Float(v)]);
        }
        let base = Symbol::default_for(GeometryType::Polygon, 0);
        let rows = [true, true, true, false];
        let Ok(Renderer::Graduated { breaks, min, .. }) =
            Renderer::graduated(&d, &rows, "v", 2, ClassMethod::EqualInterval, Ramp::Ember, base)
        else {
            panic!("graduated renderer expected")
        };
        assert_eq!(breaks.last().copied(), Some(3.0));
        assert_eq!(min, 1.0);
        // A short row list keeps the rows past its end.
        let Ok(Renderer::Graduated { breaks, .. }) =
            Renderer::graduated(&d, &[false], "v", 2, ClassMethod::EqualInterval, Ramp::Ember, base)
        else {
            panic!("graduated renderer expected")
        };
        assert_eq!(breaks.last().copied(), Some(1000.0));
        assert!(Renderer::graduated(&d, &[false; 4], "v", 2, ClassMethod::Quantile, Ramp::Ember, base).is_err());
        let Ok(Renderer::Unique { classes, .. }) = Renderer::unique(&d, &[false, true, false, true], "v", Ramp::Harbour, base) else {
            panic!("unique renderer expected")
        };
        let keys: Vec<&str> = classes.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["1000", "2"]);
    }
}
