# Maps and Basemaps

A project holds one or more maps (**Data › New Map**), each with its own layers, basemap,
projection, bookmarks and view settings. Maps open as tabs above the map view.

## Navigating

Drag to pan and scroll to zoom; ++home++ (or **Full Extent**) shows all the data. **Map ›
Navigate** has fixed zoom steps, **Previous** and **Next** extents, **Bookmarks** (the World Atlas
has World, Europe, Africa, Americas and East Asia) and **Go To XY**, which pans to a longitude and
latitude. The Bookmarks menu removes a bookmark with the button beside it, and exports or imports
them as a `.rgisbookmarks` file. Imported bookmarks show the same place on a map in another
projection; places that projection can't show are skipped. Type a scale in the status bar to jump
to it. More navigation tools are under [View
effects](#view-effects) below.

**Find Place** (`map.findPlace`) looks up a place or an address by name with OpenStreetMap's
[Nominatim](https://nominatim.org) (or a [Photon](https://photon.komoot.io) server, or a
geocoder of your own given by its address) and zooms to the one picked. The search goes to that
server over the network; see [Geocoding addresses](data.md#geocoding-addresses) for whole tables.

## Basemaps

![A MapLibre style from OpenFreeMap drawn over Paris, with the Basemap gallery listing the built-in basemaps and the web tile layer](https://assets.rustgis.app/images/tiles.webp)

**Home › Add › Basemap** offers five built-in basemaps drawn from a world outline (Paper, Midnight,
Ocean, Atlas, Blank), which need no network, and web tile layers over them:

- **Raster tiles** from an XYZ or TMS template (`https://…/{z}/{x}/{y}.png`; `{-y}` for rows
  from the south, `{s}` for subdomains, `{q}` for Bing-style quadkeys) or a WMS through
  `{bbox-epsg-3857}`. PNG, JPEG and WebP.
- **Vector tiles** (Mapbox Vector Tiles, `.pbf`) from a template or a TileJSON address, drawn in
  a built-in style that knows the OpenMapTiles, Protomaps and Shortbread schemas.
- **PMTiles archives** (`.pmtiles`) of raster or vector tiles: a web address, read a tile at a
  time by byte ranges (the server must allow range requests, and CORS for the web app), or on the
  desktop a file (**Tile Archive File…**, Add Data, or drop it on the window). A vector archive
  that is a basemap is drawn in the built-in style; any other is drawn with each of its layers in
  a colour of its own.
- **MBTiles files** (`.mbtiles`) of raster or vector tiles, drawn as PMTiles archives are: on
  the desktop read in place a tile at a time (**Tile Archive File…**, Add Data, or drop it on the
  window); in the web app added with Add Data and held in memory for the session (a project keeps
  the layer, and asks for the file again).
- **Map styles**: a MapLibre style document (`style.json`), with its vector and raster sources.
  RustGIS draws background, fill, line, circle and raster layers, point labels with their icons
  from the style's sprite (SDF icons in their `icon-color`), and fill patterns; it evaluates the
  style's expressions and zoom functions. Line labels and shields along roads, line patterns,
  hillshade and 3D extrusion are not drawn (extrusions draw flat). A label shows the lines of
  its text that the map's font can draw: a style's "Syria / سوريا" shows as "Syria".

The gallery's **Web Basemaps** need no API key: the OpenFreeMap vector maps (Liberty, Bright,
Positron, Dark, Fiord) and USGS Imagery. Choosing one replaces the other web basemaps. For many
more (OpenStreetMap, Esri, NASA and others), follow **Install the Basemaps plugin** under them,
which opens the Plugin Manager at the
[Basemaps plugin](https://plugins.rustgis.app/catalog/basemaps/); it adds a tab of public
basemaps. Add any other with **Add Tile Layer…** at the bottom of the Basemap gallery.
The gallery lists the map's tile layers: show or hide them, change their opacity and order, or
remove them. The Layers pane lists them too, under the map's layers and above the basemap: tick
one to show or hide it, and right-click it for its opacity, order and **Remove**. Overlays (labels, roads) stay on top of the basemaps; choosing a built-in basemap
removes the web basemaps but keeps the overlays. Tile layers are saved with the project, and
their credits are shown in the corner of the map and drawn on exports.

Tiles load in the background and the map fills in as they arrive, with coarser tiles standing in
meanwhile; exports and `rustgis-cli` wait for them. On the globe and in other projections the
tiles are reprojected. From scripts: `map.addTiles`, `map.removeTiles`, `map.setTiles` and
`map.tiles`.

### Worldview

The world outline and the World Atlas sample's countries come in two worldviews: Natural
Earth's default borders, and China's (Natural Earth's China point of view: Taiwan, Aksai Chin
and South Tibet are part of China). RustGIS picks China's when the system's time zone is
mainland China's (such as `Asia/Shanghai`) or its first locale is `zh-CN`; nothing is looked up
over the network. Choose one in **Settings › Language › Worldview**, set `RUSTGIS_WORLDVIEW`
(`default` or `china`), or pass `--worldview` to `rustgis-cli`. An untouched sample project
switches its countries at once; others keep the layer they have.

## Projections

The flat map is drawn in Web Mercator by default. **View › Projection** switches it to another
projection, and clicking the coordinate system in the status bar opens the **Coordinate
System** dialog, which also takes a projected EPSG code (such as `3035`, `32633` for UTM zone
33N or `2056` for Switzerland). A code whose projection method or datum grid RustGIS can't use
(Polyconic such as `5880`, Cassini, oblique Mercator, or a NAD27 code that needs a grid) is refused
with the reason, rather than drawn in another projection:

| Preset | Kind |
| --- | --- |
| Equirectangular, Miller | World, cylindrical |
| Mollweide, Lambert Cylindrical Equal Area, Equal Earth, Eckert IV | World, equal-area |
| Robinson, Natural Earth, Winkel Tripel | World, compromise |
| North / South Polar Stereographic | Polar |
| Europe Equal Area (ETRS89 LAEA), USA Contiguous Albers Equal Area | Regional, equal-area |

Layers are projected on the fly. A feature that crosses the edge of a world projection (the
meridian opposite its central one, such as 90°E in Equal Earth Americas) is cut there and drawn on
both sides of the map. Each projection has an area of use: polar projections show
their hemisphere poleward of 30°, regional ones their region, transverse Mercator (UTM) zones
30° either side of their meridian, and data outside it isn't drawn. Switching keeps the centre
and scale, and the choice is saved with the map (`map.projection` from the command line). A
raster already in the map's coordinate system draws without being reprojected. To open every new
session in a projection, set it in **Settings › Start-up › Projection**. Robinson, Equal
Earth, Natural Earth, Winkel Tripel and Eckert IV are computed by RustGIS itself on a sphere,
so EPSG:8857 (Equal Earth) works too.

## Globe

![The World Atlas on the 3D globe, with the View tab open](https://assets.rustgis.app/images/globe.webp)

**View › Globe** shows the map as a 3D globe seen from space instead of the flat Web Mercator
map, and back again. Drag to turn the globe, scroll to zoom, and use **Full Extent** to see the
whole Earth. Every layer draws on the globe: vectors are clipped at the horizon and follow great
circles, rasters are draped onto the sphere, and labels, selection, identify, measuring and
editing work as on the flat map. The scale stays the same when you switch, and the choice is
saved with each map in the project (`map.projection` from the command line).

While the globe turns or zooms, the map is drawn flat once and laid over the sphere by the
graphics card, so it moves smoothly whatever the basemap; labels come back, and the map is
drawn exactly, as soon as it stops.

The globe hangs in dark space, in the light theme too, with stars around it and now and then a
shooting star passing behind it. **View › Visual Effect › Stars** turns them off (exports show
the stars but never the shooting stars).

## View effects

![Dark mode over Europe on the Midnight basemap, with the View tab open](https://assets.rustgis.app/images/dark.webp)

The View tab adds effects drawn over the map: **Illumination** (day and night shading for a time,
with twilight), **Weather** (haze, fog, rain, snow), **Visual Effect** (vignette, edges, the stars around the globe), **Camera
Effect** (tilt-shift blur, film grain) and **Postprocessing** (brightness, contrast, saturation,
sepia). The **Color Vision** simulator shows the map as people with protanopia, deuteranopia,
tritanopia or achromatopsia see it (on screen only; exports are untouched).

**Global** shows a globe; **Local** a flat view centred where you are (azimuthal equidistant;
not in the web app, which doesn't include that projection).
**Wireframe** draws polygons as outlines, **Depth Priority** lifts a layer above others whatever
its order, **View Clipping** hides features beyond a distance from the centre, and **Speed
Display** draws without antialiasing. The **Navigator** adds a compass, zoom buttons and a pan
pad; **Camera** shows and sets the position and scale; the **Magnifier** shows twice the map under
the pointer. On the Home tab, **First-Person** navigation walks with W A S D and flies up and down
with Q and E. The interface follows the system's light or dark setting; **View › Display › Dark
Mode** switches it by hand.

**View › Display › GPU Rendering** (beta) draws the flat map on the GPU instead of the CPU:
vector layers are tessellated once and re-projected per frame, so panning and zooming stay
smooth on large datasets. It is off by default, and the setting is remembered across sessions
(`ui.map.gpu` from the command line; `RUSTGIS_GPU=1` forces it on for development). The globe,
view effects and the colour-vision simulator always render on the CPU, and exports are
unaffected — the choice only changes the on-screen flat map.
