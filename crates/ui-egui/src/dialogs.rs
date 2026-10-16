//! Dialogs (Select By Attributes, Go To XY, bookmarks, fields, definition query, rename) and
//! native file pickers.

use egui::{Align2, vec2};
use rustgis_engine::Session;
use serde_json::{Value, json};

use crate::RustGisApp;
use crate::theme::{Tokens, semibold};

#[derive(Default)]
pub struct Dialogs {
    pub open: Option<&'static str>,
    pub layer: String,
    pub text: String,
    pub a: String,
    pub b: String,
    pub c: String,
    pub mode: &'static str,
    pub flag: bool,
    pub error: Option<String>,
    pub info: Option<String>,
    /// Choices found for the dialog (Add From Database's tables; Add Delimited Text's columns).
    pub list: Vec<String>,
    /// Add Delimited Text's file, picked before the dialog opens.
    pub csv: Option<CsvFile>,
}

/// A file picked for Add Delimited Text.
pub struct CsvFile {
    pub name: String,
    pub source: CsvSource,
}

/// Where a file picked for Add Delimited Text is: on disk, or (in the browser) its bytes, kept
/// until it is added.
pub enum CsvSource {
    Path(String),
    Bytes(Vec<u8>),
}

impl Dialogs {
    pub fn open(&mut self, which: &str, s: &Session) -> Result<(), String> {
        let active = s.active_layer.and_then(|id| s.map().layer(id));
        let name: &'static str = match which {
            "selectByAttributes" => "selectByAttributes",
            "goToXY" => "goToXY",
            "bookmark" => "bookmark",
            "addField" => "addField",
            "calculateField" => "calculateField",
            "definitionQuery" => "definitionQuery",
            "renameLayer" => "renameLayer",
            "renameMap" => "renameMap",
            "renameLayout" => "renameLayout",
            "coordinateSystem" => "coordinateSystem",
            "graphicText" => "graphicText",
            "camera" => "camera",
            "addTiles" => "addTiles",
            "addCsv" => "addCsv",
            "findPlace" => "findPlace",
            "geocode" => "geocode",
            "addUrl" => "addUrl",
            "addService" => "addService",
            "addDatabase" => "addDatabase",
            "stac" => "stac",
            "s3" => "s3",
            "osm" => "osm",
            "boundaries" => "boundaries",
            "overture" => "overture",
            "sourcecoop" => "sourcecoop",
            "opendata" => "opendata",
            "zarr" => "zarr",
            "exportLayer" => "exportLayer",
            "saveProject" => "saveProject",
            other => return Err(format!("unknown dialog '{other}'")),
        };
        if matches!(name, "addField" | "calculateField" | "definitionQuery" | "renameLayer" | "exportLayer") && active.is_none() {
            return Err("select a layer in the Layers pane first".into());
        }
        if name == "exportLayer" && active.is_some_and(|l| l.is_raster()) {
            return Err("Export Features needs a vector layer (Data › Export Raster writes a raster)".into());
        }
        if name == "selectByAttributes" && s.map().layers.is_empty() {
            return Err("the map has no layers".into());
        }
        *self = Dialogs { open: Some(name), mode: "new", ..Dialogs::default() };
        self.layer = active.or_else(|| s.map().layers.first()).map(|l| l.name.clone()).unwrap_or_default();
        match name {
            "coordinateSystem" => {
                self.text = s.map().projection.code();
                // An EPSG code goes in the code box; presets are picked from the list.
                let is_code = self.text.to_ascii_lowercase().starts_with("epsg");
                self.a = if is_code { self.text.trim_start_matches(|c: char| !c.is_ascii_digit()).to_string() } else { String::new() };
            }
            "goToXY" => {
                let c = s.map().center_lonlat();
                self.a = format!("{:.4}", c.x);
                self.b = format!("{:.4}", c.y);
            }
            "bookmark" => self.text = format!("Bookmark {}", s.map().bookmarks.len() + 1),
            "camera" => {
                let c = s.map().center_lonlat();
                self.a = format!("{:.5}", c.x);
                self.b = format!("{:.5}", c.y);
                self.c = format!("{:.0}", s.scale());
            }
            "graphicText" => {
                self.text = "Text".into();
                self.c = "14".into();
            }
            "addField" => self.b = "Double".into(),
            "calculateField" => self.a = active.and_then(|l| l.dataset.fields.last()).map(|f| f.name.clone()).unwrap_or_default(),
            "definitionQuery" => self.text = active.and_then(|l| l.definition_query.clone()).unwrap_or_default(),
            "renameLayer" => self.text = active.map(|l| l.name.clone()).unwrap_or_default(),
            // A file name can't hold a folder separator, which a project name may.
            "saveProject" => self.text = format!("{}.rgis", s.project.name.replace(['/', '\\'], "-")),
            "exportLayer" => self.mode = "geojson",
            // A table of addresses is what's usually geocoded: picked by its id (`flag` says the
            // choice is a table), since names can repeat.
            "geocode" => {
                if let Some(t) = s.project.tables.last() {
                    (self.layer, self.flag) = (t.id.to_string(), true);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub fn show(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(which) = app.dialogs.open else { return };
    let title = match which {
        "selectByAttributes" => "Select By Attributes",
        "goToXY" => "Go To XY",
        "bookmark" => "New Bookmark",
        "addField" => "Add Field",
        "calculateField" => "Calculate Field",
        "definitionQuery" => "Definition Query",
        "coordinateSystem" => "Coordinate System",
        "graphicText" => "Add Text",
        "camera" => "Camera",
        "addTiles" => "Add Tile Layer",
        "addCsv" => "Add Delimited Text",
        "findPlace" => "Find Place",
        "geocode" => "Geocode Addresses",
        "addUrl" => "Add Data From URL",
        "addService" => "Add From Service",
        "addDatabase" => "Add From Database",
        "stac" => "STAC Search",
        "s3" => "S3 Buckets",
        "osm" => "OpenStreetMap",
        "boundaries" => "Administrative Boundaries",
        "overture" => "Overture Maps",
        "sourcecoop" => "Source Cooperative",
        "opendata" => "Search Open Data",
        "zarr" => "Add Zarr Variable",
        "exportLayer" => "Export Features",
        "saveProject" => "Save Project As",
        "renameMap" => "Rename Map",
        "renameLayout" => "Rename Layout",
        _ => "Rename Layer",
    };
    let t = Tokens::get(ctx);
    let mut open = true;
    let mut close = false;
    // Opens centred and can be moved; each dialog remembers where it was put.
    egui::Window::new(tl!(title))
        .id(egui::Id::new(("dialog", which)))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center() + vec2(0.0, -40.0))
        .show(ctx, |ui| {
        ui.set_width(match which {
            "coordinateSystem" | "zarr" => 660.0,
            "addTiles" | "addUrl" | "addService" | "stac" | "s3" | "sourcecoop" | "opendata" => 560.0,
            _ => 440.0,
        });
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        match which {
            "selectByAttributes" => close = select_by_attributes(app, ui, &t),
            "goToXY" => {
                egui::Grid::new("xy").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                    ui.label(tl!("Longitude (X)"));
                    ui.add(egui::TextEdit::singleline(&mut app.dialogs.a).desired_width(200.0));
                    ui.end_row();
                    ui.label(tl!("Latitude (Y)"));
                    ui.add(egui::TextEdit::singleline(&mut app.dialogs.b).desired_width(200.0));
                    ui.end_row();
                    ui.label(tl!("Scale 1:"));
                    ui.add(egui::TextEdit::singleline(&mut app.dialogs.c).hint_text(tl!("keep current")).desired_width(200.0));
                    ui.end_row();
                });
                close = buttons(app, ui, "Go", |app| {
                    let mut p = json!({"lon": app.dialogs.a.trim(), "lat": app.dialogs.b.trim()});
                    if let Ok(s) = app.dialogs.c.trim().replace(',', "").parse::<f64>() {
                        p["scale"] = json!(s);
                    }
                    app.run("map.goToXY", p).map(|_| ())
                });
            }
            "coordinateSystem" => close = coordinate_system(app, ui, &t),
            "addTiles" => close = add_tiles(app, ui),
            "addCsv" => close = add_csv(app, ui),
            "findPlace" => close = crate::geocodeui::find_place(app, ui),
            "geocode" => close = crate::geocodeui::geocode(app, ui),
            "addUrl" => close = add_url(app, ui),
            "addService" => close = add_service(app, ui),
            "addDatabase" => close = add_database(app, ui),
            "stac" => crate::stacui::dialog(app, ui),
            "s3" => crate::s3ui::dialog(app, ui),
            "osm" => crate::portalsui::osm(app, ui),
            "boundaries" => crate::portalsui::boundaries(app, ui),
            "overture" => crate::catalogsui::overture(app, ui),
            "sourcecoop" => crate::catalogsui::sourcecoop(app, ui),
            "opendata" => crate::opendataui::dialog(app, ui),
            "zarr" => close = crate::zarrui::dialog(app, ui),
            "exportLayer" => close = export_layer(app, ui),
            "camera" => {
                egui::Grid::new("cam").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                    for (label, v) in [("Longitude", &mut app.dialogs.a), ("Latitude", &mut app.dialogs.b), ("Scale 1:", &mut app.dialogs.c)] {
                        ui.label(tl!(label));
                        ui.add(egui::TextEdit::singleline(v).desired_width(160.0));
                        ui.end_row();
                    }
                    ui.label(tl!("Projection"));
                    ui.label(app.session.map().projection.label());
                    ui.end_row();
                });
                close = buttons(app, ui, "Apply", |app| {
                    // Empty keeps the current value; anything else must be a number.
                    let n = |s: &str, what: &str| -> Result<Option<f64>, String> {
                        let s = s.trim();
                        if s.is_empty() {
                            return Ok(None);
                        }
                        s.parse::<f64>().ok().filter(|v| v.is_finite()).map(Some).ok_or_else(|| crate::i18n::fmt("{what} must be a number", &[("what", tl!(what))]))
                    };
                    let (lon, lat, scale) = (n(&app.dialogs.a, "Longitude")?, n(&app.dialogs.b, "Latitude")?, n(&app.dialogs.c, "Scale")?);
                    app.run("map.camera", json!({"lon": lon, "lat": lat, "scale": scale})).map(|_| ())
                });
            }
            "graphicText" => {
                ui.label(tl!("Text"));
                ui.add(egui::TextEdit::multiline(&mut app.dialogs.text).desired_rows(2).desired_width(f32::INFINITY));
                ui.horizontal_wrapped(|ui| {
                    ui.label(egui::RichText::new(tl!("Dynamic text:")).small().weak());
                    for (token, what) in rustgis_engine::map::graphics::TOKENS {
                        if ui.small_button(*token).on_hover_text(tl!(*what)).clicked() {
                            app.dialogs.text.push_str(token);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(tl!("Size"));
                    ui.add(egui::TextEdit::singleline(&mut app.dialogs.c).desired_width(50.0));
                });
                close = buttons(app, ui, "Add", |app| {
                    let (lon, lat) = (app.dialogs.a.parse::<f64>().unwrap_or(f64::NAN), app.dialogs.b.parse::<f64>().unwrap_or(f64::NAN));
                    let size = app.dialogs.c.trim().parse::<f64>().unwrap_or(14.0);
                    let r = app.run("graphics.add", json!({"shape": "text", "points": [[lon, lat]], "text": app.dialogs.text, "fontSize": size}));
                    if r.is_ok() && !app.session.keep_tool {
                        app.session.tool = rustgis_engine::Tool::Explore;
                    }
                    r.map(|_| ())
                });
            }
            "bookmark" => {
                ui.label(tl!("Name"));
                ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).desired_width(f32::INFINITY));
                close = buttons(app, ui, "Create", |app| app.run("bookmark.add", json!({"name": app.dialogs.text})).map(|_| ()));
            }
            "addField" => {
                egui::Grid::new("af").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                    ui.label(tl!("Field name"));
                    ui.add(egui::TextEdit::singleline(&mut app.dialogs.a).desired_width(220.0));
                    ui.end_row();
                    ui.label(tl!("Data type"));
                    egui::ComboBox::from_id_salt("af_type").selected_text(app.dialogs.b.clone()).show_ui(ui, |ui| {
                        for ty in ["Long", "Double", "Text", "Boolean"] {
                            ui.selectable_value(&mut app.dialogs.b, ty.to_string(), ty);
                        }
                    });
                    ui.end_row();
                });
                close = buttons(app, ui, "Add", |app| app.run("edit.addField", json!({"name": app.dialogs.a, "type": app.dialogs.b})).map(|_| ()));
            }
            "calculateField" => {
                let fields: Vec<String> = app.session.active_layer.and_then(|id| app.session.map().layer(id)).map(|l| l.dataset.fields.iter().map(|f| f.name.clone()).collect()).unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label(tl!("Field"));
                    egui::ComboBox::from_id_salt("cf_field").selected_text(app.dialogs.a.clone()).width(220.0).show_ui(ui, |ui| {
                        for f in &fields {
                            ui.selectable_value(&mut app.dialogs.a, f.clone(), f);
                        }
                    });
                });
                ui.label(format!("{} =", app.dialogs.a));
                ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).hint_text(tl!("e.g.  pop_est / 1000000   or   'Africa'   or   name")).desired_width(f32::INFINITY));
                ui.label(egui::RichText::new(tl!("A value, a field, or two of them joined by + − × ÷. Applies to the selected rows, or every row when none is selected.")).small().weak());
                close = buttons(app, ui, "Calculate", |app| app.run("edit.calculateField", json!({"field": app.dialogs.a, "expression": app.dialogs.text})).map(|_| ()));
            }
            "definitionQuery" => {
                ui.label(egui::RichText::new(crate::i18n::fmt("Only features matching this expression draw in {layer}.", &[("layer", &app.dialogs.layer)])).weak());
                ui.add(egui::TextEdit::multiline(&mut app.dialogs.text).hint_text(tl!("e.g. continent = 'Europe'")).desired_rows(3).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                field_chips(app, ui);
                close = buttons(app, ui, "Apply", |app| app.run("layer.definitionQuery", json!({"where": app.dialogs.text})).map(|_| ()));
            }
            "saveProject" => close = save_project(app, ui),
            // `a` is the map or layout being renamed; an empty or unchanged name just closes.
            "renameMap" | "renameLayout" => {
                ui.label(tl!("Name"));
                ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).desired_width(f32::INFINITY));
                close = buttons(app, ui, "Rename", |app| {
                    let (old, new) = (app.dialogs.a.clone(), app.dialogs.text.trim().to_string());
                    if new.is_empty() || new == old {
                        return Ok(());
                    }
                    let r = if which == "renameMap" {
                        app.run("map.rename", json!({"map": old, "name": new}))
                    } else {
                        app.run("layout.rename", json!({"layout": old, "newName": new}))
                    };
                    r.map(|_| ())
                });
            }
            _ => {
                ui.label(tl!("Name"));
                ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).desired_width(f32::INFINITY));
                close = buttons(app, ui, "Rename", |app| app.run("layer.rename", json!({"name": app.dialogs.text})).map(|_| ()));
            }
        }
        if let Some(e) = &app.dialogs.error {
            ui.label(egui::RichText::new(e).color(t.red));
        }
        if let Some(i) = &app.dialogs.info {
            ui.label(egui::RichText::new(i).color(t.green));
        }
    });
    if !open || close {
        app.dialogs.open = None;
    }
}

