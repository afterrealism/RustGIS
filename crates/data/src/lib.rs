//! L1: the feature model (geometry + typed attributes) and vector I/O.
//!
//! Coordinates are stored as WGS 84 longitude/latitude. Readers never panic on malformed input:
//! bad features are skipped and counted in [`ReadReport`].
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod dxf;
pub mod geometry;
pub mod georss;
pub mod gml;
pub mod gpx;
pub mod interop;
pub mod io;
pub mod kml;
pub mod landxml;
pub mod lidar;
pub mod mapinfo;
pub mod mbtiles;
pub mod ogc;
pub mod osm;
pub mod photo;
pub mod query;
pub mod raster;
pub mod sheets;
pub mod source;
pub mod sqlite;
pub mod topojson;
pub mod value;
pub mod wkb;
pub mod xml;
pub mod zip;

pub use geometry::{Geometry, GeometryType};
pub use rustgis_geom::{Coord, Extent};
pub use value::{Field, FieldType, Value};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Format(String),
    #[error("unsupported file type: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, DataError>;

/// One row: an optional geometry and one value per field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    pub id: u64,
    pub geometry: Option<Geometry>,
    pub attrs: Vec<Value>,
}

/// A dataset's features, shared until changed: cloning them (an undo snapshot, a copy of the
/// project) copies a pointer, and the first change through one of the copies copies the rows
/// for it. A large layer is then held once however many undo steps leave it alone.
#[derive(Clone, Default)]
pub struct Features(std::sync::Arc<Vec<Feature>>);

impl Features {
    /// Whether two are the same rows in memory (one cloned from the other, neither changed since).
    pub fn shares(&self, other: &Features) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }

    /// The rows, taking them out of the share (copying them only when another copy holds them).
    pub fn into_vec(self) -> Vec<Feature> {
        std::sync::Arc::unwrap_or_clone(self.0)
    }
}

impl std::ops::Deref for Features {
    type Target = Vec<Feature>;
    fn deref(&self) -> &Vec<Feature> {
        &self.0
    }
}

impl std::ops::DerefMut for Features {
    fn deref_mut(&mut self) -> &mut Vec<Feature> {
        std::sync::Arc::make_mut(&mut self.0)
    }
}

impl PartialEq for Features {
    fn eq(&self, other: &Features) -> bool {
        self.shares(other) || *self.0 == *other.0
    }
}

impl std::fmt::Debug for Features {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Vec<Feature>> for Features {
    fn from(v: Vec<Feature>) -> Features {
        Features(std::sync::Arc::new(v))
    }
}

impl FromIterator<Feature> for Features {
    fn from_iter<I: IntoIterator<Item = Feature>>(it: I) -> Features {
        Features::from(it.into_iter().collect::<Vec<_>>())
    }
}

impl IntoIterator for Features {
    type Item = Feature;
    type IntoIter = std::vec::IntoIter<Feature>;
    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}

impl<'a> IntoIterator for &'a Features {
    type Item = &'a Feature;
    type IntoIter = std::slice::Iter<'a, Feature>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a> IntoIterator for &'a mut Features {
    type Item = &'a mut Feature;
    type IntoIter = std::slice::IterMut<'a, Feature>;
    fn into_iter(self) -> Self::IntoIter {
        std::ops::DerefMut::deref_mut(self).iter_mut()
    }
}

impl Serialize for Features {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Features {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Features, D::Error> {
        Vec::<Feature>::deserialize(d).map(Features::from)
    }
}

/// A feature class: a named table of features sharing a schema and geometry type.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Dataset {
    pub name: String,
    pub geometry_type: GeometryType,
    pub fields: Vec<Field>,
    pub features: Features,
    /// Where it was read from, if anywhere.
    #[serde(default)]
    pub source: Option<String>,
    /// The features are a window onto a source read in place (the part of a large file in view),
    /// so they aren't saved: the source is read again instead.
    #[serde(skip)]
    pub windowed: bool,
}

impl Serialize for Dataset {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Dataset", 5)?;
        st.serialize_field("name", &self.name)?;
        st.serialize_field("geometry_type", &self.geometry_type)?;
        st.serialize_field("fields", &self.fields)?;
        let none: &[Feature] = &[];
        st.serialize_field("features", if self.windowed { none } else { self.features.as_slice() })?;
        st.serialize_field("source", &self.source)?;
        st.end()
    }
}

/// What a reader skipped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadReport {
    pub skipped: usize,
    pub messages: Vec<String>,
}

impl Dataset {
    pub fn new(name: impl Into<String>, geometry_type: GeometryType, fields: Vec<Field>) -> Self {
        Dataset { name: name.into(), geometry_type, fields, features: Features::default(), source: None, windowed: false }
    }

    /// Index of a field by case-insensitive name.
    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// Value of `field` for feature `row` (Null when out of range).
    pub fn value(&self, row: usize, field: usize) -> &Value {
        self.features.get(row).and_then(|f| f.attrs.get(field)).unwrap_or(&Value::Null)
    }

    pub fn extent(&self) -> Extent {
        self.features.iter().filter_map(|f| f.geometry.as_ref()).fold(Extent::EMPTY, |e, g| e.union(&g.extent()))
    }

