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

All three network services are **configurable**, system-wide by the
administrator and per user (see *Settings → Service endpoints*). The
built-in defaults point at the public OpenStreetMap-community services so
the app works out of the box:

| Service | Default endpoint | Policy |
|---|---|---|
| Raster tiles | `https://tile.openstreetmap.org/{z}/{x}/{y}.png` | <https://operations.osmfoundation.org/policies/tiles/> |
| Geocoding (search, "What's here?") | `https://nominatim.openstreetmap.org` (Nominatim API) | <https://operations.osmfoundation.org/policies/nominatim/> |
| Routing | `https://router.project-osrm.org` (OSRM HTTP API v1) | demo server, see <https://github.com/Project-OSRM/osrm-backend/wiki/Demo-server> |

**These public servers are run by volunteers / donations and are not meant
for production traffic.** They have usage policies (valid identifying
User-Agent, caching, ≤1 geocoding request per second, no bulk downloading,
no client-side autocomplete against Nominatim, car profile only on the OSRM
demo). **Heavy use needs a commercial or self-hosted provider**: a Helix
distribution or site that ships this app to many users should set its tile,
geocoding and routing provider in `/etc/xdg/helix-maps/endpoints.toml`.

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
- Window size / maximized state, last viewed location, zoom level and
  rotation persist across runs in the `org.gnome.Maps` keys (first run uses
  the schema defaults: maximized, centred on 0°, 0°, zoom 0 clamped to the
  minimum zoom 2). The viewed area is also saved 2 s after the map stops
  moving, so it survives the process being killed.
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
  profiles `driving`, `cycling`, `foot`; persisted in `transportation-type`
  as `car` / `bike` / `pedestrian`). GNOME's `transit` has no OSRM profile:
  it is shown as Walk and left unchanged until the user picks a mode. Each entry holds
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
- *Current location* button / `Ctrl+L`: asks the **XDG Location portal**
  (`org.freedesktop.portal.Location` on the session bus) — like every Helix
  app, Maps never talks to GeoClue directly; the portal applies the
  desktop's permission store and works sandboxed or not. Sequence:
  subscribe to `LocationUpdated`; `CreateSession` (`session_handle_token`,
  `accuracy`=5 *exact*, thresholds 0); subscribe to `Response` on the
  predicted request path; `Start(session, "", {handle_token})`; on
  response 0 take the first `LocationUpdated` for that session
  (`Latitude`, `Longitude`, `Accuracy`, `Description`); then
  `org.freedesktop.portal.Session.Close`. Runs on a worker thread, gives up
  after 30 s and closes the session in every case (fix, denial, timeout).
- On success the blue dot is placed and the map flies there (zoom chosen
  from the accuracy). Failure shows a toast: "Location access was denied"
  (response 1), "The location portal is not available" (no portal on the
  bus), "Timed out waiting for a location fix", …
- Tested against a mock portal served on a private `dbus-daemon` that the
  unit tests start themselves (no service activation; fix, denial, absent
  portal and timeout cases).

### Settings

#### GNOME keys (`org.gnome.Maps`)
Read/written through `helix_app_common::Prefs` (in memory when the schema
is not installed). Names, types and defaults are the published ones;
`data/org.gnome.Maps.gschema.xml` ships exactly these keys:

| Key | Type | Default | Use |
|---|---|---|---|
| `last-viewed-location` | `ad` | `[0.0,0.0]` | map centre on start; invalid → default |
| `zoom-level` | `i` | `0` | start zoom, clamped to 2 – max tile zoom; saved rounded |
| `rotation` | `d` | `0.0` | map rotation in radians |
| `window-size` | `ai` | `[]` | `[width, height]`; empty/insane → 1000×700; saved only when not maximized |
| `window-maximized` | `b` | `true` | |
| `max-search-results` | `i` | `20` | geocoder `limit`, clamped 1–50 |
| `transportation-type` | enum `car`/`bike`/`pedestrian`/`transit` | `'pedestrian'` | routing mode (see Directions) |
| `show-scale` | `b` | `true` | scale overlay visibility (Preferences switch) |

Unused GNOME keys (`map-type`, `window-position`, `recent-*-limit`,
`osm-username-oauth2`) are not shipped.

