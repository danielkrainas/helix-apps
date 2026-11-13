# Maps

Drop-in replacement for GNOME Maps.

- **App ID**: `org.helix.Maps`
- **Binary**: `helix-maps`
- **Crate**: `apps/maps`

## Purpose
Map viewer and place search.

## Main windows
- Map view.
- Search and place details.
- Routing (optional).

## Contracts used
- XDG portals: Location, OpenURI.
- GSettings: `org.gnome.Maps`.
- OpenStreetMap or equivalent tile/service backend.

## Non-goals
- Full offline map support on day one.

---

## Behaviour (derived spec)

### Backends and usage policies (read this first)

All three network services are **configurable** (Preferences dialog or
`settings.json`, see below). The defaults point at the public
OpenStreetMap-community services so the app works out of the box:

| Service | Default endpoint | Policy |
|---|---|---|
| Raster tiles | `https://tile.openstreetmap.org/{z}/{x}/{y}.png` | <https://operations.osmfoundation.org/policies/tiles/> |
| Geocoding (search, "What's here?") | `https://nominatim.openstreetmap.org` (Nominatim API) | <https://operations.osmfoundation.org/policies/nominatim/> |
| Routing | `https://router.project-osrm.org` (OSRM HTTP API v1) | demo server, see <https://github.com/Project-OSRM/osrm-backend/wiki/Demo-server> |

**These public servers are run by volunteers / donations and are not meant
for production traffic.** They have usage policies (valid identifying
User-Agent, caching, ≤1 geocoding request per second, no bulk downloading,
no client-side autocomplete against Nominatim, car profile only on the OSRM
demo). A Helix distribution that ships this app to real users **must**
configure a commercial or self-hosted tile, geocoding and routing provider.

To comply with the policies the app:
- sends `User-Agent: helix-maps/<version>` on every request (no personal
  data, no per-user identifiers);
- caches tiles on disk (`$XDG_CACHE_HOME/helix-maps/tiles/<hash>/z/x/y`,
  fresh for 7 days, stale tiles reused when offline) and downloads at most
  2 tiles concurrently, newest request first, skipping cancelled requests;
- sends geocoding/routing requests from a single worker that enforces at
  least **1 second between requests**; superseded queued requests are
  dropped; search runs only when the user presses Enter (no
  search-as-you-type);
- shows the tile attribution (configurable) in the map's license overlay.

### Window
- One `AdwApplicationWindow`. Header bar: *Current location* button (left),
  a search entry as title, *Directions* toggle, *Favorites* menu button and
  the primary menu (*Preferences*, *Keyboard Shortcuts*, *About Maps*).
- Content: an `AdwOverlaySplitView` whose content is the map
  (`ShumateSimpleMap`: tiles, zoom buttons, compass, scale, license) and
  whose sidebar (hidden by default, on the start side) shows either the
  **place details** page or the **directions** page.
- Window size / maximized state, last viewed location and zoom level
  persist across runs (first run: a world view, zoom 3 centred on 30°N
  10°E). The viewed area is also saved 2 s after the map stops moving, so
  it survives the process being killed.
- A toast overlay reports errors (network failures, location unavailable).

### Map
- Drag to pan, scroll / pinch / zoom buttons / `Ctrl+=` `Ctrl+-` to zoom,
  double click zooms in, two-finger rotate is allowed (compass resets it).
- Zoom range 2 – `tile_max_zoom` (default 19).
- Right click (or long press) on the map opens a context menu at that
  point: **What's here?** (reverse geocode → place details), **Directions
  from here**, **Directions to here**, **Copy location** (copies
  `lat, lon` with 6 decimals).
- Selected place is shown with a pin marker; the current location with a
  blue dot marker; a route as a blue polyline with start/end markers.

### Search
- Typing in the search entry and pressing Enter queries the geocoder
  (`/search?format=jsonv2&addressdetails=1&extratags=1&limit=N&q=…`,
  `accept-language` from the locale). Results are biased to the current
  view (`viewbox` of the visible area, not bounded).
- If the text is a coordinate pair (`52.52, 13.40`, also `52.52 13.40`) or
  a `geo:` URI, no request is made: the map jumps there and the place page
  shows the coordinates.
- Results appear in a popover under the entry (name, short address,
  category). Activating one selects it: map flies to it (fitting its
  bounding box), the pin is placed and the place page opens.
- `Escape` in the entry clears it / closes the popover.

### Place details page
- Title (name or first address part), full address, humanised category
  (e.g. "Restaurant", "Railway station"), coordinates, and when available
  from extratags: opening hours, phone, website (link), Wikipedia.
- Buttons: **Directions** (opens directions with this place as
  destination), **Favorite** star toggle, **Copy location**, **Share**
  (copies a `geo:lat,lon` URI), **Open in OpenStreetMap** (opens
  `https://www.openstreetmap.org/<type>/<id>` or a `?mlat=&mlon=` URL via
  `GtkUriLauncher`, i.e. the OpenURI portal when sandboxed).

### Favorites
- Starred places are stored in `$XDG_DATA_HOME/helix-maps/favorites.json`
  (list of `{name, address, lat, lon, osm_type, osm_id, category}`).
- The Favorites header menu lists them; activating one selects it like a
  search result. Removing the star removes it.