    /// Pushes a feature with the next id, padding/truncating attributes to the schema.
    pub fn push(&mut self, geometry: Option<Geometry>, mut attrs: Vec<Value>) {
        attrs.resize(self.fields.len(), Value::Null);
        let id = self.features.iter().map(|f| f.id).max().map(|m| m.saturating_add(1)).unwrap_or(1);
        self.features.push(Feature { id, geometry, attrs });
    }

    /// Pushes a feature with the id one past the last feature's, padding/truncating attributes:
    /// for readers filling a new dataset, where [`Dataset::push`]'s search for the largest id
    /// would make reading quadratic.
    pub fn push_next(&mut self, geometry: Option<Geometry>, mut attrs: Vec<Value>) {
        attrs.resize(self.fields.len(), Value::Null);
        let id = self.features.last().map(|f| f.id.saturating_add(1)).unwrap_or(1);
        self.features.push(Feature { id, geometry, attrs });
    }

    /// Adds a field (if missing) and returns its index.
    pub fn ensure_field(&mut self, name: &str, ty: FieldType) -> usize {
        if let Some(i) = self.field_index(name) {
            return i;
        }
        self.fields.push(Field::new(name, ty));
        for f in &mut self.features {
            f.attrs.push(Value::Null);
        }
        self.fields.len() - 1
    }

    /// The features `rows` keeps, by position; a row past its end counts, so `&[]` keeps them all.
    pub fn kept_features<'a>(&'a self, rows: &'a [bool]) -> impl Iterator<Item = &'a Feature> {
        self.features.iter().enumerate().filter(|(r, _)| rows.get(*r).copied().unwrap_or(true)).map(|(_, f)| f)
    }

    /// Numeric (min, max) of a field over the features `rows` keeps, ignoring non-numbers.
    pub fn numeric_range(&self, field: usize, rows: &[bool]) -> Option<(f64, f64)> {
        let mut r: Option<(f64, f64)> = None;
        for f in self.kept_features(rows) {
            if let Some(v) = f.attrs.get(field).and_then(Value::as_f64) {
                r = Some(match r {
                    Some((a, b)) => (a.min(v), b.max(v)),
                    None => (v, v),
                });
            }
        }
        r
    }

    /// Distinct display values of a field over the features `rows` keeps, sorted, capped at `limit`.
    pub fn unique_values(&self, field: usize, rows: &[bool], limit: usize) -> Vec<String> {
        let mut v: Vec<String> = self.kept_features(rows).map(|f| f.attrs.get(field).map(|x| x.to_string()).unwrap_or_default()).collect();
        v.sort();
        v.dedup();
        v.truncate(limit);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_are_shared_until_changed() {
        let mut d = Dataset::new("t", GeometryType::Point, vec![Field::new("a", FieldType::Integer)]);
        d.push(Some(Geometry::Point(Coord::new(1.0, 2.0))), vec![Value::Int(5)]);
        let mut copy = d.clone();
        assert!(copy.features.shares(&d.features));
        // Reading doesn't copy; the first change does, and only the changed copy sees it.
        assert_eq!(copy.features.len(), 1);
        assert!(copy.features.shares(&d.features));
        copy.push(Some(Geometry::Point(Coord::new(3.0, 4.0))), vec![Value::Int(6)]);
        assert!(!copy.features.shares(&d.features));
        assert_eq!((d.features.len(), copy.features.len()), (1, 2));
        for f in &mut copy.features {
            f.attrs[0] = Value::Int(0);
        }
        assert_eq!(d.value(0, 0), &Value::Int(5));
        assert_ne!(copy, d);
        let back: Features = serde_json::from_str(&serde_json::to_string(&copy.features).unwrap()).unwrap();
        assert_eq!(back, copy.features);
    }

    #[test]
    fn windowed_features_are_not_saved() {
        let mut d = Dataset::new("t", GeometryType::Point, vec![Field::new("a", FieldType::Integer)]);
        d.push(Some(Geometry::Point(Coord::new(1.0, 2.0))), vec![Value::Int(5)]);
        let back: Dataset = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(back, d);
        d.windowed = true;
        let back: Dataset = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert!(back.features.is_empty() && !back.windowed);
        assert_eq!(back.fields, d.fields);
    }

    #[test]
    fn push_pads_attrs_and_assigns_ids() {
        let mut d = Dataset::new("t", GeometryType::Point, vec![Field::new("a", FieldType::Integer), Field::new("b", FieldType::Text)]);
        d.push(Some(Geometry::Point(Coord::new(1.0, 2.0))), vec![Value::Int(5)]);
        d.push(None, vec![Value::Int(1), Value::Null, Value::Int(9)]);
        assert_eq!(d.features[0].attrs.len(), 2);
        assert_eq!(d.features[1].attrs.len(), 2);
        assert_eq!(d.features[1].id, 2);
        assert_eq!(d.numeric_range(0, &[]), Some((1.0, 5.0)));
        assert_eq!(d.numeric_range(0, &[false]), Some((1.0, 1.0)));
        assert_eq!(d.numeric_range(0, &[false, false]), None);
        assert_eq!(d.unique_values(0, &[true, false], 10), vec!["5"]);
        assert_eq!(d.value(99, 99), &Value::Null);
    }
}
