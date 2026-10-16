# Supported Data Formats

This page lists the data formats RustGIS reads and writes, how to add each one, and the main
limits. [Data](data.md) covers adding data, large files and rasters in more detail; [Maps and
Basemaps](maps.md#basemaps) covers web tile layers.

Most formats open in the desktop app and in the [web app](https://web.rustgis.app) alike. Those
that need C libraries (GDAL, DuckDB) or files on disk are desktop-only, as the tables say. The
web app reads GeoPackages without SQLite, with a small SQLite file reader written in Rust.

## Vector and tabular files

| Format | Extensions or source | How to add it | Notes |
| --- | --- | --- | --- |
| GeoJSON | `.geojson`, `.json` | Home › Add Data, or drop the file | Features with typed attributes; a `GeometryCollection` keeps its first member. On the desktop, a FeatureCollection larger than 32 MB is read in place, a view at a time, however large; otherwise files up to 256 MB load whole. |
| Delimited text | `.csv`, `.tsv`, `.txt` | Add Data, or Add Data › Delimited Text… to choose the columns from the file's header | Comma, semicolon, tab or pipe separated (found from the header, or `"delimiter"`); `info.fileColumns` lists the columns and the ones found. Points from x/y columns (named like longitude/latitude or easting/northing, or chosen with `"x"` and `"y"`), or any geometry from a WKT/EWKT column (named like `wkt` or `geometry`, or `"wkt"`). Coordinates in another system are converted from `"crs"` (`EPSG:2263`, or WKT) or an EWKT `SRID=`; without either, coordinates outside longitude/latitude are pointed out. A file without coordinates opens as a standalone table. |
| Shapefile | `.shp` with `.dbf` (and `.shx`), or zipped (`.zip`) | Add Data, drop the file, or upload the zip in the web app | A zip may hold several Shapefiles, in folders, and other vector files (GeoJSON, CSV, GeoParquet, FlatGeobuf); each becomes a layer, at most 32 for the whole archive (Shapefiles first, so other files are left out when Shapefiles fill it). `.txt` files and macOS `__MACOSX` entries are passed over, and a member that can't be read is skipped and named in a notice. Converted to longitude/latitude from the coordinate system in its `.prj` (EPSG, Esri or OGC WKT; Web Mercator, UTM, State Plane and other common projections); without a `.prj` it must be in longitude/latitude. On the desktop, one with more than 100,000 shapes (counted from its `.shx`) is read in place, a view at a time. |
| KML / KMZ | `.kml`, `.kmz` | Add Data, drop the file, or upload it in the web app | A layer for each kind of geometry (points, lines, polygons), with each placemark's name, description, folder path and extended data. Styles are kept: the layer is drawn by unique values of its `style` field, with each KML style's line and fill colours, widths and icon colours (up to 64 styles). Multi-geometries and `gx:Track`s are read. Ground overlays (pictures on a `LatLonBox`, turned by its `rotation`) are raster layers kept in the project: the picture inside a KMZ, or a file beside a KML (not a web address, nor outside its folder); a `gx:LatLonQuad` isn't drawn. |
| GPX | `.gpx` | Add Data, drop the file, or upload it in the web app | Waypoints, routes, tracks (one line per track, its segments as parts) and track points (with elevation and time) as separate layers. |
| Spreadsheets | `.xlsx`, `.xlsm`, `.xlsb`, `.xls`, `.ods` | Add Data (or Delimited Text… to choose the columns), drop the file, or upload it in the web app | One sheet at a time, read like delimited text: the first non-empty row names the columns, and x/y or WKT columns (found, or given with `"x"`, `"y"`, `"wkt"` and `"crs"`) make the geometry; a sheet without them is a table. The first sheet is added and the others named in a notice; `"layer"` picks one, and `info.fileLayers` lists them. Dates read as ISO 8601. |
| GeoRSS | `.rss`, `.atom`, `.georss`, or `.xml` | Add Data, drop the file, upload, or From URL… | RSS 2.0, RSS 1.0 and Atom feeds with GeoRSS Simple, GeoRSS GML or W3C Basic Geo places: points, lines and polygons layers with each item's title, link, description, date, id and category. |
| LandXML | `.landxml`, or `.xml` | Add Data, drop the file, or upload | Coordinate geometry points, parcels (polygons), alignments (lines; curves as chords) and TIN surfaces (a triangle per face, with its mean elevation), converted from the file's `epsgCode` or `"crs"`. |
| Encoded polylines | text | `layer.addEncodedPolyline` with `"polyline"` (one or a list) | Google's encoding, precision 5, or 6 (`"precision"`) as OSRM and Valhalla can give: a line layer. |
| GML | `.gml` | Add Data, drop the file, or upload | Simple features in GML 2 and 3 (WFS answers, OGR's GML): a layer per feature type, its simple elements as fields. Converted from the `srsName` (EPSG codes, URNs and URLs; the URN and URL forms of geographic systems are latitude first). |
| MapInfo Interchange | `.mif` with its `.mid` (or both zipped) | Add Data, drop the file, or upload the zip | Points, lines, polylines, regions (holes found by nesting), rectangles and ellipses; text and arcs are rows without geometry. Converted from the `CoordSys`: longitude/latitude, Transverse Mercator, Lambert Conformal Conic, Albers or Mercator. |
| OpenStreetMap PBF | `.osm.pbf` | Add Data, drop the file, or upload | Points (tagged nodes), lines and multipolygons (closed ways that are areas, and multipolygon and boundary relations), with common tags as fields and the rest in `other_tags`, as GDAL gives them. Up to 40 million nodes (4 million in the web app); on the desktop, extracts over 256 MB open through GDAL when it is installed. |
| TopoJSON | `.topojson`, or `.json` | Add Data, drop the file, or upload | A layer per object, its geometries rebuilt from the shared arcs (quantized or not). |
| AutoCAD DXF | `.dxf` (ASCII) | Add Data, drop the file, or upload | Points, lines and polygons layers from the drawing's entities: POINT, LINE, LWPOLYLINE and POLYLINE (bulges drawn as arcs; closed ones stay lines; polyface meshes become polygons), CIRCLE, ARC, ELLIPSE, SPLINE, HELIX, LEADER, MLINE (its centre line), TEXT, MTEXT, ATTRIB and constant ATTDEF (points with their text, at the alignment point of justified text), HATCH boundaries (holes found by nesting), SOLID, TRACE and 3DFACE (polygons). Blocks (named in any case) are drawn where INSERTs place them (scaled, rotated, in arrays, nested up to 8 deep), and so are DIMENSIONs' blocks (a linear dimension without one draws its lines and measured text). Entities in an object's own coordinate system are turned into the drawing's. Fields: `layer`, `entity`, `color` (ACI, BYLAYER and BYBLOCK resolved), `linetype`, `text`, `height`, `rotation`, `block`, `elevation` and `paperspace`. The drawing's layouts (paper space: title blocks, viewport frames) are left out, with a notice, unless `"paperSpace": true`. A drawing never says its coordinate system: give `"crs"` (such as `EPSG:2263`), or its coordinates are kept as they are, with a notice. `"layer"` picks one of the drawing's own layers, and `info.fileLayers` lists them. Up to 2 million features (500,000 in the web app). Not read: binary DXF, MULTILEADER, WIPEOUT, IMAGE and ACIS solids (3DSOLID, REGION), named in a notice. |
| Geotagged photos | `.jpg`, `.jpeg` | Add Data (several at once in the web app), drop them, upload, or `layer.addPhotos` with `"paths"` or a `"folder"` | JPEGs with a GPS position in their EXIF become points in the **Photos** layer (photos added one at a time join it): each with its `name`, `file`, `time` (when it was taken), `altitude`, `direction` (where the camera faced), `camera` and a `thumbnail` of at most 160 pixels, kept with the feature so the project needs no other file. Identify shows the thumbnail. Photos without a position are named in a notice and left out; a layer holds up to 2,000 photos. |
| LiDAR point clouds | `.las`, `.laz` (COPC too) | Add Data, drop the file, upload, or From URL… | LAS 1.0–1.4 (point formats 0–10) and LAZ, on the desktop and in the web app: a point layer with each point's `z`, `intensity`, `classification`, `return` and colour (`red`, `green`, `blue`), coloured by ASPRS class (or by elevation when the cloud has one class). A cloud past 2 million points (500,000 in the web app) shows an evenly spread sample of them. Converted from the coordinate system its WKT or GeoTIFF-key record names (the horizontal part of a compound one); a file that names none keeps its coordinates, with a notice. Read in place, so a project keeps the file's path, not its points. |
| GeoPackage | `.gpkg` | Add Data, or upload in the web app | Every feature table becomes a layer, up to 32 at once (the result counts the rest in `skippedCount` and names up to 100 of them in `skipped`); `"layer"` picks one, and `info.fileLayers` lists them all. Read in place by its R-tree; any EPSG system is reprojected. The web app reads the file with its own SQLite reader (tables scanned, the envelope in each geometry header tested first; no R-tree); UTF-16 databases aren't read there. |
| GeoParquet | `.parquet`, `.geoparquet` | Add Data, or upload in the web app | WKB geometry, with the CRS from the file's `geo` metadata. Row groups are pruned by the bbox covering. Snappy, gzip, LZ4 and zstd compression are read, in the browser too; Brotli isn't. |
| FlatGeobuf | `.fgb` | Add Data, or upload | Read in place by its packed R-tree. |
| Standalone tables | CSV, or any layer without geometry | Add Data | Listed under the map in Layers; join or relate them to layers. |

Files with more than 100,000 features (GeoPackage, GeoParquet, FlatGeobuf, and Shapefile on the
desktop) are read in place, a view
at a time, rather than loaded whole: see [Large files](data.md#large-files). They draw, label,
identify and run through tools like any layer; **Load All Features** loads one to edit it.

## Formats read through GDAL

With [GDAL](https://gdal.org) installed (its `ogr2ogr` and `ogrinfo` on the `PATH`, GDAL 3.7 or
later), the desktop app also reads the vector formats below. Each layer is converted once into a
cached, indexed FlatGeobuf, so large ones stream too: see [Formats through
GDAL](data.md#formats-through-gdal).

| Format | Extensions or source | Notes |
| --- | --- | --- |
| Esri File Geodatabase | `.gdb` folder | Every feature class becomes a layer. |
| AutoCAD DWG | `.dwg` | Through GDAL's CAD driver, when it is built in. Give `"crs"` (such as `EPSG:2263`) when the drawing doesn't say its coordinate system. |
| MapInfo tables | `.tab` | Keep the companion files together. |
| OpenStreetMap XML | `.osm` | |
| GeoJSON Lines, GMT | `.geojsonl`, `.geojsons`, `.gmt` | |

## Files on the web

**Home › Add Data › From URL…** (`layer.addUrl` with `"url"`) downloads a file by its `https://`
(or `s3://`, see [S3 buckets](data.md#s3-buckets)) address and adds it like a file from this computer: GeoJSON, CSV, GeoParquet, FlatGeobuf,
GeoPackage, zipped Shapefiles, KML/KMZ, GPX or GeoTIFF/COG, up to 512 MB on the desktop and 256 MB
in the web app (where the server must allow requests from web pages, CORS). The format comes from
the address, or from the contents when the address has no extension (an API endpoint, say). The
app downloads in the background and adds the layer when the file arrives (to the project it was
started in: opening another meanwhile drops it). A `.pmtiles` address is added as a tile layer
and read a tile at a time instead.

A cloud-optimised GeoTIFF (a tiled `.tif` with overviews, as most published imagery is) is read
in place rather than downloaded: its header first, then, by HTTP range requests, only the tiles of
the overview each part of the view needs, so a Sentinel-2 scene of 300 MB draws in a couple of
seconds and full detail comes as you zoom in. It is a tile layer (`map.addTiles` kind `cog`,
saved in the project by its address), drawn under the map's own layers like other tile layers:
RGB(A) 8-bit images as they are, other images in grey stretched between the 2nd and 98th
percentiles of their coarsest overview. A GeoTIFF in strips, with bands stored apart, or without
tiles is downloaded whole as a raster layer, as is any GeoTIFF added with `"whole": true` (for
the raster tools and symbology). The web app needs the server to allow range requests from web
pages (CORS with the `Range` header), as public buckets such as Sentinel-2's do.

A GeoParquet or FlatGeobuf file by its address is read in place too, a view at a time, however
large: the layer is streamed like a large local file, and the view's features are fetched in the
background by HTTP range requests and drawn as they land. A FlatGeobuf's header and packed R-tree
index are read first (a level of the tree per round of requests), then only the features in the
view. A GeoParquet's footer is read first, then the row groups the view meets, by their bounding
box covering column (`bbox`) and its statistics: the covering of groups on the view's edge, then
the geometry and attribute columns of the rows taken. A view holding more than a read takes (60,000
features on the desktop, 30,000 in the web app) shows an evenly spread sample of it, in patches;
zoom in for all of them. So a 12 GB FlatGeobuf of 11 million areas opens after 65 KB and
draws a city in about a second from 9 MB, and a 525 MB Overture buildings file opens after 1 MB.
The project keeps the address and reads it the same way when it opens. A FlatGeobuf without an
index, a GeoParquet without a covering column, or a server that doesn't answer range requests
means the file is downloaded whole instead, as is any file added
with `"whole": true`; Load All Features and the geoprocessing tools fetch the whole file on the
desktop, while in the web app they need the file downloaded first.

`layer.addUrl` waits for the download, up to a minute, which suits the command line, MCP and the
desktop app's assistant; a larger file comes through the From URL… dialog, which downloads in the
background. In the web app, where nothing may wait, it adds `.pmtiles` and GeoTIFF addresses only
(a GeoTIFF read in place); other files, GeoParquet and FlatGeobuf read in place among them,
come through the From URL… dialog (`ui.addUrl`) or a `?data=` link. GeoJSON, CSV, KML and GPX files are read up to
256 MB; larger downloads (GeoParquet, FlatGeobuf, GeoPackage, GeoTIFF) up to the limits above.

## Map and feature services

**Home › Add Data › From Service…** connects to a service by its address, lists what it offers from
its capabilities, and adds a layer at a time (`service.layers` and `service.add` from scripts, the
command line and MCP).

| Service | What it lists | How a layer is added |
| --- | --- | --- |
| WMS 1.1.1 / 1.3.0 | Named layers offered in Web Mercator | A tile layer of GetMap images (PNG when the service offers it, else JPEG) |
| WMTS | Layers whose tile matrix set is Web Mercator's pyramid | A tile layer from the tile template (or KVP GetTile) |
| WFS 1.1 / 2.0 | Feature types | Their features downloaded as GeoJSON, in the output format the service names (up to 50,000) |
| OGC API Features | Collections | Their items downloaded as GeoJSON |
| OGC API Tiles | Collections' map and vector tiles; a landing page's dataset map tiles | A tile layer in WebMercatorQuad, from the tile template the tileset advertises (its `item` link; `…/tiles/WebMercatorQuad/{z}/{y}/{x}`, the usual path, when that can't be read), keeping the tiles link's query (a token), map tiles in the server's image format, vector tiles drawn layer by layer in colours of their own |
| WCS 1.0 / 2.0 (a 1.1 service is asked in 1.0) | Coverages, with their bounds | A raster: GetCoverage of the map's view within the coverage's bounds (or its whole extent: `"bbox": "full"`), as a GeoTIFF in EPSG:4326, at most 2048 pixels on a side |
| ArcGIS REST | FeatureServer and MapServer layers; a MapServer's map; an ImageServer; a VectorTileServer | Layers queried as GeoJSON (up to the server's record limit); a map as its tile cache when it is Web Mercator's pyramid, else exported images; an ImageServer as exported images; a VectorTileServer as its MapLibre style |

The type is guessed from the address (`?SERVICE=`, `/wmts`, `/wfs`, `/wcs`, `/collections`, `/rest/services`…) or
chosen (an OGC API landing page needs the OGC API type). A WebMercatorQuad tileset is found by its
tile matrix set, whatever the server calls it. A service that asks for a token says so. In the web app the server must allow requests
from web pages (CORS).

A coverage is read once, for the view when it was added (its DescribeCoverage first, for its axes,
format and bounds; `service.coverage` gives the GetCoverage address alone). Moving the map doesn't
ask for it again: add it again for another area. The server reprojects it to EPSG:4326 (WCS 2.0
through its CRS extension); one that can't answers with an error, which is shown.

### Catalogues

The same dialog's **Catalogue search (CSW)** type searches a CSW 2.0.2 catalogue (GeoNetwork,
pycsw, geocat.ch…) by words anywhere in its records and, if you like, the map's area
(`catalog.search`). Its records are listed 20 at a time (**More** asks for the next), with their
titles, abstracts (on hover) and bounds (**Zoom**), and a button per link RustGIS can follow: a WMS,
WMTS, WFS, WCS, OGC API or ArcGIS service connects in the dialog, a download is added as From URL…
does. Links pointing where a document on the web may not send RustGIS (the local network) are left
out.

## PostgreSQL and PostGIS

The desktop app reads the geometry and geography columns of a PostGIS database live, talking to the
server itself: see [PostgreSQL and PostGIS](data.md#postgresql-and-postgis).

| Source | How to add it | Notes |
| --- | --- | --- |
| PostGIS table or view | Add Data › Database… (`layer.addDatabase`) with a libpq connection string or `postgresql://` address | Large tables read a view at a time by their spatial index, others loaded whole. Converted to longitude/latitude by the server; `"crs"` for geometry that doesn't say. A column of mixed types becomes a layer per type. The password is kept for the session only. |

## SQL queries and DuckDB databases

With [DuckDB](https://duckdb.org) installed (the `duckdb` program on the `PATH`, DuckDB 1.5 or
later), the desktop app adds the result of an SQL query as a layer, and opens DuckDB database files.
Results are written as cached GeoParquet, so large ones stream too: see [SQL queries and DuckDB
databases](data.md#sql-queries-and-duckdb-databases).

| Source | How to add it | Notes |
| --- | --- | --- |
| SQL query | `layer.addSql` with `"sql"` | One DuckDB `SELECT` with DuckDB Spatial: Parquet, CSV, GeoParquet and `ST_Read` files, web addresses (`https://`, `s3://`). Give `"crs"` for a geometry that doesn't say its coordinate system. |
| DuckDB database | `.duckdb`, `.ddb`: Add Data | Every table with a geometry column becomes a layer (`"layer"` picks one). Converted once, until the file changes. |

## Rasters

| Format | Extensions or source | How to add it | Notes |
| --- | --- | --- | --- |
| GeoTIFF / Cloud-optimised GeoTIFF | `.tif`, `.tiff` | Add Data, or upload | Regular or cloud-optimised, striped or tiled; grey, RGB(A), palette (with its colour table) or multiband; Deflate, LZW, PackBits, JPEG or Zstandard compression. Any EPSG system, or the file's own projection from its GeoKeys (Albers, LCC, transverse Mercator…), reprojected as it draws. A large file opens from its overview, or one made as it is read when it has none. |
| ASCII grid | `.asc` (with an optional `.prj`) | Add Data, or upload | |
| Zarr | a store's folder (`.zarr`) or web address, format 2 (consolidated) or 3 | Add Data › Zarr… (`layer.addZarr`), Add Data on the folder, or a `?zarr=` link | A variable's latitude/longitude (or y/x) map at the steps picked of its other dimensions (time, level), read chunk by chunk in pure Rust (blosc, zlib, gzip, zstd, LZ4; sharded arrays too), on the desktop and the web. See [Zarr stores](data.md#zarr-stores). |
| Rasters GDAL reads | `.jp2`, `.img`, `.hgt`, `.dem`, `.dt0`–`.dt2`, `.nc`, `.hdf`, `.h5`, `.bil`, `.grd`, `.sdat`, `.kea`, `.ers`, `.rst`, `.ntf`, `.ecw`, `.sid`… | Add Data (desktop, with [GDAL](https://gdal.org) installed: `gdal_translate`, `gdalinfo` and `gdalwarp`) | Converted once to a cloud-optimised GeoTIFF in the cache, then read as one. A NetCDF or HDF file's variables are its subdatasets: the first is added, the others are named in a notice, and `"layer"` picks one. A file that doesn't say its coordinate system is read as WGS 84 when its coordinates look like longitude/latitude (0–360° longitudes are moved to -180–180°); otherwise give `"crs"`. |

## Tiles and web services

| Format | Extensions or source | How to add it | Notes |
| --- | --- | --- | --- |
| Raster tiles | XYZ/TMS template (`{z}/{x}/{y}`, `{-y}`, `{s}`, `{q}`), WMS (`{bbox-epsg-3857}`) | Home › Add › Basemap › Add Tile Layer | PNG, JPEG and WebP. |
| Vector tiles | Mapbox Vector Tiles (`.pbf`, `.mvt`) template, or TileJSON | Add Tile Layer | Drawn in a built-in style that knows the OpenMapTiles, Protomaps and Shortbread schemas. |
| MapLibre styles | `style.json` | Add Tile Layer | The style's vector and raster sources, sprite, expressions and zoom functions. |
| PMTiles | `.pmtiles`: a web address, or a file on the desktop | Add Tile Layer (or its **Tile Archive File…** button), Add Data, or drop the file | Raster or vector, as the archive says. Read a tile at a time by byte ranges (the server must allow them, and CORS in the web app). A vector archive is drawn in the built-in style when it is a basemap, otherwise with each of its layers in its own colour. |
| MBTiles | `.mbtiles`: a file | Add Data, drop the file, Add Tile Layer's **Tile Archive File…** button, or upload in the web app | Raster (PNG, JPEG, WebP) or vector tiles, as its metadata says, drawn as PMTiles archives are. The desktop reads the file in place, a tile at a time. The web app reads an upload into memory (the plain `tiles` table and the deduplicated layouts of mbutil, TileMill and Planetiler) and keeps it for the session: a project saved there asks for the file again. |

## Writing

| Format | How to write it | Notes |
| --- | --- | --- |
| GeoJSON, CSV | Data › Export (`share.exportLayer`) | The Export Features dialog picks the format; the file is saved in it whatever extension is typed. In the web app it downloads (any format below but GeoPackage; a Shapefile as one zip). A streamed layer writes its whole source (as every format below does). |
| GeoPackage | Data › Export to `.gpkg` (desktop) | One feature table (an attributes table for a layer without geometry) in WGS 84, with an R-tree spatial index and the GeoPackage 1.4 triggers that keep it current when other software edits the file. Feature ids are kept. |
| GeoParquet | Data › Export to `.parquet` | GeoParquet 1.1: WKB geometry in WGS 84, Snappy compression, row groups of 65,536 features and a `bbox` covering column so readers can skip rows by extent. |
| Shapefile | Data › Export to `.shp` (or `.zip` for the files zipped) | Writes `.shp`, `.shx`, `.dbf`, `.prj` (WGS 84) and `.cpg` (UTF-8). Field names are cut to 10 characters (made unique), text to 254 bytes, and numbers too wide for a dBase field are left empty; features without geometry, and lines of a single point, are left out (and reported). |
| FlatGeobuf | Data › Export to `.fgb` | With a spatial index; features without geometry are left out (and reported). |
| KML, KMZ, GPX | Data › Export to `.kml`, `.kmz` or `.gpx` | Placemarks named by a `name` field, if there is one; features without geometry are left out. |
| KML, GPX, CSV with WKT | Analyze › Workflows › Data Interop | |
| GeoTIFF | Data › Export | |
| PNG | Publish › Export Map (`share.exportMap`) | At any DPI, with labels, scale bar and tile credits. |
| PDF | Layouts, reports and presentations | |
| Wavefront OBJ, glTF | Publish › Export To 3D, Share As › Web Scene | Polygons extruded by a field. |
| Web map, OGC API Features, MapLibre style | Publish › Share As | Open-format files that need no server: see [Sharing](sharing.md). |
| Raster tiles | Analyze › Create Hosted Imagery | |

## RustGIS files

| File | Extension | Notes |
| --- | --- | --- |
| Project | `.rgis` (older `.rgisx` open too) | Saves streamed layers' file paths, not their features; a browser upload is kept inside. |
| Map, layer, style, color ramp, template, task, bookmark and layout files | `.rgismap`, `.rgislyr`, `.rgisstyle`, `.rgisramp`, `.rgistemplate`, `.rgistask`, `.rgisbookmarks`, `.rgislayout` | See [Project items](data.md#project-items). |
| Project package | `.rgispkg` | Everything, rasters included. |
| Plugin | `.rgplugin` | See [Plugins](../plugins.md). |
| Color ramps from other tools | QGIS style `.xml` and layer style `.qml`, GDAL color-relief `.txt`, GMT `.cpt` | Read as custom color ramps; see [Custom color ramps](cartography.md#color-ramps-from-other-tools). |
