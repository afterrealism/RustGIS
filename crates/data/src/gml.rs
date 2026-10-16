//! GML: simple features in Geography Markup Language 2 and 3 — WFS feature collections, OGR's
//! GML, and other application schemas whose features are members of a collection. Each feature
//! type becomes a layer; a feature's simple child elements are its attributes and its first
//! geometry is its shape (points, lines, polygons and their multi forms, envelopes).
//!
//! Coordinates are converted to longitude/latitude from the `srsName` (EPSG codes, URNs and URLs);
//! the URN and URL forms of geographic systems are latitude first, as GML 3 says. A file without
//! one is taken as longitude/latitude, and the report says so when that looks wrong.
//!
//! The file is hostile: no DTD is read, the node count is capped, and features whose geometry
//! can't be read are counted.

use std::collections::BTreeMap;

use roxmltree::{Node, ParsingOptions};
use rustgis_geom::Coord;

use crate::raster::Crs;
use crate::{DataError, Dataset, Field, Geometry, GeometryType, ReadReport, Result, Value};

/// GML file types.
pub const EXTENSIONS: &[&str] = &["gml"];

/// Most feature types (layers) read from one file.
const MAX_LAYERS: usize = 64;

pub fn is_gml_name(name: &str) -> bool {
    let ext = std::path::Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    EXTENSIONS.contains(&ext.as_str())
}

fn local<'a>(n: &Node<'a, '_>) -> &'a str {
    n.tag_name().name()
}

fn is_gml(n: &Node) -> bool {
    n.tag_name().namespace().is_some_and(|ns| ns.starts_with("http://www.opengis.net/gml"))
}

fn text_of(n: &Node) -> String {
    n.children().filter(|t| t.is_text()).filter_map(|t| t.text()).collect::<String>()
}

/// A coordinate system named by `srsName`, and whether its axes are latitude first.
fn srs(name: &str) -> Option<(Crs, bool)> {
    let n = name.trim();
    let up = n.to_ascii_uppercase();
    if up.ends_with("CRS84") {
        return Some((Crs::LonLat, false));
    }
    let code: u32 = n.rsplit([':', '/', '#']).next()?.trim().parse().ok()?;
    let crs = Crs::from_epsg(code)?;
    // `EPSG:4326` and the old `…epsg.xml#4326` are longitude first; URNs and the
    // opengis.net/def URLs follow the EPSG axis order, latitude first for geographic systems.
    let official = up.starts_with("URN:") || up.contains("OPENGIS.NET/DEF/CRS");
    Some((crs, official && crs.is_geographic()))
}

/// GML geometry elements read.
const GEOMETRIES: &[&str] = &[
    "Point",
    "MultiPoint",
    "LineString",
    "MultiLineString",
    "Curve",
    "MultiCurve",
    "Polygon",
    "MultiPolygon",
    "Surface",
    "MultiSurface",
    "Envelope",
    "Box",
    "LinearRing",
];

/// The coordinates under a node: `gml:pos`/`gml:posList` (with `srsDimension`),
/// `gml:coordinates` (`cs`, `ts`) or `gml:coord` (`X`, `Y`).
fn coords(n: &Node) -> Option<Vec<Coord>> {
    let mut out = Vec::new();
    for c in n.descendants().filter(|d| d.is_element() && is_gml(d)) {
        match local(&c) {
            "pos" | "posList" | "lowerCorner" | "upperCorner" => {
                let v: Vec<f64> = text_of(&c).split_whitespace().map(|t| t.parse().ok()).collect::<Option<_>>()?;
                // Without srsDimension, one position's dimension is its count (a 3D `pos`).
                let one = local(&c) != "posList" && (2..=4).contains(&v.len());
                let dim: usize = c
                    .attribute("srsDimension")
                    .or_else(|| c.ancestors().find_map(|a| a.attribute("srsDimension")))
                    .and_then(|d| d.parse().ok())
                    .filter(|d| (2..=4).contains(d))
                    .unwrap_or(if one { v.len() } else { 2 });
                if !v.len().is_multiple_of(dim) {
                    return None;
                }
                out.extend(
                    v.chunks(dim).map(|p| Coord::new(p.first().copied().unwrap_or(f64::NAN), p.get(1).copied().unwrap_or(f64::NAN))),
                );
            }
            "coordinates" => {
                let cs = c.attribute("cs").unwrap_or(",");
                let ts = c.attribute("ts").unwrap_or(" ");
                let text = text_of(&c);
                for tuple in
                    text.split(|ch: char| if ts == " " { ch.is_whitespace() } else { ts.contains(ch) }).filter(|t| !t.trim().is_empty())
                {
                    let v: Vec<f64> = tuple.split(cs).map(|t| t.trim().parse().ok()).collect::<Option<_>>()?;
                    out.push(Coord::new(*v.first()?, *v.get(1)?));
                }
            }
            "coord" => {
                let get =
                    |k: &str| c.children().find(|x| x.is_element() && local(x) == k).and_then(|x| text_of(&x).trim().parse::<f64>().ok());
                out.push(Coord::new(get("X")?, get("Y")?));
            }
            _ => {}
        }
    }
    (out.iter().all(|c| c.is_finite())).then_some(out)
}

