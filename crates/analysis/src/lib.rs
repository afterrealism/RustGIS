//! L2: geoprocessing tools.
//!
//! Each tool is a [`ToolSpec`]: an id, a toolbox, parameters for the Tools pane, and a
//! pure `run` function over datasets. Inputs refer to layers by name through [`Inputs`]. Work is
//! done in Web Mercator metres; distances are corrected for scale distortion at each feature's
//! latitude so a 100 km buffer is about 100 km on the ground.
//!
//! Tools from external suites (the GeoLibre/Whitebox tools in `rustgis-geolibre`, and plugins
//! in `rustgis-plugins`) join the built-in ones through [`register_external`]: they're listed by [`tools`], found by
//! [`tool`] and run by [`run`] like the rest.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod convert;
pub mod network;
pub mod neural;
pub mod rasterops;
mod tools;
pub mod topology;

pub use tools::{TOOLS, holds_coordinates, simplify_geometry};

use rustgis_data::Dataset;
use serde_json::Value;

/// Parameter kinds, which the Tools pane turns into widgets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// Any layer.
    Layer,
    /// A polygon layer.
    PolygonLayer,
    /// A point layer.
    PointLayer,
    /// A field of the layer given by the first `Layer` parameter.
    Field,
    /// A numeric field of the first layer.
    NumericField,
    /// A field of the first layer that can hold coordinates: a numeric field, or a text field
    /// with numbers written in it (see [`holds_coordinates`]).
    CoordinateField,
    Number,
    Text,
    /// One of a fixed list of choices.
    Choice(&'static [&'static str]),
    /// A raster layer.
    RasterLayer,
    /// Any vector layer (no standalone tables).
    VectorLayer,
    /// Several raster layers, in order: their names (or file paths) separated by `;`.
    RasterLayers,
    /// Several vector layers, in order: their names (or file paths) separated by `;`.
    VectorLayers,
    /// On or off.
    Bool,
    /// A file path typed by the user (for data RustGIS doesn't open as a layer, such as LiDAR).
    Path,
}

#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    pub required: bool,
    /// Default value as JSON text (`"10"`, `"\"km\""`), or empty.
    pub default: &'static str,
    /// A longer description, shown on hover (may be empty).
    pub help: &'static str,
}

/// What a tool produced.
#[derive(Clone, Debug)]
pub enum Output {
    /// A new layer to add to the map.
    Layer(Dataset),
    /// Rows to select in an existing layer.
    Selection { layer: String, rows: Vec<usize> },
    /// An existing layer's dataset replaced (e.g. new fields).
    Update { layer: String, dataset: Dataset },
    /// A standalone table.
    Table(Dataset),
    /// Several new layers and tables (external tools can write more than one output).
    Layers(Vec<OutputLayer>),
}

/// One output of [`Output::Layers`].
#[derive(Clone, Debug)]
pub enum OutputLayer {
    Vector(Dataset),
    /// A raster file, kept as its bytes so the project can embed it.
    Raster {
        name: String,
        file_name: String,
        bytes: Vec<u8>,
    },
    Table(Dataset),
}

/// A raster layer handed to a tool: its file (on disk or embedded), or failing that the pixels.
pub struct RasterInput<'a> {
    pub raster: &'a rustgis_data::raster::Raster,
    /// The file it was read from, when it is on disk.
    pub path: Option<&'a str>,
    /// The file's name and bytes, when it is embedded in the project.
    pub embedded: Option<(&'a str, &'a [u8])>,
}

#[derive(Clone, Debug)]
pub struct ToolResult {
    pub output: Output,
    pub messages: Vec<String>,
}

/// Access to the map's layers by name.
pub trait Inputs {
    fn dataset(&self, layer: &str) -> Option<&Dataset>;
    /// A standalone table by its id: an input given as a number, which no name is taken for.
    fn table(&self, _id: u32) -> Option<&Dataset> {
        None
    }
    /// Selected rows of a layer; tools honour a selection like desktop GIS does.
    fn selection(&self, layer: &str) -> &[usize];
    /// A raster layer by name.
    fn raster(&self, _layer: &str) -> Option<RasterInput<'_>> {
        None
    }
}

pub struct ToolSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub toolbox: &'static str,
    pub summary: &'static str,
    pub params: &'static [ParamSpec],
    pub run: fn(&dyn Inputs, &Value) -> Result<ToolResult, String>,
}

/// Runs an external tool (the spec says which).
pub type ExternalRunner = fn(&ToolSpec, &dyn Inputs, &Value) -> Result<ToolResult, String>;

/// Where external tools come from: called the first time the tools are listed, and again after
/// [`reload_external`].
pub type ExternalProvider = fn() -> Vec<ToolSpec>;

/// One external tool suite: its tools are loaded lazily and leaked (each load lives as long as
/// the app, and reloads are rare: the user installs a suite or a plugin), so specs can be handed
/// out as `&'static`.
struct Source {
    key: &'static str,
    provider: ExternalProvider,
    runner: ExternalRunner,
    tools: Option<&'static [ToolSpec]>,
}

static SOURCES: std::sync::RwLock<Vec<Source>> = std::sync::RwLock::new(Vec::new());

/// Adds an external tool suite under `key` (the GeoLibre tools, plugins). Only the first
/// registration of a key counts; the provider runs lazily, so registering costs nothing until a
/// tool list is needed.
pub fn register_external(key: &'static str, provider: ExternalProvider, runner: ExternalRunner) {
    if let Ok(mut v) = SOURCES.write()
        && !v.iter().any(|s| s.key == key)
    {
        v.push(Source { key, provider, runner, tools: None });
    }
}

