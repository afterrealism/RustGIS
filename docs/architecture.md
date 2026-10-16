# Architecture

RustGIS is a layered Cargo workspace. Lower layers never depend on the UI, which keeps the
engine scriptable and the front end swappable.

| Layer | Crate | Job |
| --- | --- | --- |
| L0 | `rustgis-geom` | Coordinates, extents, Web Mercator, geodesic distance/area, scale |
| L1 | `rustgis-data` | Features and typed attributes; GeoJSON, CSV, Shapefile; a read-only SQLite file reader (`sqlite`, for GeoPackages in the browser); data providers reading GeoPackage, GeoParquet and FlatGeobuf in place by their spatial index, and PostGIS tables live (desktop) (GDAL formats and DuckDB queries converted by those programs when installed); SQL where clauses; rasters (GeoTIFF, ASCII grid) and their coordinate systems |
| L2 | `rustgis-map`, `rustgis-analysis` | Layers, symbology, labeling, camera, basemaps and web tile layers; geoprocessing tools |
| L2 | `rustgis-tiles` | Web map tiles: XYZ/TMS/WMS raster tiles, Mapbox Vector Tiles, PMTiles and MBTiles archives and a subset of MapLibre styles; a cache that fetches in the background (worker threads natively, the browser's `fetch` on the web) |
| L3 | `rustgis-render` | tiny-skia renderer, label placement, PNG export |
| L3 | `rustgis-geolibre` | The [GeoLibre tools](user-guide/analysis.md#geolibre-tools) as geoprocessing tools: run by the optional `geolibre` program natively, by geolibre-rust's WASI runner in the browser |
| L3 | `rustgis-plugins` | [Plugins](plugins.md): sandboxed WebAssembly modules that add commands and geoprocessing tools; `.rgplugin` packages and the plugin registry |
| L4 | `rustgis-engine` | Session, undo, command registry, sample project |
| L5 | `rustgis-assistant` | The [AI Assistant](user-guide/assistant.md): a chat agent that acts through commands; OpenAI-compatible, OpenAI Responses (a ChatGPT plan's sign-in), Anthropic and Gemini wire formats as plain request/response data, so the desktop (a thread), the browser (`fetch`) and the command line share one loop |
| L6 | `rustgis-ui-egui` | The ribbon front end |
| apps | `rustgis`, `rustgis-cli`, `rustgis-web` | Desktop app, command line, browser (WebAssembly) |

## Everything is a command

Each user-visible action is a `CommandSpec` with an id, label, ribbon location, shortcut,
parameter description, an `enabled` check and a `run` function. The ribbon, keyboard shortcuts,
command search and CLI all dispatch through `Session::run`, which also snapshots the project
for undo and restores it if a command fails.

Plugin commands (`plugin:<plugin>/<command>`) dispatch through `Session::run` too, as one undo
step. While they run, they reach the session only through permission-checked requests (see
[Plugins](plugins.md)).

## Never crash

Production code avoids `unwrap`, `expect` and panics, and treats file contents and command
parameters as hostile. Tests run every command and every geoprocessing tool with junk
parameters.

## Quality gates

```sh
cargo xtask ci        # fmt, clippy -D warnings, tests, layering check
```