fn ring(mut r: Vec<Coord>) -> Option<Vec<Coord>> {
    if r.first() != r.last() {
        r.push(*r.first()?);
    }
    (r.len() >= 4).then_some(r)
}

/// The rings of a polygon or surface patch: exterior (outerBoundaryIs) first.
fn polygon(p: &Node) -> Option<Vec<Vec<Coord>>> {
    let boundary = |names: &[&str]| -> Vec<Vec<Coord>> {
        p.descendants().filter(|d| d.is_element() && is_gml(d) && names.contains(&local(d))).filter_map(|b| ring(coords(&b)?)).collect()
    };
    let mut rings = boundary(&["exterior", "outerBoundaryIs"]);
    if rings.is_empty() {
        return None;
    }
    rings.truncate(1);
    rings.extend(boundary(&["interior", "innerBoundaryIs"]));
    Some(rings)
}

/// A GML geometry element as a geometry.
fn geometry(g: &Node) -> Option<Geometry> {
    let kids =
        |names: &[&str]| -> Vec<Node> { g.descendants().filter(|d| d.is_element() && is_gml(d) && names.contains(&local(d))).collect() };
    match local(g) {
        "Point" => coords(g)?.first().map(|c| Geometry::Point(*c)),
        "MultiPoint" => {
            let pts: Vec<Coord> = kids(&["Point"]).iter().filter_map(|p| coords(p)?.first().copied()).collect();
            (!pts.is_empty()).then_some(Geometry::MultiPoint(pts))
        }
        "LineString" | "Curve" | "LinearRing" => {
            let c = coords(g)?;
            (c.len() >= 2).then_some(Geometry::Line(vec![c]))
        }
        "MultiLineString" | "MultiCurve" => {
            let parts: Vec<Vec<Coord>> = kids(&["LineString", "Curve"]).iter().filter_map(|l| coords(l)).filter(|c| c.len() >= 2).collect();
            (!parts.is_empty()).then_some(Geometry::Line(parts))
        }
        "Polygon" => Some(Geometry::Polygon(vec![polygon(g)?])),
        "Surface" | "MultiPolygon" | "MultiSurface" => {
            let polys: Vec<Vec<Vec<Coord>>> = kids(&["Polygon", "PolygonPatch"]).iter().filter_map(polygon).collect();
            (!polys.is_empty()).then_some(Geometry::Polygon(polys))
        }
        "Envelope" | "Box" => {
            let c = coords(g)?;
            let (a, b) = (c.first()?, c.get(1)?);
            let (w, e, s, n) = (a.x.min(b.x), a.x.max(b.x), a.y.min(b.y), a.y.max(b.y));
            (w < e && s < n).then(|| {
                Geometry::Polygon(vec![vec![vec![
                    Coord::new(w, s),
                    Coord::new(e, s),
                    Coord::new(e, n),
                    Coord::new(w, n),
                    Coord::new(w, s),
                ]]])
            })
        }
        _ => None,
    }
}

/// A feature type being read: its attribute columns in order of appearance, and its rows.
#[derive(Default)]
struct Kind {
    columns: Vec<String>,
    rows: Vec<(Option<Geometry>, BTreeMap<String, String>)>,
}