/// Save Project As in the browser, which has no save dialog: the file name to download the
/// project as. The browser decides where downloads go.
fn save_project(app: &mut RustGisApp, ui: &mut egui::Ui) -> bool {
    ui.label(tl!("File name"));
    let mut edit = egui::TextEdit::singleline(&mut app.dialogs.text).desired_width(f32::INFINITY).show(ui);
    // Opened: the name is selected, so typing replaces it.
    if app.dialogs.mode == "new" {
        app.dialogs.mode = "";
        edit.response.request_focus();
        let end = app.dialogs.text.chars().count();
        edit.state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(end))));
        edit.state.store(ui.ctx(), edit.response.id);
    }
    ui.label(egui::RichText::new(tl!("The project downloads as this file, to the folder your browser saves downloads in.")).small().weak());
    buttons(app, ui, "Save", |app| {
        let file = app.dialogs.text.clone();
        app.save_download(&file)
    })
}

/// The formats Export Features writes: (name, extension). Format names aren't translated.
pub const EXPORT_FORMATS: &[(&str, &str)] = &[
    ("GeoJSON", "geojson"),
    ("GeoPackage", "gpkg"),
    ("GeoParquet", "parquet"),
    ("Shapefile", "shp"),
    ("FlatGeobuf", "fgb"),
    ("CSV", "csv"),
    ("KML", "kml"),
    ("KMZ", "kmz"),
    ("GPX", "gpx"),
];

