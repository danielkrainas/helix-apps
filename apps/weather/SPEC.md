# Weather

Drop-in replacement for GNOME Weather.

- **App ID**: `org.helix.Weather`
- **Binary**: `helix-weather`
- **Crate**: `apps/weather`

## Purpose
Current conditions and forecast.

## Main windows
- Locations list with current conditions and forecast.
- Location search.

## Contracts used
- GSettings: `org.gnome.Weather`, `org.gnome.GWeather4`,
  `org.gnome.shell.weather` (read only, import).
- Network access for weather data (Open-Meteo) and reverse geocoding
  (OpenStreetMap Nominatim, fallback only).
- XDG portals: Location; GeoClue2 (fallback).
- GWeather location database `libgweather-4/Locations.xml` (read-only data
  file, optional).

## Non-goals
- Exact visual style of current GNOME Weather.

---

## Behaviour (derived spec)

### Data source
- libgweather is not available, so Helix Weather talks directly to the
  keyless public **Open-Meteo** services over HTTPS (pure-Rust client:
  `ureq` + `rustls`, bundled Mozilla roots):
  - Forecast: `https://api.open-meteo.com/v1/forecast` with
    `latitude`, `longitude`, `timezone=auto`, `forecast_days=10`, and
    - `current=temperature_2m,relative_humidity_2m,apparent_temperature,is_day,weather_code,wind_speed_10m,wind_direction_10m,pressure_msl`
    - `hourly=temperature_2m,weather_code,precipitation_probability,is_day`
    - `daily=weather_code,temperature_2m_max,temperature_2m_min,sunrise,sunset,precipitation_probability_max`
  - Geocoding: `https://geocoding-api.open-meteo.com/v1/search` with
    `name`, `count=10`, `language=<UI language>`, `format=json`.
- Data is always requested in metric units (°C, km/h, hPa) and converted
  locally. Times are the location's local time (`timezone=auto`); the
  response's `utc_offset_seconds` tells the app what "now" is there.
- The only client header is a generic `User-Agent: helix-weather/0.1`.
  Nothing is sent besides coordinates / search text.
- **Terms of use.** Open-Meteo's free API is offered for **non-commercial
  use only** (fewer than 10 000 calls per day, attribution required; data
  licensed CC BY 4.0). Commercial use requires an Open-Meteo API
  subscription (`customer-api.open-meteo.com` with an `apikey`) or a
  self-hosted Open-Meteo instance. Helix Weather stays within the free
  terms (one forecast request per location per 30 minutes, plus searches)
  and credits "Weather data by Open-Meteo.com" (linked) under the forecast
  and in the About dialog, which also credits OpenStreetMap contributors for
  Nominatim place names (ODbL; Nominatim's usage policy allows such light
  use: one request per lookup, identifying User-Agent). Distributors shipping Helix commercially must review
  these terms; the endpoint URLs are constants in `src/api.rs`.
- Requests run on a worker thread (`gio::spawn_blocking`); the UI never
  blocks. Timeout 15 s. API errors (`{"error":true,"reason":…}`) are shown
  to the user.

### Weather codes
WMO codes map to a description and a standard symbolic icon
(`weather-*-symbolic`), with night variants when `is_day == 0`:

| WMO | Description | Icon |
|---|---|---|
| 0 | Clear sky | `weather-clear` / `weather-clear-night` |
| 1, 2 | Mainly clear / Partly cloudy | `weather-few-clouds` / `weather-few-clouds-night` |
| 3 | Overcast | `weather-overcast` |
| 45, 48 | Fog / Depositing rime fog | `weather-fog` |
| 51–57 | Drizzle / freezing drizzle | `weather-showers-scattered` |
| 61 | Slight rain | `weather-showers-scattered` |
| 63, 65, 66, 67 | Rain / freezing rain | `weather-showers` |
| 71–77 | Snow / snow grains | `weather-snow` |
| 80 | Slight rain showers | `weather-showers-scattered` |
| 81, 82 | Rain showers | `weather-showers` |
| 85, 86 | Snow showers | `weather-snow` |
| 95, 96, 99 | Thunderstorm (with hail) | `weather-storm` |
| other | Unknown | `weather-severe-alert` |

### Window
- One `AdwApplicationWindow` (default 900×700; size and maximized state
  persist via `org.gnome.Weather` window keys).
- **Header bar**: title is the selected location's name (subtitle:
  region, country). Start: a *Places* menu button (Ctrl+L / Ctrl+F)
  opening a popover with a search entry, search results and the saved
  locations. End: primary menu and a *Refresh* button (F5 / Ctrl+R).
