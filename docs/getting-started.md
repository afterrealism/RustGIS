# Getting Started

## Open the app

- **In the browser**: open [web.rustgis.app](https://web.rustgis.app). Nothing to install.
- **On the desktop**: [install](installation.md) RustGIS and start it. It opens the World Atlas
  sample: countries, cities, rivers and lakes from Natural Earth.
- **From source**:

```sh
cargo run --release -p rustgis                  # opens the World Atlas sample
cargo run --release -p rustgis -- places.geojson
cargo run --release -p rustgis -- atlas.rgis   # a saved project
```

## Five minutes with the World Atlas

1. **Explore.** Drag to pan, scroll to zoom, and click a country to see its attributes in a
   pop-up. Press ++home++ for the full extent, or pick a place from **Home › Bookmarks**.
2. **Style a layer.** Click *Countries* in the Layers pane, then open the
   **Style** tab and **Style › Symbology**, choose *Graduated Colors* or *Unique Values*,
   and pick a colour scheme.
3. **Select.** **Home › Select By Attributes**, enter `continent = 'Asia'`, and press *Apply*.
   Open the attribute table with ++ctrl+t++. Rows and map features stay in sync.
4. **Analyse.** **Analyze › Buffer**: choose *Cities*, 300 km, *Dissolve: All*, and *Run*. The
   result is added as a new layer, and the run is listed under **Analyze › History**.
5. **Change the view.** **View › Globe** turns the map into a 3D globe; **View › Projection**
   offers Equal Earth, Robinson and other projections. **Home › Add › Basemap** switches the basemap or
   adds a web tile layer.
6. **Share.** **Publish › Export Map** saves a PNG with labels and a scale bar. **Data › New
   Layout** makes a printable page with a legend and north arrow. **Project › Save As** writes a
   `.rgis` project.

!!! tip "Command search"
    Press ++ctrl+k++ to find any command or geoprocessing tool by name.

## Your own data

**Home › Add Data** (or drop files on the window) opens GeoJSON, CSV with longitude and latitude
columns, GeoParquet, FlatGeobuf, GeoTIFFs and ASCII grids, and on the desktop also Shapefiles and
GeoPackages. **Project › New › Map** starts from a blank map.
See [Data](user-guide/data.md) for the details of each format.

## Ask the AI Assistant

**Analyze › AI Assistant** opens a chat pane. Choose a model provider (Anthropic, OpenAI,
your ChatGPT account, Gemini, OpenRouter, a local Ollama, or any OpenAI-compatible server) and ask, for example,
"Which 5 countries have the largest GDP?" or "Label the cities and zoom to Tokyo". See
[AI Assistant](user-guide/assistant.md).

## Next steps

- The [User Guide](user-guide/index.md) walks through every part of the app.
- [Command Line](cli.md) shows how to script RustGIS with `rustgis-cli`.
- [Plugins](plugins.md) shows how to add tools from the plugin registry, or write your own.