/// Reads a GML file into a layer per feature type (those with features).
pub fn read(file_name: &str, bytes: &[u8]) -> Result<Vec<(Dataset, ReadReport)>> {
    let stem = std::path::Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("gml").to_string();
    let text = std::str::from_utf8(bytes).map_err(|_| DataError::Format(format!("{file_name} is not UTF-8 text")))?;
    let opts = ParsingOptions { allow_dtd: false, nodes_limit: crate::io::XML_NODES, ..ParsingOptions::default() };
    let doc = crate::xml::parse(text.trim_start_matches('\u{feff}'), opts).map_err(|e| DataError::Format(format!("{file_name}: {e}")))?;
    let root = doc.root_element();
    // The features: children of member elements (not of one inside a feature, such as a
    // property or a geometry collection's member named alike).
    let is_member = |n: &Node| n.is_element() && matches!(local(n), "featureMember" | "featureMembers" | "member" | "members");
    let members: Vec<Node> = root
        .descendants()
        .filter(|n| is_member(n) && n.ancestors().skip(1).all(|a| !is_member(&a)))
        .flat_map(|m| m.children().filter(|c| c.is_element()))
        .filter(|f| !(is_gml(f) && GEOMETRIES.contains(&local(f))))
        .collect();
    if members.is_empty() {
        return Err(DataError::Format(format!("{file_name}: no features (featureMember elements) in the GML")));
    }
    let mut kinds: BTreeMap<String, Kind> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    let (mut skipped, mut file_srs): (usize, Option<(Crs, bool)>) = (0, None);
    let mut unknown_srs: Option<String> = None;
    let mut over = 0usize;
    for f in members {
        let name = local(&f).to_string();
        if !kinds.contains_key(&name) {
            if kinds.len() >= MAX_LAYERS {
                over += 1;
                continue;
            }
            order.push(name.clone());
        }
        let kind = kinds.entry(name).or_default();
        let mut attrs = BTreeMap::new();
        let mut geom = None;
        let mut bad = false;
        for p in f.children().filter(|c| c.is_element()) {
            // A property holding a geometry, or a simple value.
            // A feature's bounding box (gml:boundedBy) is neither its shape nor an attribute.
            if is_gml(&p) && local(&p) == "boundedBy" {
                continue;
            }
            if let Some(g) = p.children().find(|c| c.is_element() && is_gml(c) && GEOMETRIES.contains(&local(c))) {
                if geom.is_some() {
                    continue;
                }
                let s = g.attribute("srsName").or_else(|| g.descendants().find_map(|d| d.attribute("srsName")));
                if let Some(s) = s {
                    match srs(s) {
                        Some(found) => {
                            file_srs.get_or_insert(found);
                        }
                        None => unknown_srs = Some(s.to_string()),
                    }
                }
                match geometry(&g) {
                    Some(gm) => geom = Some(gm),
                    None => bad = true,
                }
            } else if p.children().any(|c| c.is_element() && is_gml(&c)) {
                // A geometry of a kind not read (a CompositeCurve, a Solid…): said as unreadable.
                if geom.is_none() {
                    bad = true;
                }
            } else if !(p.children().any(|c| c.is_element()) || is_gml(&p) && local(&p) == "boundedBy") {
                let key = local(&p).to_string();
                if !kind.columns.contains(&key) && kind.columns.len() < 500 {
                    kind.columns.push(key.clone());
                }
                attrs.insert(key, text_of(&p).trim().to_string());
            }
        }
        // A gml:id is a feature's id when it has no other.
        if let Some(id) = f.attributes().find(|a| a.name() == "id" || a.name() == "fid").map(|a| a.value().to_string()) {
            if !kind.columns.iter().any(|c| c == "gml_id") {
                kind.columns.insert(0, "gml_id".into());
            }
            attrs.insert("gml_id".into(), id);
        }
        // A geometry that couldn't be read, unless another property gave the feature its shape.
        skipped += usize::from(bad && geom.is_none());
        kind.rows.push((geom, attrs));
    }
    let mut report = ReadReport { skipped, ..ReadReport::default() };
    if skipped > 0 {
        report.messages.push(format!("{skipped} features had geometry that couldn't be read"));
    }
    if over > 0 {
        report.messages.push(format!("{over} features of more than {MAX_LAYERS} feature types were left out"));
    }
    if let Some(s) = &unknown_srs {
        report.messages.push(format!("unknown coordinate system '{s}': coordinates are read as they are"));
    }
    let transform = match file_srs {
        Some((c, swap)) => Some((if c == Crs::LonLat { None } else { Some(c.transformer().map_err(DataError::Format)?) }, swap)),
        None => None,
    };
    let single = order.len() == 1;
    let mut out = Vec::new();
    for name in order {
        let Some(kind) = kinds.remove(&name) else { continue };
        let fields: Vec<Field> = kind
            .columns
            .iter()
            .map(|c| Field::new(c.clone(), crate::io::text_type(kind.rows.iter().filter_map(|(_, a)| a.get(c)).map(String::as_str))))
            .collect();
        let title = if single { stem.clone() } else { format!("{stem} {name}") };
        let mut ds = Dataset::new(title, GeometryType::None, fields);
        let mut outside = false;
        for (g, attrs) in kind.rows {
            let g = g.map(|g| match &transform {
                Some((t, swap)) => g.map_coords(&|c| {
                    let c = if *swap { Coord::new(c.y, c.x) } else { c };
                    match t {
                        Some(t) => t.to_lonlat(c).unwrap_or(Coord::new(f64::NAN, f64::NAN)),
                        None => c,
                    }
                }),
                None => g,
            });
            if let Some(g) = &g {
                let e = g.extent();
                outside |= e.min_x < -180.0 || e.max_x > 180.0 || e.min_y < -90.0 || e.max_y > 90.0;
            }
            let row = ds.fields.iter().map(|f| attrs.get(&f.name).map(|v| Value::parse(v, f.ty)).unwrap_or_default()).collect();
            ds.push_next(g, row);
        }
        ds.geometry_type = crate::io::dominant_type(&ds);
        // The file's notices go with its first layer only.
        let mut rep = if out.is_empty() { report.clone() } else { ReadReport::default() };
        if file_srs.is_none() && outside {
            rep.messages.push("the GML doesn't say its coordinate system (srsName) and its coordinates aren't longitude/latitude".into());
        }
        out.push((ds, rep));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FieldType;

    const WFS: &str = r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="http://example.com/app">
<wfs:member><app:roads gml:id="r.1"><gml:boundedBy><gml:Envelope srsName="urn:ogc:def:crs:EPSG::4326"><gml:lowerCorner>48 10</gml:lowerCorner><gml:upperCorner>49 11</gml:upperCorner></gml:Envelope></gml:boundedBy><app:name>Main</app:name><app:lanes>2</app:lanes>
  <app:geom><gml:LineString srsName="urn:ogc:def:crs:EPSG::4326"><gml:posList>48 10 49 11</gml:posList></gml:LineString></app:geom></app:roads></wfs:member>
<wfs:member><app:roads gml:id="r.2"><app:name>Side</app:name><app:lanes>1</app:lanes>
  <app:geom><gml:LineString srsName="urn:ogc:def:crs:EPSG::4326"><gml:posList srsDimension="3">48 10 5 49 12 6</gml:posList></gml:LineString></app:geom></app:roads></wfs:member>
<wfs:member><app:parks gml:id="p.1"><app:name>Park</app:name>
  <app:geom><gml:Polygon srsName="urn:ogc:def:crs:EPSG::4326"><gml:exterior><gml:LinearRing><gml:posList>48 10 48 11 49 11 48 10</gml:posList></gml:LinearRing></gml:exterior>
  <gml:interior><gml:LinearRing><gml:posList>48.2 10.4 48.3 10.5 48.4 10.4 48.2 10.4</gml:posList></gml:LinearRing></gml:interior></gml:Polygon></app:geom></app:parks></wfs:member>
</wfs:FeatureCollection>"#;

    #[test]
    fn reads_wfs_gml3() {
        let layers = read("w.gml", WFS.as_bytes()).unwrap();
        let names: Vec<&str> = layers.iter().map(|(d, _)| d.name.as_str()).collect();
        assert_eq!(names, ["w roads", "w parks"]);
        let roads = &layers[0].0;
        assert_eq!(roads.geometry_type, GeometryType::Polyline);
        assert_eq!(roads.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["gml_id", "name", "lanes"]);
        assert_eq!(roads.fields[2].ty, FieldType::Integer);
        // URN axis order: latitude first, swapped to longitude first.
        assert_eq!(roads.features[0].geometry, Some(Geometry::Line(vec![vec![Coord::new(10.0, 48.0), Coord::new(11.0, 49.0)]])));
        assert_eq!(
            roads.features[1].geometry,
            Some(Geometry::Line(vec![vec![Coord::new(10.0, 48.0), Coord::new(12.0, 49.0)]])),
            "3D posList"
        );
        assert!(matches!(&layers[1].0.features[0].geometry, Some(Geometry::Polygon(p)) if p[0].len() == 2));
        assert_eq!(layers[0].1.skipped, 0, "a feature's boundedBy is no failed geometry");
    }

    #[test]
    fn reads_gml2_and_projected() {
        // OGR-style GML 2: gml:coordinates, EPSG:32633 (easting/northing), a MultiPolygon.
        let ogr = r#"<ogr:FeatureCollection xmlns:ogr="http://ogr.maptools.org/" xmlns:gml="http://www.opengis.net/gml">
<gml:featureMember><ogr:sites fid="sites.0"><ogr:geometryProperty><gml:Point srsName="EPSG:32633"><gml:coordinates>500000,4649776</gml:coordinates></gml:Point></ogr:geometryProperty><ogr:id>1</ogr:id></ogr:sites></gml:featureMember>
<gml:featureMember><ogr:sites fid="sites.1"><ogr:geometryProperty><gml:MultiPolygon srsName="EPSG:32633"><gml:polygonMember><gml:Polygon><gml:outerBoundaryIs><gml:LinearRing><gml:coordinates>500000,4649776 500100,4649776 500100,4649876 500000,4649776</gml:coordinates></gml:LinearRing></gml:outerBoundaryIs></gml:Polygon></gml:polygonMember></gml:MultiPolygon></ogr:geometryProperty><ogr:id>2</ogr:id></ogr:sites></gml:featureMember>
<gml:featureMember><ogr:sites fid="sites.2"><ogr:geometryProperty><gml:Point><gml:coordinates>x,y</gml:coordinates></gml:Point></ogr:geometryProperty><ogr:id>3</ogr:id></ogr:sites></gml:featureMember>
</ogr:FeatureCollection>"#;
        let layers = read("s.gml", ogr.as_bytes()).unwrap();
        assert_eq!(layers.len(), 1);
        let (ds, rep) = &layers[0];
        assert_eq!(ds.name, "s");
        let Some(Geometry::Point(c)) = &ds.features[0].geometry else { panic!() };
        assert!((c.x - 15.0).abs() < 1e-6 && (c.y - 42.0).abs() < 0.01, "{c:?}");
        assert_eq!(rep.skipped, 1);
        assert_eq!(srs("EPSG:4326").map(|s| s.1), Some(false));
        assert_eq!(srs("http://www.opengis.net/def/crs/EPSG/0/4326").map(|s| s.1), Some(true));
        assert_eq!(srs("urn:ogc:def:crs:EPSG::32633").map(|s| s.1), Some(false), "projected: easting first");
    }

    #[test]
    fn three_d_positions_and_nested_members() {
        // A 3D `pos` without srsDimension, and a property named `member` inside the feature: one
        // feature, its point read.
        let gml = r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="http://example.com/app">
<wfs:member><app:wells gml:id="w.1"><app:member><app:person>Ann</app:person></app:member>
  <app:geom><gml:Point srsName="EPSG:4326"><gml:pos>10 48 312</gml:pos></gml:Point></app:geom></app:wells></wfs:member>
</wfs:FeatureCollection>"#;
        let layers = read("n.gml", gml.as_bytes()).unwrap();
        assert_eq!(layers.len(), 1, "{:?}", layers.iter().map(|l| &l.0.name).collect::<Vec<_>>());
        let (ds, rep) = &layers[0];
        assert_eq!((ds.features.len(), rep.skipped), (1, 0));
        assert_eq!(ds.features[0].geometry, Some(Geometry::Point(Coord::new(10.0, 48.0))));
    }

    #[test]
    fn unread_geometry_kinds_are_counted() {
        let gml = r#"<wfs:FeatureCollection xmlns:wfs="http://www.opengis.net/wfs/2.0" xmlns:gml="http://www.opengis.net/gml/3.2" xmlns:app="http://example.com/app">
<wfs:member><app:roads gml:id="r.1"><app:name>A</app:name><app:geom><gml:CompositeCurve><gml:curveMember/></gml:CompositeCurve></app:geom></app:roads></wfs:member>
<wfs:member><app:roads gml:id="r.2"><app:name>B</app:name><app:geom><gml:LineString><gml:posList>10 48 11 49</gml:posList></gml:LineString></app:geom></app:roads></wfs:member>
</wfs:FeatureCollection>"#;
        let layers = read("c.gml", gml.as_bytes()).unwrap();
        let (ds, rep) = &layers[0];
        assert_eq!((ds.features.len(), rep.skipped), (2, 1));
        assert!(ds.features[0].geometry.is_none());
    }

    #[test]
    fn junk_is_refused() {
        assert!(read("x.gml", b"<a/>").is_err());
        assert!(read("x.gml", b"\xff").is_err());
        for cut in (0..WFS.len()).step_by(53) {
            let _ = read("x.gml", WFS.as_bytes().get(..cut).unwrap_or_default());
        }
    }
}
