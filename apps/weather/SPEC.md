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
- GSettings: `org.gnome.Weather`.
- Network access for weather data (libgweather or equivalent).
- XDG portals: Location (optional).

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
3. **Daily**: 10 rows: "Today"/"Tomorrow"/weekday + day-of-month, icon,
   description, precipitation probability, min and max temperature with a
   range bar (cairo) scaled to the 10-day min/max.
4. Footer: "Updated just now / N minutes ago / at HH:MM" (re-evaluated
   every minute).

### Locations
- Search: typing in the search entry queries geocoding (debounced 300 ms,
  ≥ 2 characters). Results show "Name" + "Region, Country". Activating a
  result adds it to the saved list (no duplicates by coordinates) and
  selects it.
- Saved list: activate a row to select it; each row has a remove button.
  Removing the selected location selects the first remaining one.
- The selected location persists alongside the list.
- Storage: `$XDG_CONFIG_HOME/helix-weather/locations.json`:
  `{ "selected": 0, "locations": [{ "id", "name", "admin1", "country",
  "country_code", "latitude", "longitude", "timezone" }] }`.

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
  `window-maximized` (b). (`locations` and `automatic-location` are
  **not** used, see Open questions.)
- `org.gnome.GWeather4`: `temperature-unit`, `speed-unit`,
  `pressure-unit` (enums above).
- `org.gnome.desktop.interface`: `clock-format` (read only).

### Keyboard shortcuts
`Ctrl+L` / `Ctrl+F` open place search, `F5` / `Ctrl+R` refresh,
`Ctrl+Q` quit, `Ctrl+W` close window.

### Command line
`helix-weather` (no options). `--help` prints usage.

## Open questions
- **Saved locations format.** GNOME Weather stores `org.gnome.Weather`
  `locations` as GWeather-serialized variants (`av`), which cannot be
  produced or fully interpreted without libgweather. Helix keeps its own
  list in `$XDG_CONFIG_HOME/helix-weather/locations.json` instead, so
  locations do not carry over between GNOME Weather and Helix Weather.
  Should we attempt a best-effort import of the GWeather variant (name +
  coordinates) on first run?
- **Automatic location.** `automatic-location` (GeoClue / XDG Location
  portal) is not implemented. Decide whether to use the Location portal
  via zbus, and how to name the resulting place (Open-Meteo offers no
  reverse geocoding).
- **Data provider.** Open-Meteo is keyless and free for non-commercial
  use; confirm it is acceptable as the default provider (or make it
  configurable / self-hostable).