- Primary menu: *Temperature unit* radio (°C / °F), *Refresh*,
  *About Weather*, *Quit*.
- **Content** is a stack:
  - *Empty*: `AdwStatusPage` "Add a Location" with a button opening the
    search when no location is saved.
  - *Loading*: a spinner while the first forecast for a location loads
    and nothing is cached.
  - *Error*: `AdwStatusPage` with the error text and a *Retry* button when
    the fetch failed and no cached forecast exists.
  - *Weather*: scrollable page (below).
- An `AdwBanner` shows "Offline — showing forecast from <time>" when the
  latest refresh failed but a cached forecast is shown; its button retries.

### Weather page
1. **Current conditions**: large condition icon, temperature, condition
   description, "Feels like N°", and a details grid: wind (speed + compass
   direction, e.g. "12 km/h NW"), humidity (%), pressure, sunrise and
   sunset (today, location-local time).
2. **Hourly**: horizontally scrollable strip of the next 48 hours starting
   at the current location-local hour: time ("Now" for the first),
   condition icon, a temperature graph drawn with cairo (filled area, line,
   dots and value labels) and precipitation probability when ≥ 10 %.
   Hour and precipitation texts use the `caption` style.
3. **Daily**: 10 rows: "Today"/"Tomorrow"/weekday + day-of-month, icon,
   description, precipitation probability, min and max temperature with a
   range bar (cairo) scaled to the 10-day min/max.
- Graphs take their colours from the style each time they are drawn: the
  curve, dots and bars in the accent colour, labels and bar tracks in the
  text colour. They redraw at once when the style switches between light,
  dark and high contrast, or the interface font or text scaling changes.
  The value labels are laid out with Pango in the interface font (bold),
  so they follow the system font and Large Text.
4. Footer: "Updated just now / N minutes ago / at HH:MM" (re-evaluated
   every minute).
5. Credit line: "Weather data by Open-Meteo.com" (link).