/// Export Features: the active layer, in a format picked here (a file's type isn't changed by
/// the filter chosen in a save dialog, and the web app has no save dialog).
fn export_layer(app: &mut RustGisApp, ui: &mut egui::Ui) -> bool {
    let web = app.services.download.is_some();
    egui::Grid::new("export_layer").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        ui.label(tl!("Layer"));
        ui.label(&app.dialogs.layer);
        ui.end_row();
        ui.label(tl!("Format"));
        let shown = EXPORT_FORMATS.iter().find(|(_, e)| *e == app.dialogs.mode).map(|(l, _)| *l).unwrap_or("GeoJSON");
        egui::ComboBox::from_id_salt("export_format").selected_text(shown).width(220.0).show_ui(ui, |ui| {
            // GeoPackages are written with SQLite, which the web app doesn't have.
            for (label, ext) in EXPORT_FORMATS.iter().filter(|(_, e)| !(web && *e == "gpkg")) {
                ui.selectable_value(&mut app.dialogs.mode, *ext, *label);
            }
        });
        ui.end_row();
    });
    if app.dialogs.mode == "shp" && web {
        ui.label(egui::RichText::new(tl!("The Shapefile's files download as one zip.")).small().weak());
    }
    buttons(app, ui, if web { "Download" } else { "Export…" }, |app| {
        app.run("ui.exportLayer", json!({"format": app.dialogs.mode})).map(|_| ())
    })
}

/// Add Delimited Text's separators: (code, name).
const SEPARATORS: [(&str, &str); 5] = [("auto", "Automatic"), (",", "Comma"), (";", "Semicolon"), ("\t", "Tab"), ("|", "Pipe")];

/// Opens Add Delimited Text for a file picked: its columns, with the ones found by name picked.
pub fn open_csv(app: &mut RustGisApp, name: &str, source: CsvSource) -> Result<Value, String> {
    app.run("ui.dialog", json!({"name": "addCsv"}))?;
    let file = CsvFile { name: name.to_string(), source };
    app.dialogs.mode = "auto";
    show_columns(&mut app.dialogs, csv_columns(&file, "auto"));
    app.dialogs.csv = Some(file);
    Ok(Value::Null)
}

/// The columns of a file picked for Add Delimited Text, split at `separator` (a code of
/// [`SEPARATORS`]).
fn csv_columns(file: &CsvFile, separator: &str) -> Result<Value, String> {
    let p = if separator == "auto" { json!({}) } else { json!({"delimiter": separator}) };
    match &file.source {
        CsvSource::Path(path) => rustgis_engine::cmd::layer::columns_in_file(path, &p),
        CsvSource::Bytes(bytes) => rustgis_engine::cmd::layer::columns_of(&file.name, bytes, &p),
    }
}

