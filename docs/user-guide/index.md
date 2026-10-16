# User Guide

This guide walks through RustGIS by task. Each page covers one part of the app:

| Page | What it covers |
| --- | --- |
| [Data](data.md) | Vector and raster formats, adding data, large files, raster display, project items |
| [Supported Data Formats](data-formats.md) | Every format RustGIS reads and writes, how to add it, and its limits |
| [Maps and Basemaps](maps.md) | Basemaps and web tile layers, projections, the 3D globe, view effects and navigation |
| [Cartography](cartography.md) | Symbology, labeling, layer effects and graphics |
| [Editing](editing.md) | Creating and modifying features, data design, the clipboard and pop-ups |
| [Analysis](analysis.md) | Selection and SQL queries, geoprocessing, the GeoLibre tools, guided workflows, imagery |
| [Layouts and Reports](layouts.md) | Page layouts, map series, reports, presentations and notebooks |
| [Sharing](sharing.md) | Exports, packages and open-format web maps, layers, styles and scenes |
| [Workspace](workspace.md) | Linked views, the Browser view, animation, profiles, offline areas, customizing the ribbon |
| [AI Assistant](assistant.md) | A chat pane that answers questions and acts on the map through commands |

[![RustGIS with the Home tab of the ribbon open over a world map in the Equal Earth projection, countries shaded by population](https://assets.rustgis.app/images/hero.webp)](https://assets.rustgis.app/images/hero.webp){ target="_blank" rel="noopener" title="Open the full-size image" }

## The window

- **Ribbon**: tabs of commands grouped by task. The **Project** button opens the backstage (new,
  open, save, About).
- **Quick Access Toolbar** (top left): New, Open, Save, Undo and Redo, plus any commands you add
  under **Settings › Customize › Quick Access**.
- **Layers pane** (left): the map's layers in drawing order, with their legends. Tick a layer to
  show or hide it; drag it up or down to change the drawing order; right-click it to zoom to it,
  rename or remove it and more.
  The Style and Data tabs act on the layer selected here. The tabs at the top of the left pane
  switch it between **Layers**, **Browser** and **Data Sources** (also in **View › Windows**).
- **Map view**: one tab per map and layout. The buttons at the top right zoom in, zoom out and go
  to the full extent; the north arrow and scale bar sit in the corners; tile credits at the bottom
  right.
- **Panes** (right): Catalog, Geoprocessing, Symbology, History, Details, Editing, Workflows,
  Script, the AI Assistant and more. Open them from **View › Windows** or the commands that use
  them.
- **Status bar**: the scale (type one, or pick from the list), the coordinate system (click it to
  change the projection), the pointer's coordinates (click them to switch between decimal
  degrees and DMS), the selection count and the draw time.

**Command Search** (++ctrl+k++, or the box at the top right) finds any command or geoprocessing
tool by name and runs it.

To give the map more room, collapse the ribbon to its tab row with the chevron at its right
end, by double-clicking a tab, or with ++ctrl+f1++ (++cmd+f1++ on macOS, where F1 may need the Fn
key). Click a tab to peek at its commands while collapsed. When the window is narrower than the
ribbon, arrows at its ends scroll it.

## The ribbon

| Tab | Groups |
| --- | --- |
| **Home** | Navigate (Pan, Bookmarks, extents, Zoom In/Out, Go To XY, Identify), Bookmarks, Add (Basemap, Add Data), Selection, Inspect (Measure, Locate, Zoom Box, Infographics) |
| **Data** | Project (New Map, New Layout, Toolbox, Import, Add Folder, Templates, Task), Layer Templates, Definition Query, Table, Selection, Data Design, Archiving, Relationship, Export (for a raster layer: Information, Export), Offline, Feature Cache |
| **Style** | For the layer selected in Layers: visibility range, transparency, symbology, drawing effects, extrusion and labels (labels, label range, placement, label class, text symbol) for a vector layer; visibility range, transparency, swipe, raster effects, rendering and enhancement for a raster layer; Styles |
| **Edit** | Manage Edits (Edit, Save, Undo), Snapping, Features (Create, Modify, Delete), Tools, Elevation, Corrections, Graphics, Graphics and Text, Clipboard |
| **Analyze** | Geoprocessing (History, Tools, ModelBuilder, Script, Environments, Ready To Use), AI (AI Assistant), Portal (Feature and Raster Analysis), Workflows, Raster (Raster Functions, Function Editor), the tool gallery, Selection, Notebook, the imagery groups (Ortho Mapping, Reality Mapping, Alignment, Image Analysis, Image Classification, Imagery Sharing, Mensuration, Imagery Tools) |
| **View** | Display, View (Globe, Projection, Global, Local), Effects, Scene, Windows, Labeling, Navigation, Link, Workspace, Animation, Profile |
| **Publish** | While a layout is open, its groups first (Page, Map Frames, Map Surrounds, Text, Export); then Report, Presentation, Output (Export Map, Print, Capture), Package, Share As, Layout, Save As, Thumbnail |
| **Plugins** | The Plugin Manager, plus a tab for each [plugin](../plugins.md) that adds one |
| **Settings** | Customize (ribbon tabs, Quick Access Toolbar), Language (interface language and worldview), Start-up (start with the sample or a blank map, projection, extent) |
| **Help** | Performance (Diagnostic Monitor), Learn, Help (About, Check for Updates on the desktop, Command Search, Shortcuts) |

The tabs stay the same whatever is selected: Style and Data act on the layer selected in Layers.

## Projects

A project (`.rgis`) holds maps, layouts, reports, presentations, notebooks, styles, tasks and
models. Vector data added from files is stored inside the project, so a project file is
self-contained; rasters are referenced by path on the desktop and stored inside the project in
the web app. **Project › New** starts a blank map or the World Atlas sample; **Save** and **Save
As** write the project, named after its file; a project opened from a file is named after that
file too. See [Data › Project items](data.md#project-items) for the other files
RustGIS reads and writes.

Every change is a command, and every command can be undone (++ctrl+z++) and redone
(++ctrl+y++).

## Keyboard shortcuts

| Keys | Action |
| --- | --- |
| ++ctrl+k++ | Command search |
| ++ctrl+n++ / ++ctrl+o++ / ++ctrl+s++ | New / Open / Save project |
| ++ctrl+z++ / ++ctrl+y++ | Undo / Redo |
| ++ctrl+t++ | Attribute table |
| ++ctrl+f1++ | Collapse or expand the ribbon |
| ++home++ | Full extent |
| ++esc++ | Back to Explore; cancel a sketch |
| ++delete++ | Delete selected features |
| ++ctrl+c++ / ++ctrl+x++ / ++ctrl+v++ | Copy / Cut / Paste features |

**Help › Shortcuts** lists them all in the app.