/// The tools of the suite registered as `key` (empty when there is none).
pub fn external_tools_of(key: &str) -> &'static [ToolSpec] {
    let provider = match SOURCES.read() {
        Ok(v) => match v.iter().find(|s| s.key == key) {
            Some(Source { tools: Some(t), .. }) => return t,
            Some(s) => s.provider,
            None => return &[],
        },
        Err(_) => return &[],
    };
    // One load at a time: a second caller waits for the first instead of loading again. The
    // provider runs without the sources' lock held, so a slow one never blocks the others.
    static LOADING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _loading = LOADING.lock();
    if let Ok(v) = SOURCES.read()
        && let Some(Source { tools: Some(t), .. }) = v.iter().find(|s| s.key == key)
    {
        return t;
    }
    let tools: &'static [ToolSpec] = Box::leak(
        std::panic::catch_unwind(provider)
            .unwrap_or_else(|_| {
                log::error!("the external tools '{key}' failed to load");
                Vec::new()
            })
            .into_boxed_slice(),
    );
    match SOURCES.write() {
        Ok(mut v) => match v.iter_mut().find(|s| s.key == key) {
            Some(s) => s.tools.get_or_insert(tools),
            None => tools,
        },
        Err(_) => tools,
    }
}

/// The external tools of every suite, in registration order (empty when none are registered).
pub fn external_tools() -> Vec<&'static ToolSpec> {
    let keys: Vec<&'static str> = SOURCES.read().map(|v| v.iter().map(|s| s.key).collect()).unwrap_or_default();
    keys.into_iter().flat_map(external_tools_of).collect()
}

/// Loads every suite's tools again the next time they're needed.
pub fn reload_external() {
    if let Ok(mut v) = SOURCES.write() {
        for s in v.iter_mut() {
            s.tools = None;
        }
    }
}

/// Loads the tools of the suite `key` again the next time they're needed (it was installed,
/// located or removed, or a plugin was).
pub fn reload_external_of(key: &str) {
    if let Ok(mut v) = SOURCES.write()
        && let Some(s) = v.iter_mut().find(|s| s.key == key)
    {
        s.tools = None;
    }
}

/// Every tool: the built-in ones, then the external ones.
pub fn tools() -> impl Iterator<Item = &'static ToolSpec> {
    TOOLS.iter().chain(external_tools())
}

/// The suite an external tool comes from, with its runner (`None` for a built-in tool).
fn source_of(spec: &ToolSpec) -> Option<(&'static str, ExternalRunner)> {
    // By id, not by address: a spec handed out before a reload (an open tool form) is still
    // the same tool.
    if TOOLS.iter().any(|t| std::ptr::eq(t, spec)) {
        return None;
    }
    let keys: Vec<(&'static str, ExternalRunner)> =
        SOURCES.read().map(|v| v.iter().map(|s| (s.key, s.runner)).collect()).unwrap_or_default();
    keys.into_iter().find(|(key, _)| external_tools_of(key).iter().any(|t| t.id == spec.id))
}

/// Whether a tool comes from an external suite.
pub fn is_external(spec: &ToolSpec) -> bool {
    source_of(spec).is_some()
}

/// The key of the suite a tool comes from (`None` for a built-in tool).
pub fn external_source(spec: &ToolSpec) -> Option<&'static str> {
    source_of(spec).map(|(k, _)| k)
}

pub fn tool(id: &str) -> Option<&'static ToolSpec> {
    let by_id = |t: &&ToolSpec| t.id.eq_ignore_ascii_case(id);
    let by_name = |t: &&ToolSpec| t.name.eq_ignore_ascii_case(id);
    // A built-in tool wins over an external one with the same name.
    tools().find(by_id).or_else(|| TOOLS.iter().find(by_name)).or_else(|| external_tools().into_iter().find(by_name))
}

/// The runner for external tools; never called (external specs carry the runner's id).
pub fn external_placeholder(_: &dyn Inputs, _: &Value) -> Result<ToolResult, String> {
    Err("this tool runs through the external suite".into())
}

/// A tool's parameters with defaults filled in, or an error naming a missing required one.
/// [`run`] does this first; a host that runs an external tool another way (the browser's
/// asynchronous runner) does it too, so both check parameters the same way.
pub fn resolve_params(spec: &ToolSpec, params: &Value) -> Result<Value, String> {
    let mut p = params.as_object().cloned().unwrap_or_default();
    for ps in spec.params {
        if !p.contains_key(ps.name)
            && !ps.default.is_empty()
            && let Ok(v) = serde_json::from_str::<Value>(ps.default)
        {
            p.insert(ps.name.to_string(), v);
        }
        if ps.required && p.get(ps.name).is_none_or(|v| v.is_null() || v.as_str() == Some("")) {
            return Err(format!("{}: '{}' is required", spec.name, ps.label));
        }
    }
    Ok(Value::Object(p))
}

/// Runs a tool with JSON parameters, filling defaults and checking required ones.
pub fn run(id: &str, inputs: &dyn Inputs, params: &Value) -> Result<ToolResult, String> {
    let spec = tool(id).ok_or_else(|| format!("no tool '{id}'"))?;
    let p = resolve_params(spec, params)?;
    // A bug in a tool must not take the app down.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match source_of(spec) {
        Some((_, runner)) => runner(spec, inputs, &p),
        None => (spec.run)(inputs, &p),
    }))
    .unwrap_or_else(|_| Err(format!("{} failed unexpectedly", spec.name)))
}