/// Puts the columns read (`info.fileColumns`' answer) in the dialog, the ones found picked; an
/// error leaves none.
fn show_columns(d: &mut Dialogs, found: Result<Value, String>) {
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    match found {
        Ok(v) => {
            d.list = v["columns"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
            (d.a, d.b, d.c) = (text(&v["x"]), text(&v["y"]), text(&v["wkt"]));
            if let Some((code, _)) = SEPARATORS.iter().find(|(c, _)| v["delimiter"].as_str() == Some(*c)) {
                d.mode = *code;
            }
            d.error = None;
        }
        Err(e) => {
            d.list.clear();
            (d.a, d.b, d.c) = (String::new(), String::new(), String::new());
            d.error = Some(e);
        }
    }
}

/// After the columns are read again: the x/y columns (or the WKT column) picked before stay picked
/// when the file still has them; otherwise the ones found are.
fn keep_picked(d: &mut Dialogs, [x, y, wkt]: [String; 3]) {
    let has = |c: &String| !c.is_empty() && d.list.contains(c);
    let (xy, w) = (has(&x) && has(&y), has(&wkt));
    if xy {
        (d.a, d.b, d.c) = (x, y, String::new());
    } else if w {
        (d.a, d.b, d.c) = (String::new(), String::new(), wkt);
    }
}

/// Add Delimited Text, for the file picked: which of its columns hold the coordinates (or WKT
/// geometry), their coordinate system and the separator.
fn add_csv(app: &mut RustGisApp, ui: &mut egui::Ui) -> bool {
    let Some(name) = app.dialogs.csv.as_ref().map(|f| f.name.clone()) else {
        // Opened without a file (`ui.dialog`): the file first.
        ui.label(tl!("Choose a delimited text file or a spreadsheet, then pick its columns."));
        let close = buttons(app, ui, "Choose File…", |app| app.run("ui.addDelimited", Value::Null).map(|_| ()));
        // On the desktop the dialog is open again, for the file picked.
        return close && app.dialogs.csv.is_none();
    };
    let sheet = rustgis_engine::data::sheets::is_sheet_name(&name);
    let shown = std::path::Path::new(&name).file_name().and_then(|n| n.to_str()).unwrap_or(&name).to_string();
    ui.label(egui::RichText::new(shown).strong());
    ui.label(egui::RichText::new(tl!("Columns named like longitude/latitude, easting/northing or geometry are picked for you.")).weak());
    let mut reread = false;
    egui::Grid::new("csv").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        let columns = app.dialogs.list.clone();
        for (i, label) in ["X column", "Y column", "WKT column"].into_iter().enumerate() {
            ui.label(tl!(label));
            let d = &mut app.dialogs;
            let v = match i {
                0 => &mut d.a,
                1 => &mut d.b,
                _ => &mut d.c,
            };
            let before = v.clone();
            let selected = if v.is_empty() { tl!("None").to_string() } else { v.clone() };
            egui::ComboBox::from_id_salt(("csv_column", i)).selected_text(selected).width(240.0).show_ui(ui, |ui| {
                ui.selectable_value(v, String::new(), tl!("None"));
                for c in &columns {
                    ui.selectable_value(v, c.clone(), c.as_str());
                }
            });
            // x/y columns or a WKT column, not both.
            if *v != before && !v.is_empty() {
                if i == 2 {
                    (d.a, d.b) = (String::new(), String::new());
                } else {
                    d.c.clear();
                }
            }
            ui.end_row();
        }
        ui.label(tl!("Coordinate system"));
        ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).hint_text("EPSG:4326").desired_width(240.0));
        ui.end_row();
        // A sheet has no separator.
        if !sheet {
            ui.label(tl!("Separator"));
            let before = app.dialogs.mode;
            let shown = SEPARATORS.iter().find(|(c, _)| *c == app.dialogs.mode).map(|(_, l)| tl!(*l).to_string()).unwrap_or_default();
            egui::ComboBox::from_id_salt("csv_sep").selected_text(shown).width(240.0).show_ui(ui, |ui| {
                for (code, label) in SEPARATORS {
                    ui.selectable_value(&mut app.dialogs.mode, code, tl!(label));
                }
            });
            reread = app.dialogs.mode != before;
            ui.end_row();
        }
    });
    // Another separator splits the header differently: its columns are read again.
    if reread && let Some(file) = &app.dialogs.csv {
        let found = csv_columns(file, app.dialogs.mode);
        let picked = [app.dialogs.a.clone(), app.dialogs.b.clone(), app.dialogs.c.clone()];
        show_columns(&mut app.dialogs, found);
        keep_picked(&mut app.dialogs, picked);
    }
    buttons(app, ui, "Add", |app| {
        let mut p = json!({});
        for (key, v) in [("x", &app.dialogs.a), ("y", &app.dialogs.b), ("wkt", &app.dialogs.c), ("crs", &app.dialogs.text)] {
            if !v.trim().is_empty() {
                p[key] = json!(v.trim());
            }
        }
        if app.dialogs.mode != "auto" && !sheet {
            p["delimiter"] = json!(app.dialogs.mode);
        }
        let file = app.dialogs.csv.take().ok_or("choose a file first")?;
        let r = match &file.source {
            CsvSource::Path(path) => {
                p["path"] = json!(path);
                app.run("layer.addData", p)
            }
            CsvSource::Bytes(bytes) => app.open_bytes_with(&file.name, bytes, Some(p)),
        };
        // Kept for another try.
        if r.is_err() {
            app.dialogs.csv = Some(file);
        }
        r.map(|_| ())
    })
}

/// How many of a service's layers Add From Service lists.
const SHOWN_LAYERS: usize = 500;