### Locations
- Popover top: an **Automatic Location** switch row ("Show the weather where
  you are"). The empty page also offers *Use Current Location*, which turns
  it on.
- Search: typing in the search entry queries geocoding (debounced 300 ms,
  ≥ 2 characters). Results show "Name" + "Region, Country". Activating a
  result adds it to the saved list (no duplicates by coordinates) and
  selects it.
- Saved list: activate a row to select it; each row has a remove button.
  Removing the selected location selects the first remaining one.
- The selected location persists alongside the list.
- Storage: `$XDG_CONFIG_HOME/helix-weather/locations.json`:
  `{ "selected": 0, "automatic_location": false, "locations": [{ "id",
  "name", "admin1", "country", "country_code", "latitude", "longitude",
  "timezone", "automatic"? }] }` (`automatic: true` only on the located
  place).

### Automatic location
- When on, Helix asks for the position on startup, whenever it is switched
  on, and at every 30-minute auto-refresh: the XDG **Location portal**
  (`CreateSession` with city accuracy, `Start`, first `LocationUpdated`,
  `Session.Close`), falling back to **GeoClue2** on the system bus
  (`DesktopId` = `org.helix.Weather`, city accuracy). Worker thread, 30 s
  timeout.
- **Reverse geocoding**: (1) offline, the nearest `<city>` within 50 km in
  the GWeather database gives the name, region (state), country and time
  zone; (2) otherwise OpenStreetMap **Nominatim**
  (`https://nominatim.openstreetmap.org/reverse?format=jsonv2&zoom=10`,
  `lat`/`lon` rounded to 4 decimals, `accept-language` = UI language; name
  = city/town/village/municipality/hamlet/suburb/county); (3) otherwise
  "Current Location". The forecast uses the real coordinates, not the
  city's.
- The located place is kept as the first entry of the saved list (subtitle
  "Current location • Region, Country", a location icon). A new fix
  replaces it; the selection follows it if it was selected (or nothing else
  is saved). Removing that row or switching the row off turns automatic
  location off and drops the entry.
- If the lookup fails and nothing else is saved, the error page shows
  "Could Not Find Your Location" with the reason; *Retry* tries again
  and *Search Locations* opens the places popover to pick a place by hand
  (automatic location stays on; the chosen place is shown meanwhile).
- The switch mirrors `org.gnome.Weather automatic-location` when that schema
  is installed (it then wins at startup); otherwise it persists in
  `locations.json`.

### First-run import (read only)
- When `locations.json` does not exist yet, Helix imports GNOME Weather's
  saved locations once, from the first non-empty source: (1) the
  `org.gnome.Weather locations` key if the schema is installed (it is not on
  this machine; only `org.gnome.GWeather4` is), (2) `dconf read
  /org/gnome/Weather/locations` (works without the schema), (3) the Flatpak
  app's keyfile `~/.var/app/org.gnome.Weather/config/glib-2.0/settings/keyfile`
  (`[org/gnome/Weather] locations`), (4) GNOME Shell's copy
  `org.gnome.shell.weather locations` (installed, `av`). On this machine all
  sources were empty when checked.
- Format: `av`, each item a GWeather-serialized location `(uv)`; version 2
  is `(name, station code, is-city, [(lat, lon)] city, [(lat, lon)]
  station)` in radians, version 1 the same with maybes `m(dd)`. The city
  coordinates (else the station's) become the place; the station code is
  looked up in the GWeather database for region, country and time zone; a
  nameless entry takes the station's name. Unparsable entries are skipped,
  duplicates dropped. Nothing is ever written back to GNOME's settings.
- Any failure (no source, bad data) simply starts with an empty list.

### Refresh, cache, offline
- On selecting a location: show the cached forecast immediately (if any),
  then fetch. Auto-refresh every 30 minutes; manual refresh any time.
- The raw forecast response is cached per location in
  `$XDG_CACHE_HOME/helix-weather/forecast-<lat>_<lon>.json` together with
  its fetch time (Unix seconds) and used for offline display.
- For a stale cached forecast the hourly strip still starts at the current
  location-local hour (computed from the cached UTC offset) when that hour
  is still covered by the data.

### Units
- Temperature follows `org.gnome.GWeather4` `temperature-unit`
  (`default|kelvin|centigrade|fahrenheit`). `default` resolves by locale
  (`LC_ALL` → `LC_MEASUREMENT` → `LANG`): Fahrenheit for `en_US` (also
  `en_LR`, `my_MM`), else Celsius. `kelvin` is displayed as Celsius.
  The in-app °C/°F toggle writes `centigrade` / `fahrenheit` back.
- Wind follows `speed-unit` (`default|ms|kph|mph|knots|bft`); default:
  mph for imperial locales, else km/h.
- Pressure follows `pressure-unit`
  (`default|kpa|hpa|mb|mm-hg|inch-hg|atm`); default: inHg for imperial
  locales, else hPa.
- If the schema is missing, defaults live in memory for the session.

### Time format
- Hours follow `org.gnome.desktop.interface` `clock-format` (`24h`/`12h`)
  when readable; default 24h.

### GSettings
- `org.gnome.Weather`: `window-width` (i), `window-height` (i),
  `window-maximized` (b), `automatic-location` (b, default `false`),
  `locations` (`av`, read only for the import).
- `org.gnome.shell.weather`: `locations` (`av`, read only for the import).
- `org.gnome.GWeather4`: `temperature-unit`, `speed-unit`,
  `pressure-unit` (enums above).
- `org.gnome.desktop.interface`: `clock-format` (read only).

### Keyboard shortcuts
`Ctrl+L` / `Ctrl+F` open place search, `F5` / `Ctrl+R` refresh,
`Ctrl+Q` quit, `Ctrl+W` close window.

### Command line
`helix-weather` (no options). `--help` prints usage.

## Decisions
- **Data provider**: Open-Meteo stays the default; its non-commercial terms
  are documented above (Data source → Terms of use).
- **Automatic location**: Location portal, falling back to GeoClue2, with
  reverse geocoding (offline GWeather database, then Nominatim).
- **Saved locations**: Helix keeps its own `locations.json`; on first run it
  imports GNOME Weather's GWeather-serialized locations best-effort and read
  only; on failure it starts empty.

### Accessibility
- Status changes that happen without focus moving are announced to
  screen readers (`helix_app_common::a11y`): the forecast or location error page appearing (assertive) and the offline banner appearing.
- Focus is kept across list rebuilds (`helix_app_common::focus`): Refreshes, including the automatic one, keep keyboard focus on the same day row and the scroll position.
- In the places popover the shown place's row is exposed as selected (its
  check mark and the current-location icon are presentational; the
  subtitle already says "Current location"); each Remove button names its
  place ("Remove Berlin"). The loading spinners are named "Loading
  Forecast" and "Updating Forecast".
