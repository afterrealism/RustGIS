# Data

## Formats

Every format, with how to add it and its limits, is listed on [Supported Data
Formats](data-formats.md).

| Format | Read | Write |
| --- | --- | --- |
| GeoJSON (`.geojson`, `.json`) | Yes | Yes |
| Delimited text (`.csv`, `.tsv`, `.txt`) with x/y columns in any coordinate system, or WKT geometry | Yes | Yes |
| Shapefile (`.shp` + `.dbf`, or zipped in a `.zip`) | Yes (the web app opens zips) | Yes (`share.exportLayer`) |
| GeoPackage (`.gpkg`, every feature table) | Yes | Yes (desktop) |
| GeoParquet (`.parquet`, `.geoparquet`, WKB geometry) | Yes | Yes |
| FlatGeobuf (`.fgb`) | Yes | Yes (`share.exportLayer`) |
| KML/KMZ (with their styles), GPX | Yes | Yes |
| Spreadsheets (`.xlsx`, `.xls`, `.ods`), GeoRSS feeds, LandXML, encoded polylines | Yes | No |
| GML, MapInfo MIF/MID, OpenStreetMap PBF, TopoJSON, AutoCAD DXF (ASCII) | Yes | No |
| LiDAR point clouds: LAS, LAZ, COPC (an evenly spread sample) | Yes | No |
| Geotagged photos (JPEG with EXIF GPS): points with their thumbnails | Yes | No |
| Through GDAL: File Geodatabase (`.gdb` folder), AutoCAD DWG, MapInfo TAB, OpenStreetMap XML… | Yes (desktop, when GDAL is installed) | No |
| PostgreSQL/PostGIS databases, read live (large tables a view at a time) | Yes (desktop) | No |
| DuckDB databases (`.duckdb`, `.ddb`) and SQL queries | Yes (desktop, when DuckDB is installed) | No |
| KML, GPX, CSV with WKT geometry | No | Yes (Analyze › Workflows › Data Interop) |
| GeoTIFF and cloud-optimised GeoTIFF (`.tif`, `.tiff`) | Yes | Yes |
| ASCII grid (`.asc`, with an optional `.prj`) | Yes | No |
| Through GDAL: JPEG 2000, ERDAS Imagine, SRTM HGT, DTED, NetCDF, HDF, ENVI, SAGA… | Yes (desktop, when GDAL is installed) | No |
| Web tiles: XYZ/TMS/WMS rasters, Mapbox Vector Tiles, MapLibre styles, PMTiles and MBTiles archives | Yes | Yes (Analyze › Create Hosted Imagery) |
| RustGIS project (`.rgis`; older `.rgisx` files open too) | Yes | Yes |