/// Add From Service: a WMS, WMTS, WFS, WCS, OGC API Features or ArcGIS REST service by its address
/// (or a catalogue searched for them, `cswui`).
/// Connect fetches its capabilities in the background; its layers are then listed, each with an
/// Add button (map layers become tile layers, features are downloaded).
fn add_service(app: &mut RustGisApp, ui: &mut egui::Ui) -> bool {
    const KINDS: [(&str, &str); 8] = [
        ("auto", "Automatic"),
        ("wms", "WMS"),
        ("wmts", "WMTS"),
        ("wfs", "WFS"),
        ("wcs", "WCS"),
        ("ogcapi", "OGC API Features and Tiles"),
        ("arcgis", "ArcGIS REST"),
        ("csw", "Catalogue search (CSW)"),
    ];
    if app.dialogs.mode == "new" {
        app.dialogs.mode = "auto";
    }
    let kinds = |app: &mut RustGisApp, ui: &mut egui::Ui| {
        ui.label(tl!("Type"));
        let shown = KINDS.iter().find(|(c, _)| *c == app.dialogs.mode).map(|(_, l)| tl!(*l).to_string()).unwrap_or_default();
        egui::ComboBox::from_id_salt("service_kind").selected_text(shown).width(240.0).show_ui(ui, |ui| {
            for (code, label) in KINDS {
                ui.selectable_value(&mut app.dialogs.mode, code, tl!(label));
            }
        });
    };
    // A catalogue: searched for records, whose links connect here or download.
    if app.dialogs.mode == "csw" {
        ui.label(egui::RichText::new(tl!("A catalogue's (CSW 2.0.2) address: search its records by words and, if you like, the map's area, then add the services and files they link to.")).weak());
        crate::cswui::form(app, ui, kinds);
        return false;
    }
    ui.label(egui::RichText::new(tl!("A map, feature or coverage service's address: WMS, WMTS, WFS, WCS, OGC API Features or Tiles, or an ArcGIS FeatureServer, MapServer, ImageServer or VectorTileServer.")).weak());
    let mut connect = false;
    egui::Grid::new("service").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        ui.label(tl!("Address"));
        let r = ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).hint_text("https://…/wms").desired_width(420.0));
        connect |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.end_row();
        kinds(app, ui);
        ui.end_row();
    });
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        connect |= crate::widgets::dialog_button(ui, tl!("Connect"), Some(t.accent), vec2(96.0, 28.0)).clicked();
    });
    if connect {
        let url = app.dialogs.text.trim().to_string();
        app.dialogs.error = None;
        app.dialogs.info = None;
        if url.is_empty() {
            app.dialogs.error = Some(tl!("Give the service's web address").to_string());
        } else if let Err(e) = app.run("ui.addService", json!({"url": url, "type": app.dialogs.mode})) {
            app.dialogs.error = Some(e);
        }
    }
    // The service: connecting, an error, or its layers.
    let mut add: Option<Value> = None;
    if let Some(conn) = &app.ui.service {
        ui.separator();
        if let Some(e) = &conn.error {
            ui.colored_label(t.red, e);
        } else if let Some(v) = &conn.listing {
            let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
            let title = v.get("title").and_then(Value::as_str).unwrap_or("");
            // Borrowed, not cloned: the dialog draws every frame.
            let none = Vec::new();
            let layers = v.get("layers").and_then(Value::as_array).unwrap_or(&none);
            ui.label(
                egui::RichText::new(crate::i18n::fmt(
                    "{kind} {title}: {count} layers",
                    &[("kind", kind), ("title", title), ("count", &layers.len().to_string())],
                ))
                .strong(),
            );
            for n in v.get("notes").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                ui.label(egui::RichText::new(n).small().weak());
            }
            if layers.len() > SHOWN_LAYERS {
                ui.label(
                    egui::RichText::new(crate::i18n::fmt("The first {n} are listed.", &[("n", &SHOWN_LAYERS.to_string())])).small().weak(),
                );
            }
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                for l in layers.iter().take(SHOWN_LAYERS) {
                    ui.horizontal(|ui| {
                        if ui.small_button(tl!("Add")).clicked() {
                            add = Some(l.clone());
                        }
                        let name = l.get("name").and_then(Value::as_str).unwrap_or("");
                        ui.label(l.get("title").and_then(Value::as_str).unwrap_or(name)).on_hover_text(name);
                        let what = match l.get("offer").and_then(|o| o.get("type")).and_then(Value::as_str) {
                            Some("features") => tl!("features"),
                            Some("style") => tl!("style"),
                            Some("coverage") => tl!("coverage"),
                            _ => tl!("map"),
                        };
                        ui.label(egui::RichText::new(what).small().weak());
                    });
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(crate::i18n::fmt("Connecting to {url}…", &[("url", &conn.url)]));
            });
        }
    }
    if let (Some(l), Some(conn)) = (add, &app.ui.service) {
        let (url, kind, text) = (conn.url.clone(), conn.kind, conn.text.clone().unwrap_or_default());
        let name = l.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let title = l.get("title").and_then(Value::as_str).unwrap_or(&name).to_string();
        let offer = l.get("offer").cloned().unwrap_or_default();
        // Features are downloaded in the background, as From URL… does; map layers are added now.
        let r = if offer.get("type").and_then(Value::as_str) == Some("features") {
            let items = offer.get("url").and_then(Value::as_str).unwrap_or("").to_string();
            app.run("ui.addUrl", json!({"url": items, "name": title, "service": true})).map(|_| tl!("Downloading…"))
        } else if offer.get("type").and_then(Value::as_str) == Some("coverage") {
            // A coverage: its description first, off the frame (poll_describe asks for it).
            let describe = offer.get("describe").and_then(Value::as_str).unwrap_or("").to_string();
            let mut p = json!({"url": url, "layer": name, "text": text, "name": title});
            if kind != "auto" {
                p["type"] = json!(kind);
            }
            let slot = crate::fetch_into(&describe);
            if let Some(c) = &mut app.ui.service {
                c.describing = Some((p, slot));
            }
            Ok(tl!("Describing"))
        } else {
            let mut p = json!({"url": url, "layer": name, "text": text});
            if kind != "auto" {
                p["type"] = json!(kind);
            }
            match l.get("tilesets").and_then(Value::as_str) {
                // An OGC API layer: its tileset's advertised template is read first, off the
                // frame (poll_tiles_resolve adds the layer).
                Some(list) => {
                    p["name"] = json!(title);
                    let fallback = offer.get("url").and_then(Value::as_str).unwrap_or("").to_string();
                    let vector = offer.get("type").and_then(Value::as_str) == Some("vectorTiles");
                    let slot = crate::fetch_into(list);
                    if let Some(c) = &mut app.ui.service {
                        c.resolving = Some(crate::TilesResolve {
                            params: p,
                            list: list.to_string(),
                            doc: list.to_string(),
                            read: 0,
                            vector,
                            fallback,
                            slot,
                        });
                    }
                    Ok(tl!("Reading the tileset of"))
                }
                None => app.run("service.add", p).map(|_| tl!("Added")),
            }
        };
        match r {
            Ok(m) => app.dialogs.info = Some(format!("{m} {title}")),
            Err(e) => app.dialogs.error = Some(e),
        }
        app.map.invalidate();
    }
    if let Some(i) = &app.dialogs.info {
        ui.label(egui::RichText::new(i).small());
    }
    false
}

/// Add Data From URL: a file on the web, downloaded and added (a PMTiles archive is read in place).
fn add_url(app: &mut RustGisApp, ui: &mut egui::Ui) -> bool {
    let hint = if cfg!(target_arch = "wasm32") {
        tl!(
            "GeoJSON, CSV, GeoParquet, FlatGeobuf, GeoPackage, zipped Shapefiles, KML, GPX or GeoTIFF, up to 256 MB; the server must allow requests from web pages (CORS)."
        )
    } else {
        tl!("GeoJSON, CSV, GeoParquet, FlatGeobuf, GeoPackage, zipped Shapefiles, KML, GPX or GeoTIFF, up to 512 MB.")
    };
    ui.label(egui::RichText::new(hint).weak());
    egui::Grid::new("url").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        ui.label(tl!("Address"));
        ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).hint_text("https://…/data.geojson").desired_width(400.0));
        ui.end_row();
        ui.label(tl!("Name"));
        ui.add(egui::TextEdit::singleline(&mut app.dialogs.a).hint_text(tl!("from the address")).desired_width(400.0));
        ui.end_row();
    });
    buttons(app, ui, "Add", |app| {
        let url = app.dialogs.text.trim().to_string();
        if url.is_empty() {
            return Err(tl!("Give the file's web address").to_string());
        }
        app.run("ui.addUrl", json!({"url": url, "name": app.dialogs.a.trim()})).map(|_| ())
    })
}