#### Service endpoints (Helix-only, layered)
Six endpoint keys, resolved key by key (each layer only sets what it
contains; an invalid value is ignored and the lower layer's value stays):

1. built-in defaults (table above);
2. system-wide `helix-maps/endpoints.toml` in each `$XDG_CONFIG_DIRS`
   directory (default `/etc/xdg`); the **first** directory listed wins, as
   the Base Directory spec prescribes. A file that fails to parse is
   ignored with a warning;
3. the user's overrides in `$XDG_CONFIG_HOME/helix-maps/settings.json`.

| Key | Type | Built-in default |
|---|---|---|
| `tile_url_template` | string | `https://tile.openstreetmap.org/{z}/{x}/{y}.png` (placeholders `{z}` `{x}` `{y}` required, `{s}` = subdomain a/b/c) |
| `tile_attribution` | string | `© OpenStreetMap contributors` |
| `tile_attribution_url` | string | `https://www.openstreetmap.org/copyright` |
| `tile_max_zoom` | int 2–22 | `19` |
| `geocoder_url` | http(s) URL | `https://nominatim.openstreetmap.org` (Nominatim API) |
| `router_url` | http(s) URL | `https://router.project-osrm.org` (OSRM HTTP API v1) |

Example `/etc/xdg/helix-maps/endpoints.toml`:

```toml
tile_url_template = "https://tiles.example.org/{z}/{x}/{y}.png"
tile_attribution = "© OpenStreetMap contributors, Example Tiles"
geocoder_url = "https://nominatim.example.org"
router_url = "https://osrm.example.org"
```

`settings.json` (written atomically, `"version": 2`) stores only the
endpoint keys the user changed away from the system value, plus the
Helix-only `units` (`"metric"`/`"imperial"`, default metric). Editing a
field in Preferences to the inherited value removes the override, so later
administrator changes still reach the user. A pre-layering file (no
`version`) is migrated by dropping endpoint values equal to the built-in
defaults; its old window/view keys are ignored (they now live in the GNOME
keys).

The Preferences dialog shows the effective endpoints, the scale switch,
result count and units, "Clear tile cache", and "Reset service settings to
defaults" (drops the user's overrides; its subtitle names the system file
in use). A changed tile URL takes effect immediately (new map source,
separate cache directory).

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
- `data/org.gnome.Maps.gschema.xml`: the used `org.gnome.Maps` keys, for
  `$prefix/share/glib-2.0/schemas` — install only when the system does not
  already ship the GNOME schema.
- `/etc/xdg/helix-maps/endpoints.toml` is site configuration, not shipped
  by the app (see the example above).

## Decisions
- Map widget: libshumate 1.2 via the `libshumate` 0.6 crate (glib 0.20 /
  gtk4 0.9; single glib verified with `cargo tree -d`). Tiles are fetched by
  a Rust `ShumateDataSource` subclass so the User-Agent, cache and request
  rate are under the app's control.
- Tiles: public OSM-compatible endpoint by default, URL template
  configurable; production needs a commercial/self-hosted provider (above).
- Geocoding: Nominatim-compatible API, configurable, ≥1 s between requests.
- Routing: implemented against an OSRM-compatible endpoint, configurable.
- Current location: XDG Location portal only (zbus); no direct GeoClue
  path, consistent with the rest of Helix; tested with a mock portal on a
  private bus.
- Endpoints: tile, Nominatim-style search and OSRM-style routing stay
  configurable; production defaults are the public OSM-compatible services
  (heavy use needs a commercial or self-hosted provider); they are a
  system-wide setting (`$XDG_CONFIG_DIRS/helix-maps/endpoints.toml`) with
  per-user overrides, precedence unit-tested.
- Settings: the real `org.gnome.Maps` keys are used for everything they
  cover (exact names/types/defaults, shipped schema has only those keys);
  only Helix-specific settings (endpoint overrides, units) stay in
  `settings.json`.

### Accessibility
- Status changes that happen without focus moving are announced to
  screen readers (`helix_app_common::a11y`): search results ("5 places found", "No results found"), search failures (assertive) and a found route with its distance and time.