Add data with **Home › Add Data** (**From URL…** adds a file by its web address, reading a
cloud-optimised GeoTIFF, GeoParquet or FlatGeobuf in place, a view at a time, and downloading
others; see [Files on the web](data-formats.md#files-on-the-web)), by dropping files on the window, or by
passing them on the command line (`rustgis places.geojson`). **Home › Add Data › Delimited
Text…** asks for a CSV or a spreadsheet first, then lists its columns to pick the x and y (or WKT)
ones from, with the coordinate system (such as `EPSG:2263`) and the separator (changing it reads
the columns again); `info.fileColumns` lists them for scripts. Add Data finds them itself
(columns named like longitude/latitude, easting/northing or geometry, and EWKT's `SRID=`) and
says when coordinates aren't longitude/latitude. A CSV without coordinate columns, or a layer without geometry, opens as a
standalone table, listed under the map in Layers; right-click it to open or remove it. Tables
can share a name: commands and tools take a table's id (`info.project` lists them) as well as its
name, which stands for the first table of that name. An id given as a number is never taken for
a name. The Tools pane and Geocode Addresses send the id of the table picked; a tool's run keeps
the table's name beside it (`input_name`). The **XY Table To Point** tool
([Geoprocessing](analysis.md), and a table's right-click menu) makes points of a table already in
the project, from two fields of coordinates and their coordinate system. In a text field a single comma is the decimal separator
(`1,234` is 1.234); thousands separators are not read.
**Home › Add Data › Graphics Layer** adds a layer for [drawn graphics](cartography.md#graphics),
and **Insert › Layer Templates** adds empty layers with ready-made schemas to draw into.

Vector coordinates are WGS 84 longitude/latitude and are displayed in Web Mercator. GeoPackage,
GeoParquet and FlatGeobuf files in another coordinate system (any EPSG code) are converted as they
are read. Rasters keep
their own coordinate system (any EPSG code, such as UTM zones or national grids) and are
reprojected on the fly as they draw.

## Formats through GDAL

When [GDAL](https://gdal.org) is installed (its `ogr2ogr` and `ogrinfo` programs on the `PATH`,
or the folder holding them in `RUSTGIS_GDAL_BIN`; GDAL 3.7 or later), the desktop app also opens
the vector formats GDAL reads: File Geodatabases, DWG, MapInfo TAB, OpenStreetMap XML
and more. RustGIS doesn't link to GDAL: each layer is converted once, by `ogr2ogr`, into a
FlatGeobuf with a spatial index in WGS 84 (GDAL reprojects it), kept in the cache folder
(`~/.cache/rustgis/gdal` on Linux, `~/Library/Caches/RustGIS/gdal` on macOS,
`%LOCALAPPDATA%\RustGIS\cache\gdal` on Windows, or `RUSTGIS_GDAL_CACHE`). Adding the file again
reuses the conversion until the file changes, and large layers stream like any large FlatGeobuf
(below). Every layer of the file is added; a layer mixing geometry types is
added as one layer per type. Projects remember the original file and convert it again if the
cached copy is gone.

## SQL queries and DuckDB databases

When [DuckDB](https://duckdb.org) is installed (the `duckdb` program on the `PATH`, or the folder
holding it in `RUSTGIS_DUCKDB_BIN`; DuckDB 1.5 or later), the desktop app adds the result of an SQL
query as a layer, with the `layer.addSql` command (Add Data From SQL in command search):

```json
{"sql": "SELECT name, geometry FROM read_parquet('places.parquet') WHERE country = 'FR'", "name": "French places"}
```

**Analyze › SQL** opens the SQL pane, a workspace for these queries: an editor (Ctrl+Enter runs
the query) that completes table and column names of the database named above it and DuckDB
Spatial's common functions, the result's first 200 rows as a table, **Add to Map** for a result
with a geometry column, and a history of the queries run (kept by the app with its preferences,
without the database's password; a query that seems to hold a credential, such as `user:password@`
in an address, a signed URL or a secret, isn't kept). Queries run in the background, so the app keeps drawing while DuckDB works. The
commands behind it are `sql.query` (a query's first rows, up to 1,000, without adding a layer) and
`sql.schema` (a database's tables and their columns); both take `"database"` as `layer.addSql`
does. The web app has no DuckDB, so the pane says to open the project in the desktop app.

The query is one DuckDB `SELECT` (or `FROM …`, `WITH …`), with [DuckDB
Spatial](https://duckdb.org/docs/stable/core_extensions/spatial/overview)'s `ST_` functions: it can
read Parquet, CSV, JSON and GeoParquet files, anything `ST_Read` reads, and web addresses (`https://`,
`s3://`), so a bounding box of [Overture Maps](https://overturemaps.org) is one query:

```sql
SELECT id, names.primary AS name, geometry
FROM read_parquet('s3://overturemaps-us-west-2/release/2026-09-23.1/theme=places/type=place/*', hive_partitioning = 1)
WHERE bbox.xmin BETWEEN -83.95 AND -83.89 AND bbox.ymin BETWEEN 35.94 AND 35.98
```

The first geometry column becomes the layer's geometry; other columns become its attributes (types
RustGIS doesn't read, such as dates and lists, as text). A geometry that says its coordinate system
is converted to longitude/latitude; give `"crs"` (such as `EPSG:2263`) for one that doesn't. A
result mixing geometry types becomes a layer per type, and rows without a geometry are left out.
`"database"` opens a DuckDB database (read only) so the query names its tables plainly; the
database is trusted like the query, since its views run as part of it.

**Home › Add Data** opens a DuckDB database file (`.duckdb`, `.ddb`): every table with a geometry
column becomes a layer (`"layer"` picks one).

### PostgreSQL and PostGIS

**Home › Add Data › Database…** (`layer.addDatabase`) connects to a PostgreSQL database with
PostGIS and reads its tables live. RustGIS talks to the server itself (nothing else to install;
TLS as libpq does it). `sslmode=prefer` (the default) and `require` encrypt the connection but
don't check the server's certificate, so they don't protect against a server in the middle:
use `verify-full` (or `verify-ca`, which here checks the host name too) on networks you don't
trust. The certificate is checked against `sslrootcert` (a private authority's PEM file) or the
web's roots. `disable` turns TLS off. The connection is a libpq connection string
(`host=localhost port=5432 dbname=gis user=postgres`) or a `postgresql://user@host/dbname`
address; what it leaves out comes from `PGHOST`, `PGPORT`, `PGUSER` and `PGDATABASE`, as with
libpq. **List Tables** (`info.databaseTables`) shows the tables and views with a geometry or
geography column, and every one of them becomes a layer unless one is chosen (`table`,
`schema.table`, or `table:column` for one of several geometry columns). The connection is read
only.

A table of more than 100,000 rows (or any, with `"stream": true`) is read in place a view at a
time, like a large GeoParquet: the view's box is matched against the column's spatial index, and
when more rows match than a view draws, an evenly spread sample of them is numbered on the server,
so only those cross the network. Smaller tables are loaded whole, so they can be edited. The
server converts geometry to longitude/latitude (`ST_Transform`, so every coordinate system in its
`spatial_ref_sys` works) and curves to lines; give `"crs"` (such as `EPSG:2263`) for a column whose
geometry doesn't say its coordinate system. A column of mixed geometry types becomes a layer per
type, and empty tables (such as the TIGER geocoder's) are left out. Features keep their ids between
views: the table's integer primary key, else the row's place in the table (the rows of a view or
a partitioned table without a key are numbered as read, so theirs can change between views).

The password is kept until RustGIS closes and never written to the project, which names the
connection without it. Without one, `PGPASSWORD` or libpq's password file (`~/.pgpass`, or
`%APPDATA%\postgresql\pgpass.conf`) serve when you add a table. A project keeps a streamed
table's connection and reads it again when it opens, sending only a password given in this
session: a project file could name any host, and must never get your own credentials. When the
server wants a password, connect again with **Add Data › Database…** (the layer reads again when
the project is next opened). A small table's features are kept in the project as they were when
added.

`layer.addSql` takes a connection as its `"database"` too, through DuckDB's `postgres` extension
(installed the first time, like DuckDB Spatial), so a query names its tables plainly; the password
reaches DuckDB in its environment (`PGPASSWORD`), never on its command line:

```sh
rustgis-cli run --cmd 'layer.addSql={"sql":"SELECT name, geom FROM roads WHERE lanes > 2","database":"postgres:host=localhost dbname=gis user=postgres","password":"…"}'
```

RustGIS doesn't link to DuckDB: it runs `duckdb`, which writes the result as GeoParquet in the
cache folder (`~/.cache/rustgis/duckdb` on Linux, `~/Library/Caches/RustGIS/duckdb` on macOS,
`%LOCALAPPDATA%\RustGIS\cache\duckdb` on Windows, or `RUSTGIS_DUCKDB_CACHE`), sorted along a
Hilbert curve with a bounding-box covering, so large results stream like any large GeoParquet
(below). DuckDB Spatial is installed the first time (a download from DuckDB's extension
repository). A database table is converted once and reused until the file changes; a query runs
afresh each time. Projects convert a table again if the cached result is gone, but never run a
query when they open: run it again. A database is read on its own, shut out of other files and web
addresses, so a view in it that reaches elsewhere fails rather than reading them.

A query can read any file the app can and fetch any web address, and a database is a file or a
server, so the [AI Assistant](assistant.md) asks before it runs a query or reads a database, and
plugins need both the files and network permissions.

## STAC catalogs

**Home › Add Data › STAC Search…** searches a [STAC](https://stacspec.org) API for imagery and
other assets: by collection, dates, the most cloud cover and (by default) where the map is. The
default API is Element 84's [Earth Search](https://earth-search.aws.element84.com/v1)
(Sentinel-2, Landsat, NAIP, Copernicus DEM and Sentinel-1 on AWS); Microsoft's [Planetary
Computer](https://planetarycomputer.microsoft.com/api/stac/v1) is offered beside it, and any
STAC API with item search works. **Collections** lists what the API offers, **Find Items** shows up to 50 items (date, cloud
cover, id) and draws their footprints as a layer; a click frames an item, and **Add** adds the
asset chosen beside it: `visual` (true colour) when there is one, else the first GeoTIFF, else the
first data asset. A cloud-optimised GeoTIFF is read in place, the tiles each view needs by range
(see [Files on the web](data-formats.md#files-on-the-web)); other assets download whole.

The commands do the same: `stac.collections` (`"url"`), `stac.search` (`"collections"`,
`"bbox"` as `[west, south, east, north]` or `"view"`, `"datetime"` such as
`2024-06-01/2024-06-30`, `"limit"`, `"maxCloud"`, `"footprints"`), and `stac.addAsset` with
an item `stac.search` returned (`"item"`) or an item's address (`"url"`) and `"asset"`:

```sh
rustgis-cli run --cmd 'stac.search={"collections":"sentinel-2-l2a","bbox":[7.4,46.9,7.5,47],"datetime":"2024-07-01/2024-07-31","maxCloud":20}' --cmd 'stac.addAsset={"url":"https://earth-search.aws.element84.com/v1/collections/sentinel-2-l2a/items/S2B_32TLS_20240727_0_L2A","asset":"scl"}'
```

Assets are added by their web address: one in a cloud store's own protocol (`s3://`) is refused
with a message saying so, and an item on the web can't point into the local network. A Planetary
Computer asset (in Azure Blob Storage) is signed first: RustGIS asks the Planetary Computer's
token service for its storage container's token and reuses it for half an hour. The token is part
of the layer's address, so a project saved with one reads it again only while the token lasts
(about an hour): add the asset again after that. A `?stac=` link opens the web app on an item or a
collection ([web app links](../installation.md)). Searches reach the network, so the [AI
Assistant](assistant.md) asks before it runs them, and plugins need the network permission.

## Overture Maps

**Home › Add Data › Overture Maps…** adds [Overture Maps](https://overturemaps.org)' open data:
buildings, places, transportation (segments and connectors), administrative divisions, base
layers (land use, land, land cover, water, infrastructure, bathymetry) and addresses, from the
latest release (or one named with `"release"`). Two ways:

- **The features where the map is.** A theme's type is a few hundred GeoParquet files, each
  covering an area; Overture's [STAC catalog](https://stac.overturemaps.org) gives every file's
  extent, so only the files that cover the map's view are read, in place by range like any
  GeoParquet by address (see [Files on the web](data-formats.md#files-on-the-web)), and each
  becomes a layer read a view at a time. Zoom to a city or smaller: a view that takes more than six
  files is refused (a country takes dozens).
- **Map tiles of the whole world**, from the theme's PMTiles archive: every zoom, drawn a layer per
  type. Some themes' tiles hold their features only when zoomed in (places from zoom 14).

`overture.add` does the same (`"theme"`, `"type"`, `"bbox"` or the view, `"tiles": true` for the
tiles):

```sh
rustgis-cli run --cmd 'map.zoomToExtent={"extent":[7.40,46.93,7.47,46.97]}' --cmd 'overture.add={"theme":"buildings"}' --cmd 'overture.add={"theme":"transportation"}' --cmd 'share.exportMap={"path":"bern.png"}'
```

The data's licences differ by theme (OpenStreetMap's ODbL for most, CDLA Permissive 2.0 for
places); the layer says which.

## Source Cooperative

**Home › Add Data › Source Cooperative…** lists [Source Cooperative](https://source.coop)'s
featured products (or an account's), browses a product's folders and adds a file by its address on
the data proxy (`https://data.source.coop/<account>/<product>/…`): GeoParquet, FlatGeobuf,
cloud-optimised GeoTIFFs and PMTiles are read in place, other files downloaded.
`sourcecoop.products` (`"account"`) and `sourcecoop.list` (`"account"`, `"product"`, `"path"`)
list them; a listed file's `"url"` is added with `layer.addUrl`. Like STAC searches, these reach the
network: the AI Assistant asks first and plugins need the network permission.

## Geocoding addresses

**Home › Add Data › Geocode Addresses…** (`layer.geocode`) turns a table's addresses into points:
a CSV of addresses without coordinates is added as a standalone table, then each row's address
fields (`"address"`: a field, or several such as street, city and country, joined with commas)
are looked up, each distinct address once. The result is a new point layer holding every field
plus `geocoded`, what the geocoder called the place; rows that weren't found stay in its table
without a point.

The geocoder is OpenStreetMap's public [Nominatim](https://nominatim.org) by default, or
[Photon](https://photon.komoot.io) (`"provider": "photon"`). Their usage policies allow light
use, so RustGIS sends them at most one request a second and 100 addresses a run; give `"url"` of
a Nominatim or Photon server of your own for more (up to 10,000, with no wait between requests).
In the app the requests go out in the background, one at a time, and the layer is added when the
last answer lands; `layer.geocode` on the command line waits for them.

```sh
rustgis-cli run --cmd 'layer.addData={"path":"stores.csv"}' --cmd 'layer.geocode={"table":"stores","address":["street","city","country"]}'
```

The addresses are sent to the geocoder's server, so the [AI Assistant](assistant.md) asks before
it geocodes, and plugins need the network permission.

## OpenStreetMap and administrative boundaries

**Home › Add Data › OpenStreetMap…** (`osm.download`) downloads OpenStreetMap data where the map
is from an [Overpass](https://overpass-api.de) server: a preset (buildings, roads, railways, water,
land use, parks, points of interest or administrative boundaries) or tag filters such as
`amenity=cafe, shop`. The answer becomes up to three layers, points, lines and polygons: closed
ways that are areas (buildings, land use and the like, as OSM reads their tags) are polygons, and
multipolygon and boundary relations are assembled from their members. A layer's fields are its 25
most common tags, the rest in `other_tags` as GDAL writes them. The area can be up to 0.25 square
degrees (about 50 × 50 km), and an answer up to 50,000 elements: the public servers are shared, so
a larger area is better read from an extract (`.osm.pbf`). `"url"` names another Overpass server
when the public one is busy.

**Home › Add Data › Boundaries…** (`boundaries.add`) adds a country's administrative boundaries from
[geoBoundaries](https://www.geoboundaries.org) (CC BY 4.0): the country itself (`ADM0`), its states
or provinces (`ADM1`) and finer divisions down to `ADM5` where they exist, simplified for the map.
`boundaries.countries` lists the countries.

```sh
rustgis-cli run --cmd 'osm.download={"bbox":[7.44,46.944,7.456,46.952],"preset":"buildings"}' --cmd 'boundaries.add={"country":"KEN","level":"ADM1"}'
```

Both reach the network: the [AI Assistant](assistant.md) asks before it runs them, and plugins need
the network permission. OpenStreetMap data is © OpenStreetMap contributors (ODbL).

## Open-data portals

**Home › Add Data › Search Open Data…** (`portal.search`) finds datasets by text in an open-data
portal and lists, for each, the resources RustGIS can add, the best format first:

- **data.gov and CKAN catalogues**: [catalog.data.gov](https://catalog.data.gov) by default, or a
  CKAN site by its address (`https://open.canada.ca/data`, `https://data.gov.uk`…). GeoJSON,
  FlatGeobuf, GeoParquet, GeoPackage, KML, zipped Shapefiles, GeoTIFF, CSV, WMS, WFS and ArcGIS
  REST resources are offered; web pages, PDFs and the like are left out.
- **Socrata**: every Socrata site through its discovery API, or one site such as
  `data.cityofnewyork.us`. Datasets with a location column are added as their GeoJSON export (up
  to 50,000 rows).
- **ArcGIS Hub**: [hub.arcgis.com](https://hub.arcgis.com) or an organisation's Hub site; its
  feature services.

**Only where the map is** keeps to datasets covering the map's view. A file is added by its address
(`portal.add`, read in place when it can be, as **Add Data From URL…** does); a service opens
**Add From Service…** connected to it, to pick its layer (`portal.add` takes the first layer with
features unless `"layer"` names one). A search lists up to 50 datasets.

```sh
rustgis-cli run --cmd 'portal.search={"portal":"socrata","url":"data.cityofnewyork.us","query":"parks"}'
rustgis-cli run --sample --cmd 'portal.add={"url":"https://data.cityofnewyork.us/resource/enfh-gkve.geojson?$limit=50000","name":"Parks","portal":"https://api.us.socrata.com"}'
```

An address a portal lists is judged as one found in a document on the web: it may not point into
the local network. Both commands reach the network, so the AI Assistant asks before it runs them,
and plugins need the network permission. A dataset's licence is the portal's: its page (the
**Page** link) says what it is.

## S3 buckets

An `s3://bucket/key` address works wherever data is added by its address: **Add Data › From
URL…** and `layer.addUrl` (a cloud-optimised GeoTIFF is read in place by range, other files are
downloaded), PMTiles archives, `?data=` links, and STAC assets whose address is an `s3://` one.
A public bucket needs nothing more: it is read anonymously from AWS.

**Home › Add Data › S3…** (`s3.connect`) connects to a private bucket, or to an S3-compatible
service (MinIO, Cloudflare R2, Wasabi, source.coop…) by its endpoint, and browses a bucket a
folder at a time (`s3.list`), adding an object with **Add**. A connection names its endpoint (AWS
when empty), its region and the buckets it serves: the ones it is given (`"buckets"`) and the ones
browsed through it this session. An `s3://` address of one of them is read through that
connection, signed (AWS Signature Version 4) with its keys; an address of any other bucket (a
project's, a link's) is read anonymously from AWS, never with your keys or from your endpoint.
Keys are typed, or, on the desktop and only when chosen, read from an AWS profile
(`"profile"`: `~/.aws/credentials` and `~/.aws/config`) or from the environment
(`"environment": true`: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`,
`AWS_REGION`); without any, the connection is anonymous. **AWS, anonymous** in the dialog browses
with no keys at all.

```sh
rustgis-cli run --cmd 's3.connect={"name":"minio","endpoint":"https://minio.example.org","buckets":["my-bucket"],"accessKeyId":"…","secretAccessKey":"…"}' \
  --cmd 's3.list={"url":"s3://my-bucket/rasters/"}' --cmd 'layer.addUrl={"url":"s3://my-bucket/rasters/dem.tif"}'
```

Keys are kept until RustGIS closes: never in a project (which keeps the `s3://` address), a
command's result, a notice or the log. **Remember** keeps a connection's name, endpoint and region
in the app's settings, never its secret; storing secrets in the system keychain isn't supported.
Keys go only to the endpoint they're for: a signed request that is redirected isn't followed, and
keys from a profile or the environment are used only with AWS or the profile's own
`endpoint_url`. In the web app, the bucket's CORS rules must allow the page's origin (and, for a
private bucket, the `Authorization` and `x-amz-*` headers). The [AI Assistant](assistant.md) asks
before it connects or lists; plugins can't connect, and need the network permission to list.

## Data Sources

The **Data Sources** pane (**View › Windows › Data Sources**, `ui.pane.sources`) gathers the
places data comes from:

- **Favourites**: any source, right-clicked and added.
- **Recent**: the last 20 sources added (files, web addresses, tile layers, services, database
  tables, Zarr variables, STAC assets, sample data), newest first.
- **Folders**: the folders connected to the Browser (**Add Folder…**; desktop).
- **Services**, **Databases** (desktop) and **STAC APIs**: addresses saved with the box under each.
  A click opens Add From Service, Add From Database or STAC Search connected to it, which lists
  its layers, tables or collections.
- **S3**: this session's connections; a click opens the S3 browser on it.
- **Library**: well-known public sources: OpenStreetMap tiles, EOX Sentinel-2 cloudless and
  NASA GIBS (WMTS), USGS imagery, topographic and hydrography maps (ArcGIS), GEBCO bathymetry
  (WMS), Earth Search and Planetary Computer (STAC), and the Natural Earth sample layers.

A click on a file, a web address, a tile layer or a sample adds it at once. The lists are kept
with the app's preferences, never in a project. They hold no secrets: a web address loses its
password and any key, token or signature in its query (`token=`, `sig=`, `X-Amz-*`…), and a
database is kept without its password. The commands `sources.list`, `sources.save`,
`sources.favourite` and `sources.remove` read and change the lists, for the command line and the
[AI Assistant](assistant.md). On the command line they last for the run.

## Large files

GeoPackage, GeoParquet, FlatGeobuf and (on the desktop) Shapefile files with more than 100,000
features are read in place rather than loaded whole, so files far larger than memory open in a
moment. The layer reads the features in view, using the file's own spatial index (the GeoPackage
R-tree, the FlatGeobuf packed R-tree, GeoParquet row-group bounding boxes; for a Shapefile, the
shapes' bounding boxes, read once when it opens), in the background as the map moves. When a
view holds more than 250,000 features (100,000 in the web app), the layer draws an evenly spread
sample of them and its tooltip in Layers says so: zoom in to see them all. Smaller files load
whole; `layer.addData` with `"stream": true` or `false` overrides the choice.

On the desktop, a GeoJSON FeatureCollection larger than 32 MB is read the same way, with no size
limit: opening reads the file once, keeping each feature's place in it and its bounding box (a
file over 256 MB through a buffer, so it is never held whole), and each view parses just the
features it shows. A single feature, a bare geometry or TopoJSON is loaded whole.

A streamed layer works like any other for symbology, labels, pop-ups, selection and the
attribute table (which lists the features read for the view). Geoprocessing tools read the whole
file. Edits are refused, since the features are read again as the view moves: right-click the
layer in Layers and choose **Load All Features** (`layer.loadAll`) to load it into memory and
edit it. Projects save the layer's file path, not its features, and read the file again when they
open; a browser upload is kept in the project.

| File | Features | A view read |
| --- | --- | --- |
| GeoParquet, 0.1° grid (50 MB) | 1.6 million polygons | 183,000 in 0.15 s |
| FlatGeobuf, the same grid (335 MB) | 1.6 million polygons | 183,000 in 0.11 s |
| GeoPackage, the same grid (344 MB) | 1.6 million polygons | 183,000 in 0.18 s |
| GeoParquet, global grid (190 MB) | 6.5 million polygons | a sample of 136,000 in 0.27 s |

## Rasters

![A shaded-relief elevation raster of the Sierra Nevada with the Style tab open](https://assets.rustgis.app/images/raster.webp)

Add a GeoTIFF or an ASCII grid with **Add Data**, by dropping it on the window, or in the web app
by picking it. An ASCII grid takes its coordinate system from a `.prj` file next to it (as a Shapefile does);
otherwise, and always in the web app, which only sees the one file, it must be in
longitude/latitude degrees. Exporting a raster opened from an overview writes that overview.
GeoTIFFs may be regular or cloud-optimised: grey, RGB(A), palette (coloured by the file's colour
table, as land cover such as the NLCD is) or multiband, of any pixel type, tiled or striped, and
compressed with Deflate, LZW, PackBits, JPEG or Zstandard. The coordinate system is an EPSG code,
or the file's own projection described in its GeoKeys (Albers, Lambert conformal conic, transverse
Mercator, Mercator, Lambert azimuthal, stereographic, equirectangular and others), which an
exported GeoTIFF keeps. A raster too large to hold in memory (more than 120 million samples, 32
million in the web app) opens from its largest overview that fits; one without overviews opens as
an overview made as it is read, a strip or tile at a time (every second, fourth… pixel each way),
so reading it takes as long as decoding the whole file once. A cloud-optimised GeoTIFF added by its
web address is read in place instead, a tile at a time (see [Files on the
web](data-formats.md#files-on-the-web)).

The web app opens a picked or dropped file of up to 256 MB whole. A larger GeoTIFF is read a part
at a time instead: its header, then only the tiles of the overview it opens from. A
cloud-optimised file opens however large it is, as long as those parts fit in 256 MB (a 436 MB
EnMAP hyperspectral scene reads about 20 MB, the 1.6 GB NLCD land cover about 14 MB).
The parts read are what a saved project keeps. A file that isn't cloud-optimised (in strips, or
without overviews) needs most of itself, and past 256 MB the web app says to open it in the
desktop app.

With a raster layer selected in Layers, the **Style** and **Data** tabs show its groups:

| Tab › Group | Controls |
| --- | --- |
| Style › Visibility Range, Effects | Scale range, transparency |
| Style › Rendering | **Symbology**: Stretch, RGB, Classify, Unique Values, Colormap, Shaded Relief; **Stretch Type**: None, Minimum Maximum, Percent Clip, Standard Deviation, Histogram Equalize; **DRA** (stretch the pixels in view); **Resampling** (nearest or bilinear); **Band Combination** (natural colour, colour infrared, or any bands) |
| Style › Enhancement | Brightness, contrast, gamma |
| Data | Raster information (size, bands, statistics, coordinate system) and Export Raster (GeoTIFF) |

Clicking a raster with the Explore tool shows its pixel values. Projects remember a raster's file
path and reopen it; rasters added in the browser are stored inside the project.

### Zarr stores

**Home › Add Data › Zarr…** (`layer.addZarr`) adds a variable of a [Zarr](https://zarr.dev) store
(format 2 or 3), the multidimensional arrays climate and Earth-observation data are kept in: give
the store's web address, or (on the desktop) its folder, and **Connect** lists its variables
(`raster.zarrInfo`). A variable's map is its latitude/longitude (or y/x) dimensions; its other
dimensions, such as time or pressure level, are picked a step at a time with a slider, labelled by
their coordinates (CF time units as dates, the `noleap` and `360_day` model calendars included).
**Add to Map** adds that step as a raster layer named after the variable and step (`tas
2014-07-15`), and the dialog stays open to add others. Only the chunks the step needs are fetched,
a few at a time, in the background.

Stores are read in pure Rust, in the web app as well as on the desktop: Zarr 2 with consolidated
metadata (`.zmetadata`, as xarray writes) and Zarr 3 (`zarr.json`, sharded arrays too), chunks
compressed with blosc (LZ4, zlib or zstd, byte- or bit-shuffled), zlib, gzip, zstd or LZ4, of
integer or float values, in C or Fortran order. A sharded array's shards are fetched whole.
`_FillValue`, `missing_value` and a Zarr 2 array's fill value are no data; a Zarr 3 array's fill
value isn't: chunks never written are left empty, but chunks missing from inside a shard show the
fill value. `scale_factor` and `add_offset` are applied. The coordinates must be evenly spaced (a curvilinear grid isn't read);
longitude/latitude grids of 0–360° are turned to -180–180°, and projected ones take their
coordinate system from the variable's grid mapping (`crs_wkt`, `spatial_ref`, an EPSG code) or
`"crs"`. Blosclz-compressed and transposed chunks and shards inside shards aren't read yet, and
say so. A store by web address must allow the browser to fetch it (CORS) for the web app;
Google Cloud's CMIP6 bucket does, ARCO-ERA5's doesn't (the desktop reads both).

```sh
rustgis-cli run --cmd 'layer.addZarr={"url":"https://storage.googleapis.com/gcp-public-data-arco-era5/ar/1959-2022-full_37-1h-0p25deg-chunk-1.zarr-v2","variable":"2m_temperature","index":{"time":539100}}'
```

A store in a folder is saved in the project by its folder and slice (`2m_temperature[time=539100]`)
and read again when the project opens, as Add Data on the folder does (its first variable); a
store by web address is kept in the project as the slice read. `?zarr=<address>` in a web app link
opens the dialog on a store.

## Project items

RustGIS saves pieces of a project as files you can share and reuse:

| Item | Extension | Made with | Opened with |
| --- | --- | --- | --- |
| Map file (a map with its data) | `.rgismap` | Publish › Save As › Map File | Data › Import |
| Layer file (a layer with its symbology) | `.rgislyr` | Publish › Save As › Layer File | Add Data |
| Styles (saved symbology) | `.rgisstyle` | Style › Styles › New Style, then Publish › Save As › Style Items | Style › Styles › Add Style File |
| [Color ramps](cartography.md#custom-color-ramps) | `.rgisramp` (also reads QGIS `.xml`/`.qml`, GDAL color `.txt` and GMT `.cpt`) | Publish › Save As › Color Ramps, or by hand | Style › Styles › Add Color Ramp File |
| Project template (the setup without features) | `.rgistemplate` | Data › Templates, or Publish › Save As | Data › Templates › Open Custom Template |
| Project package (everything, rasters included) | `.rgispkg` | Publish › Package › Project | Project › Open |
| Map package / mobile map package | `.rgismpk` | Publish › Package › Map or Mobile Map | Data › Import |
| Results package (geoprocessing history and outputs) | `.rgisgpk` | Publish › Package › Results | any zip tool |
| Task (guided command steps) | `.rgistask` | Data › Task, Publish › Save As › Task Item | Data › Task › Import Task |
| Bookmarks (as longitude, latitude and scale) | `.rgisbookmarks` | Home › Bookmarks › Export Bookmarks | Home › Bookmarks › Import Bookmarks |
| Layout | `.rgislayout` | Publish › Save Layout File | Data › New Publish › Import Layout |

Packages are zip files. A mobile map package keeps only the visible layers and simplifies their
geometry, for phones and tablets. Styles can also be imported from the layers of a layer file,
map file or project. Data › Layer Templates adds empty layers with ready-made schemas (places,
routes, areas, observations…) to draw into. Add Folder connects a folder to the Browser, which
also lists the project's layouts, styles and tasks. A task's steps each run a command; New Task
From History turns the geoprocessing history into a task you can replay.