/// Add From Database: a PostgreSQL/PostGIS connection (or a DuckDB file), its password (kept for
/// the session only) and one table or all of them.
fn add_database(app: &mut RustGisApp, ui: &mut egui::Ui) -> bool {
    ui.label(egui::RichText::new(tl!("A PostgreSQL/PostGIS connection (host=… port=… dbname=… user=…, or a postgresql:// address) or a DuckDB file. PostGIS tables are read live, large ones a view at a time; the password is kept until RustGIS closes, never in the project.")).weak());
    // A bare connection string is a PostgreSQL one.
    let database = |a: &str| {
        let a = a.trim();
        if a.contains('=') && !a.contains("://") && !a.to_ascii_lowercase().starts_with("postgres:") {
            format!("postgres:{a}")
        } else {
            a.to_string()
        }
    };
    let mut list = false;
    egui::Grid::new("database").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        ui.label(tl!("Connection"));
        ui.add(egui::TextEdit::singleline(&mut app.dialogs.a).hint_text("host=localhost dbname=gis user=postgres").desired_width(280.0));
        ui.end_row();
        ui.label(tl!("Password"));
        ui.add(
            egui::TextEdit::singleline(&mut app.dialogs.b).password(true).hint_text(tl!("none, or from PGPASSWORD")).desired_width(280.0),
        );
        ui.end_row();
        ui.label(tl!("Table"));
        ui.horizontal(|ui| {
            let shown = if app.dialogs.c.is_empty() { tl!("All tables with geometry").to_string() } else { app.dialogs.c.clone() };
            egui::ComboBox::from_id_salt("db_table").selected_text(shown).width(190.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut app.dialogs.c, String::new(), tl!("All tables with geometry"));
                for t in app.dialogs.list.clone() {
                    ui.selectable_value(&mut app.dialogs.c, t.clone(), t);
                }
            });
            list = ui.button(tl!("List Tables")).clicked();
        });
        ui.end_row();
        ui.label(tl!("Coordinate system"));
        ui.add(egui::TextEdit::singleline(&mut app.dialogs.text).hint_text(tl!("of a geometry that doesn't say")).desired_width(280.0));
        ui.end_row();
    });
    if list {
        let p = json!({"database": database(&app.dialogs.a), "password": app.dialogs.b});
        match app.run("info.databaseTables", p) {
            Ok(v) => {
                app.dialogs.list = v["layers"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect();
                app.dialogs.error = None;
                app.dialogs.info = Some(crate::i18n::fmt("{n} tables with geometry", &[("n", &app.dialogs.list.len().to_string())]));
            }
            Err(e) => {
                app.dialogs.info = None;
                app.dialogs.error = Some(e);
            }
        }
    }
    buttons(app, ui, "Add", |app| {
        let mut p = json!({"database": database(&app.dialogs.a), "password": app.dialogs.b});
        for (key, v) in [("layer", &app.dialogs.c), ("crs", &app.dialogs.text)] {
            if !v.trim().is_empty() {
                p[key] = json!(v.trim());
            }
        }
        let r = app.run("layer.addDatabase", p).map(|_| ());
        // The password is the session's to keep, not the dialog's.
        if r.is_ok() {
            app.dialogs.b.clear();
        }
        r
    })
}

/// Add Tile Layer: an XYZ/TMS/WMS template, vector tiles or a MapLibre style by address.
fn add_tiles(app: &mut RustGisApp, ui: &mut egui::Ui) -> bool {
    use rustgis_engine::map::TileKind;
    ui.label(egui::RichText::new(tl!("A tile service's address: raster tiles with {z}/{x}/{y} (or {-y}, {q}, {bbox-epsg-3857} for WMS), vector tiles (.pbf) or TileJSON, a MapLibre style, a PMTiles archive (a web address, or a file on this computer), or an MBTiles file on this computer.")).weak());
    egui::Grid::new("tiles").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        ui.label(tl!("Address"));
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut app.dialogs.text)
                    .hint_text("https://tile.openstreetmap.org/{z}/{x}/{y}.png")
                    .desired_width(if cfg!(target_arch = "wasm32") { 400.0 } else { 320.0 }),
            );
            // A PMTiles or MBTiles file on this computer (the web app reads PMTiles archives from
            // web addresses, and MBTiles files through Add Data).
            if !cfg!(target_arch = "wasm32")
                && ui.button(tl!("Tile Archive File…")).clicked()
                && let Some(path) = pick_open(&["pmtiles", "mbtiles"], tl!("Tile archive"))
            {
                app.dialogs.mode = if path.to_ascii_lowercase().ends_with(".mbtiles") { "mbtiles" } else { "pmtiles" };
                app.dialogs.text = path;
            }
        });
        ui.end_row();
        ui.label(tl!("Type"));
        let guessed = TileKind::guess(app.dialogs.text.trim());
        let shown = match app.dialogs.mode {
            "auto" | "new" => crate::i18n::fmt("Automatic ({kind})", &[("kind", tl!(guessed.label()))]),
            code => TileKind::from_code(code).map(|k| tl!(k.label()).to_string()).unwrap_or_default(),
        };
        egui::ComboBox::from_id_salt("tiles_kind").selected_text(shown).width(260.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut app.dialogs.mode, "auto", tl!("Automatic"));
            for k in TileKind::ALL {
                ui.selectable_value(&mut app.dialogs.mode, k.code(), tl!(k.label()));
            }
        });
        ui.end_row();
        ui.label(tl!("Name"));
        ui.add(egui::TextEdit::singleline(&mut app.dialogs.a).hint_text(tl!("from the address")).desired_width(400.0));
        ui.end_row();
        ui.label(tl!("Attribution"));
        ui.add(egui::TextEdit::singleline(&mut app.dialogs.b).hint_text("© OpenStreetMap contributors").desired_width(400.0));
        ui.end_row();
        ui.label(tl!("Maximum zoom"));
        ui.add(egui::TextEdit::singleline(&mut app.dialogs.c).hint_text(tl!("from the service")).desired_width(60.0));
        ui.end_row();
    });
    ui.checkbox(&mut app.dialogs.flag, tl!("Overlay (a transparent layer kept on top, such as labels or roads)"));
    buttons(app, ui, "Add", |app| {
        let mut p = json!({"url": app.dialogs.text.trim(), "overlay": app.dialogs.flag});
        if !matches!(app.dialogs.mode, "auto" | "new") {
            p["kind"] = json!(app.dialogs.mode);
        }
        for (key, v) in [("name", &app.dialogs.a), ("attribution", &app.dialogs.b)] {
            if !v.trim().is_empty() {
                p[key] = json!(v.trim());
            }
        }
        if let Ok(z) = app.dialogs.c.trim().parse::<f64>() {
            p["maxZoom"] = json!(z);
        }
        app.run("map.addTiles", p).map(|_| ())
    })
}