### Directions
- From / To entries plus a transport selector (Car, Bike, Walk → OSRM
  profiles `driving`, `cycling`, `foot`; persisted). Each entry holds
  either a resolved place (filled from a place page, the context menu or
  *My location*) or free text, which is geocoded (first result) when
  routing. A swap button exchanges them; a *My location* button fills From
  with the current location.
- **Find route** requests
  `{router}/route/v1/{profile}/{lon,lat};{lon,lat}?overview=full&geometries=geojson&steps=true`.
  The route is drawn, the map fits it, and the page shows total distance /
  duration and the list of turn-by-turn steps ("Turn left onto Main
  Street", "Arrive at destination"); activating a step centres the map on
  it. Distances use the configured unit system (metric default).
- The OSRM demo server only supports the car profile reliably; a note under
  the selector says so when the default router is configured.

### Current location
- *Current location* button / `Ctrl+L`: asks **GeoClue** on the system bus
  (`org.freedesktop.GeoClue2.Manager.GetClient`, `DesktopId=org.helix.Maps`,
  `RequestedAccuracyLevel=8` (exact), `Start`, wait ≤ 30 s for the
  `Location` property, read `Latitude/Longitude/Accuracy/Description`,
  `Stop`). Runs on a worker thread. On success the blue dot is placed and
  the map flies there (zoom chosen from the accuracy); failure shows a
  toast ("Location unavailable").
- The XDG Location portal is not used yet (GeoClue applies its own
  agent-based authorisation; inside a sandbox the portal would be needed —
  see open questions).

### Settings
`org.gnome.Maps` is **not installed** on this system and is not available
to align with, so Helix keeps only private settings in
`$XDG_CONFIG_HOME/helix-maps/settings.json` (written atomically; unknown
or invalid fields fall back to defaults). Keys:

| Key | Type | Default | Meaning |
|---|---|---|---|
| `tile_url_template` | string | `https://tile.openstreetmap.org/{z}/{x}/{y}.png` | Raster tile URL; placeholders `{z}` `{x}` `{y}` (required) and `{s}` (subdomain a/b/c) |
| `tile_attribution` | string | `© OpenStreetMap contributors` | License overlay text |
| `tile_attribution_url` | string | `https://www.openstreetmap.org/copyright` | |
| `tile_max_zoom` | int | 19 | |
| `geocoder_url` | string | `https://nominatim.openstreetmap.org` | Nominatim-compatible base URL |
| `router_url` | string | `https://router.project-osrm.org` | OSRM-compatible base URL |
| `max_search_results` | int | 10 | 1–50 |
| `units` | `"metric"`/`"imperial"` | `metric` | |
| `transportation_type` | `"car"`/`"bike"`/`"walk"` | `car` | |
| `last_viewed_location` | [lat, lon] | [30, 10] | |
| `zoom_level` | float | 3 | |
| `window_width` / `window_height` | int | 1000 / 700 | |
| `window_maximized` | bool | false | |

The Preferences dialog edits the endpoint/attribution/units/result-count
keys, offers "Reset to defaults" and "Clear tile cache". A changed tile URL
takes effect immediately (new map source, separate cache directory).

### Keyboard shortcuts
| Keys | Action |
|---|---|
| `Ctrl+F` | Focus search |
| `Ctrl+L` | Go to current location |
| `Ctrl+D` | Toggle directions |
| `Ctrl+=` / `Ctrl++` / `Ctrl+-` | Zoom in / out |
| `Escape` | Close sidebar |
| `Ctrl+,` | Preferences |
| `Ctrl+?` | Keyboard shortcuts |
| `Ctrl+W` / `Ctrl+Q` | Close window / quit |

### Command line
`helix-maps [URI | --search QUERY]`
- `geo:` URIs (RFC 5870: `geo:lat,lon[,alt][;params][?z=ZOOM]`, plus the
  common `geo:0,0?q=lat,lon(Label)` and `geo:0,0?q=query` forms) open the
  map at that point (with a pin) or run the search.
- `maps:q=QUERY` and `--search QUERY` run a search on startup.
- `--help`, `--version`.

### Packaging
- `data/org.helix.Maps.desktop` (handles `x-scheme-handler/geo` and
  `x-scheme-handler/maps`). Installs to `$prefix/share/applications`.
- No GSettings schema is shipped (see Settings).

## Decisions
- Map widget: libshumate 1.2 via the `libshumate` 0.6 crate (glib 0.20 /
  gtk4 0.9; single glib verified with `cargo tree -d`). Tiles are fetched by
  a Rust `ShumateDataSource` subclass so the User-Agent, cache and request
  rate are under the app's control.
- Tiles: public OSM-compatible endpoint by default, URL template
  configurable; production needs a commercial/self-hosted provider (above).
- Geocoding: Nominatim-compatible API, configurable, ≥1 s between requests.
- Routing: implemented against an OSRM-compatible endpoint, configurable.
- Current location: GeoClue over D-Bus (zbus).

## Open questions
- Should the XDG Location portal be preferred over direct GeoClue (needed
  once the app is sandboxed)? Currently GeoClue only.
- Which production tile / geocoding / routing providers should Helix
  distributions configure by default (and should the endpoints become a
  system-wide setting rather than per-user)?
- `org.gnome.Maps` key migration: the schema is not installed here; if it
  should be read for carry-over (e.g. `last-viewed-location`, `zoom-level`,
  `transportation-type`), the schema XML needs to be provided.