/// OK/Cancel row; returns true when the dialog should close.
pub(crate) fn buttons(app: &mut RustGisApp, ui: &mut egui::Ui, ok: &str, f: impl FnOnce(&mut RustGisApp) -> Result<(), String>) -> bool {
    let mut close = false;
    ui.add_space(4.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        let t = Tokens::get(ui.ctx());
        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
        if crate::widgets::dialog_button(ui, tl!(ok), Some(t.accent), vec2(84.0, 28.0)).clicked() || enter {
            match f(app) {
                Ok(()) => close = true,
                Err(e) => app.dialogs.error = Some(e),
            }
        }
        if crate::widgets::dialog_button(ui, tl!("Cancel"), None, vec2(84.0, 28.0)).clicked() {
            close = true;
        }
    });
    close
}

fn field_chips(app: &mut RustGisApp, ui: &mut egui::Ui) {
    let fields: Vec<String> = app
        .session
        .map()
        .layer_by_name(&app.dialogs.layer)
        .map(|l| l.dataset.fields.iter().map(|f| f.name.clone()).collect())
        .unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(tl!("Fields:")).small().weak());
        for f in fields.iter().take(24) {
            if ui.small_button(f).clicked() {
                if !app.dialogs.text.is_empty() && !app.dialogs.text.ends_with(' ') {
                    app.dialogs.text.push(' ');
                }
                app.dialogs.text.push_str(f);
                app.dialogs.text.push(' ');
            }
        }
    });
    ui.horizontal(|ui| {
        for op in ["=", "<>", ">", "<", "LIKE", "AND", "OR", "IS NULL"] {
            if ui.small_button(op).clicked() {
                app.dialogs.text.push_str(&format!("{op} "));
            }
        }
    });
}

fn select_by_attributes(app: &mut RustGisApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    let layers: Vec<String> = app.session.map().layers.iter().map(|l| l.name.clone()).collect();
    egui::Grid::new("sba").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
        ui.label(tl!("Input Rows"));
        egui::ComboBox::from_id_salt("sba_layer").selected_text(app.dialogs.layer.clone()).width(260.0).show_ui(ui, |ui| {
            for l in &layers {
                ui.selectable_value(&mut app.dialogs.layer, l.clone(), l);
            }
        });
        ui.end_row();
        ui.label(tl!("Selection Type"));
        egui::ComboBox::from_id_salt("sba_mode")
            .selected_text(match app.dialogs.mode {
                "add" => tl!("Add To Current Selection"),
                "remove" => tl!("Remove From Current Selection"),
                "subset" => tl!("Select Subset From Current Selection"),
                _ => tl!("New Selection"),
            })
            .width(260.0)
            .show_ui(ui, |ui| {
                for (m, l) in [
                    ("new", tl!("New Selection")),
                    ("add", tl!("Add To Current Selection")),
                    ("remove", tl!("Remove From Current Selection")),
                    ("subset", tl!("Select Subset From Current Selection")),
                ] {
                    ui.selectable_value(&mut app.dialogs.mode, m, l);
                }
            });
        ui.end_row();
    });
    ui.label(egui::RichText::new(tl!("Where")).font(semibold(12.5)));
    ui.add(
        egui::TextEdit::multiline(&mut app.dialogs.text)
            .hint_text(tl!("e.g. pop_est > 100000000 AND continent = 'Asia'"))
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace),
    );
    field_chips(app, ui);
    // Live validation.
    let check =
        app.session.map().layer_by_name(&app.dialogs.layer).map(|l| rustgis_engine::data::query::select(&app.dialogs.text, &l.dataset));
    match (&check, app.dialogs.text.trim().is_empty()) {
        (_, true) => {}
        (Some(Ok(rows)), _) => {
            ui.label(
                egui::RichText::new(crate::i18n::fmt("Valid expression · {n} matching rows", &[("n", &rows.len().to_string())]))
                    .small()
                    .color(t.green),
            );
        }
        (Some(Err(e)), _) => {
            ui.label(egui::RichText::new(crate::i18n::fmt("Invalid: {error}", &[("error", e)])).small().color(t.red));
        }
        _ => {}
    }
    buttons(app, ui, "Apply", |app| {
        let layer = app.dialogs.layer.clone();
        app.run("select.byAttributes", json!({"layer": layer, "where": app.dialogs.text, "mode": app.dialogs.mode})).map(|_| ())
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub fn pick_open(exts: &[&str], label: &str) -> Option<String> {
    rfd::FileDialog::new().add_filter(label, exts).add_filter("All files", &["*"]).pick_file().map(|p| p.display().to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn pick_save(name: &str, exts: &[&str], label: &str) -> Option<String> {
    rfd::FileDialog::new().set_file_name(name).add_filter(label, exts).save_file().map(|p| p.display().to_string())
}

#[cfg(target_arch = "wasm32")]
pub fn pick_open(_: &[&str], _: &str) -> Option<String> {
    None
}

#[cfg(target_arch = "wasm32")]
pub fn pick_save(_: &str, _: &[&str], _: &str) -> Option<String> {
    None
}

#[allow(dead_code)]
fn unused(_: Value) {}

/// The map's coordinate system: Web Mercator, the globe, a preset, or any projected EPSG code. The
/// list is on the left; the right shows what the selection is, where it can be used, its PROJ
/// definition, and a link to its page on epsg.io.
fn coordinate_system(app: &mut RustGisApp, ui: &mut egui::Ui, t: &Tokens) -> bool {
    let current = app.session.map().projection.code();
    ui.label(egui::RichText::new(tl!("The map is drawn in this coordinate system; layers are projected on the fly.")).weak());
    let mut choices: Vec<(String, String)> = vec![("mercator".into(), "Web Mercator".into()), ("globe".into(), "Globe".into())];
    choices.extend(rustgis_engine::map::projection::PRESETS.iter().map(|(id, label, _)| (id.to_string(), label.to_string())));
    // Two panes of a fixed height, so the dialog doesn't stretch to the window.
    let pane_h = 340.0;
    ui.allocate_ui_with_layout(vec2(ui.available_width(), pane_h), egui::Layout::left_to_right(egui::Align::Min), |ui| {
        ui.set_height(pane_h);
        ui.vertical(|ui| {
            ui.set_width(270.0);
            egui::ScrollArea::vertical().id_salt("crs_list").max_height(pane_h - 44.0).auto_shrink([false, false]).show(ui, |ui| {
                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    for (id, label) in &choices {
                        let selected = app.dialogs.text.eq_ignore_ascii_case(id);
                        if ui.selectable_label(selected, tl!(label.as_str())).clicked() {
                            app.dialogs.text = id.clone();
                            app.dialogs.a.clear();
                        }
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(tl!("EPSG code"));
                let r = ui.add(egui::TextEdit::singleline(&mut app.dialogs.a).hint_text("3035").desired_width(110.0));
                if r.changed() && !app.dialogs.a.trim().is_empty() {
                    app.dialogs.text = format!("epsg:{}", app.dialogs.a.trim());
                }
            });
        });
        ui.separator();
        ui.vertical(|ui| {
            ui.set_width(340.0);
            egui::ScrollArea::vertical().id_salt("crs_details").max_height(pane_h).show(ui, |ui| {
                crs_details(ui, &app.dialogs.text, &current, t);
            });
        });
    });
    ui.separator();
    let chosen = app.dialogs.text.clone();
    let check = rustgis_engine::map::Projection::from_code(&chosen);
    let same = check.as_ref().is_ok_and(|p| p.code() == current);
    let mut close = false;
    ui.horizontal(|ui| {
        if ui.add_enabled(check.is_ok() && !same, egui::Button::new(tl!("Apply"))).clicked() {
            match app.run("map.projection", json!({"type": chosen})) {
                Ok(_) => {
                    let _ = app.run("map.zoomFull", Value::Null);
                    close = true;
                }
                Err(e) => app.session.notify(e, true),
            }
        }
        if ui.button(tl!("Cancel")).clicked() {
            close = true;
        }
    });
    close
}

/// What a coordinate system is: its kind, area of use, definition and epsg.io page.
fn crs_details(ui: &mut egui::Ui, chosen: &str, current: &str, t: &Tokens) {
    use rustgis_engine::map::projection::{Domain, Projector, lookup};
    let projection = match rustgis_engine::map::Projection::from_code(chosen) {
        Ok(p) => p,
        Err(e) => {
            ui.label(egui::RichText::new(e).color(t.red));
            return;
        }
    };
    let code = projection.code();
    ui.label(egui::RichText::new(tl!(projection.label().as_str())).font(crate::theme::semibold(14.0)));
    if code == current {
        ui.label(egui::RichText::new(tl!("In use")).size(11.0).color(t.accent_text));
    }
    let epsg = code.to_ascii_lowercase().strip_prefix("epsg:").map(str::to_string);
    let (kind, page): (&str, Option<String>) = match code.as_str() {
        "mercator" => ("World, conformal: the usual web map; areas grow towards the poles.", Some("3857".into())),
        "globe" => ("A 3D globe seen from space.", None),
        "equirectangular" => ("World, cylindrical: equal steps of longitude and latitude.", Some("4087".into())),
        "miller" => ("World, cylindrical: a compromise between Mercator and equal steps.", Some("54003".into())),
        "mollweide" => ("World, equal-area: true sizes, shapes bend towards the edges.", Some("54009".into())),
        "cylindricalEqualArea" => ("World, equal-area, cylindrical: true sizes, flattened near the poles.", Some("54034".into())),
        "robinson" => ("World, compromise: neither equal-area nor conformal, balanced for wall maps.", Some("54030".into())),
        "equalEarth" => ("World, equal-area: true sizes with a Robinson-like look.", Some("8857".into())),
        "naturalEarth" => ("World, compromise: rounded corners, gentle shapes near the poles.", None),
        "winkelTripel" => ("World, compromise: low overall distortion, used for world atlases.", Some("54042".into())),
        "eckertIV" => ("World, equal-area: true sizes with rounded poles.", Some("54012".into())),
        "northPolar" => ("Polar, conformal: the Arctic and the northern lands.", None),
        "southPolar" => ("Polar, conformal: Antarctica and the Southern Ocean.", Some("3031".into())),
        "europeEqualArea" => ("Regional, equal-area: the European standard for statistics.", Some("3035".into())),
        "usaAlbers" => ("Regional, equal-area: the contiguous United States.", Some("5070".into())),
        _ => ("A projected coordinate system.", epsg.clone()),
    };
    ui.label(tl!(kind));
    if let Ok(p) = Projector::new(&code) {
        let area = match p.domain() {
            Domain::World => tl!("Area of use: the whole world").to_string(),
            Domain::Box(e) => crate::i18n::fmt(
                "Area of use: longitude {w}° to {e}°, latitude {s}° to {n}°",
                &[
                    ("w", &format!("{:.0}", e.min_x)),
                    ("e", &format!("{:.0}", e.max_x)),
                    ("s", &format!("{:.0}", e.min_y)),
                    ("n", &format!("{:.0}", e.max_y)),
                ],
            ),
            Domain::Cap { center, radius } => crate::i18n::fmt(
                "Area of use: within {r}° of {lon}°, {lat}°",
                &[("r", &format!("{radius:.0}")), ("lon", &format!("{:.0}", center.x)), ("lat", &format!("{:.0}", center.y))],
            ),
            Domain::Band { lon0, half } => crate::i18n::fmt(
                "Area of use: within {r}° of the {lon}° meridian",
                &[("r", &format!("{half:.0}")), ("lon", &format!("{lon0:.0}"))],
            ),
        };
        ui.label(egui::RichText::new(area).size(11.5).color(t.text_dim));
    }
    if let Ok((def, _)) = lookup(&code) {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tl!("PROJ definition")).size(11.0).color(t.text_dim));
        ui.add(egui::Label::new(egui::RichText::new(def).monospace().size(10.5)).wrap());
    }
    if let Some(n) = page {
        ui.add_space(4.0);
        // Five-digit codes from 53000 up are ESRI's world projections.
        let prefix = if n.len() == 5 && n.starts_with('5') { "esri" } else { "epsg" }.to_ascii_uppercase();
        let text = crate::i18n::fmt("Open {code} on epsg.io", &[("code", &format!("{prefix}:{n}"))]);
        // In the browser build the link must not replace the app (and the map) in its tab.
        ui.add(egui::Hyperlink::from_label_and_url(text, format!("https://epsg.io/{n}")).open_in_new_tab(true));
    }
}
