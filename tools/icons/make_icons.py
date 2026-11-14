#!/usr/bin/env python3
"""Draw the Helix app icons (scalable SVG, 128-unit canvas).

    python3 tools/icons/make_icons.py

writes apps/<app>/data/icons/org.helix.<App>.svg for every app below.
The SVGs are committed; edit this file and run it again to change them.

The Helix style: a rounded tile with a diagonal gradient in the app's
colour (light at the top left, deeper at the bottom right), a faint dark
rim, and a white symbol with a soft shadow below it. Symbols are drawn from plain shapes so they stay legible down to
24 pixels; nothing is copied from other icon sets.
"""

import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

INK = "#ffffff"
SHADOW = "#000000"

# Line width for stroked symbols (in 128-unit space).
W = 8


def stroke(width=W, extra=""):
    return f'fill="none" stroke="{{ink}}" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round"{extra}'


FILL = 'fill="{ink}"'


def tile(light, dark):
    return f"""  <defs>
    <linearGradient id="bg" x1="16" y1="8" x2="112" y2="120" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="{light}"/>
      <stop offset="1" stop-color="{dark}"/>
    </linearGradient>
  </defs>
  <rect x="8" y="8" width="112" height="112" rx="28" fill="url(#bg)"/>
  <rect x="8.5" y="8.5" width="111" height="111" rx="27.5" fill="none" stroke="#000000" stroke-opacity=".12"/>
"""


# Each symbol is an SVG fragment using {ink} for its colour; it is drawn
# twice, once dark and shifted down as the shadow, once in white.
SYMBOLS = {
    # A play button over a sound wave.
    "AudioPlayer": f"""
    <path d="M 30 64 V 64 M 42 52 V 76 M 54 40 V 88 M 66 50 V 78" {stroke()}/>
    <path d="M 78 44 L 102 64 L 78 84 Z" {FILL} stroke="{{ink}}" stroke-width="6" stroke-linejoin="round"/>""",
    # A display above a grid of keys.
    "Calculator": f"""
    <rect x="34" y="28" width="60" height="18" rx="5" {FILL}/>
    """ + "".join(
        f'<rect x="{34 + c * 22}" y="{54 + r * 16}" width="16" height="11" rx="3.5" {FILL}/>'
        for r in range(3) for c in range(3)),
    # A page with binding rings and a grid of days.
    "Calendar": f"""
    <rect x="28" y="34" width="72" height="62" rx="9" {stroke(7)}/>
    <path d="M 28 52 H 100" {stroke(7)}/>
    <path d="M 46 26 V 40 M 82 26 V 40" {stroke(7)}/>
    """ + "".join(
        f'<circle cx="{44 + c * 13.3:.1f}" cy="{66 + r * 14}" r="4" {FILL}/>'
        for r in range(2) for c in range(4)),
    # A camera body with a lens.
    "Camera": f"""
    <path d="M 26 48 Q 26 40 34 40 H 46 L 52 32 H 76 L 82 40 H 94 Q 102 40 102 48 V 86 Q 102 94 94 94 H 34 Q 26 94 26 86 Z" {stroke(7)}/>
    <circle cx="64" cy="66" r="15" {stroke(7)}/>""",
    # An omega and a dotted letter: characters beyond the keyboard.
    "Characters": f"""
    <path d="M 34 92 H 50 V 84 C 36 78 30 66 30 56 C 30 38 44 30 58 30 C 72 30 86 38 86 56 C 86 66 80 78 66 84 V 92 H 82" {stroke(7)}/>
    <circle cx="98" cy="44" r="5" {FILL}/><circle cx="98" cy="64" r="5" {FILL}/><circle cx="98" cy="84" r="5" {FILL}/>""",
    # A clock face.
    "Clocks": f"""
    <circle cx="64" cy="64" r="34" {stroke(7)}/>
    <path d="M 64 44 V 64 L 78 74" {stroke(7)}/>
    <circle cx="64" cy="64" r="4" {FILL}/>""",
    # A person on an address card.
    "Contacts": f"""
    <rect x="24" y="32" width="80" height="64" rx="10" {stroke(7)}/>
    <circle cx="52" cy="56" r="9" {FILL}/>
    <path d="M 36 82 C 38 72 44 68 52 68 C 60 68 66 72 68 82 Z" {FILL}/>
    <path d="M 78 56 H 92 M 78 70 H 88" {stroke(6)}/>""",
    # A drive with its activity light.
    "Disks": f"""
    <rect x="24" y="40" width="80" height="48" rx="12" {stroke(7)}/>
    <path d="M 24 70 H 104" {stroke(6)}/>
    <circle cx="88" cy="80" r="4" {FILL}/>
    <path d="M 38 80 H 62" {stroke(5)}/>""",
    # A ring chart in segments.
    "DiskUsage": """
    <path d="M 64 30 A 34 34 0 0 1 97.5 70" fill="none" stroke="{ink}" stroke-width="14"/>
    <path d="M 95 77 A 34 34 0 0 1 44 91.5" fill="none" stroke="{ink}" stroke-width="14" stroke-opacity=".75"/>
    <path d="M 39 87 A 34 34 0 0 1 58 30.5" fill="none" stroke="{ink}" stroke-width="14" stroke-opacity=".5"/>""",
    # A page under the scanner's light.
    "DocumentScanner": f"""
    <path d="M 40 28 H 76 L 90 42 V 100 H 40 Z" {stroke(6)}/>
    <path d="M 52 50 H 70 M 52 84 H 78" {stroke(5)}/>
    <path d="M 26 67 H 102" {stroke(8)}/>""",
    # A page with a folded corner and lines of text.
    "DocumentViewer": f"""
    <path d="M 38 26 H 74 L 92 44 V 102 H 38 Z" {stroke(7)}/>
    <path d="M 74 26 V 44 H 92" {stroke(6)}/>
    <path d="M 50 60 H 80 M 50 74 H 80 M 50 88 H 70" {stroke(6)}/>""",
    # A folder.
    "Files": f"""
    <path d="M 26 42 Q 26 34 34 34 H 54 L 62 42 H 94 Q 102 42 102 50 V 88 Q 102 96 94 96 H 34 Q 26 96 26 88 Z" {FILL}/>
    <path d="M 26 56 H 102" fill="none" stroke="{{shadow}}" stroke-opacity=".18" stroke-width="4"/>""",
    # A capital A and a small a.
    "Fonts": f"""
    <path d="M 26 96 L 50 32 L 74 96 M 34 76 H 66" {stroke(8)}/>
    <circle cx="90" cy="82" r="11" {stroke(6)}/>
    <path d="M 101 70 V 96" {stroke(6)}/>""",
    # A life buoy.
    "Help": f"""
    <circle cx="64" cy="64" r="30" {stroke(14)}/>
    <circle cx="64" cy="64" r="30" fill="none" stroke="{{bg}}" stroke-width="14" stroke-dasharray="14 33.1" stroke-dashoffset="7" stroke-opacity=".55"/>""",
    # A picture: hills under a sun.
    "ImageViewer": f"""
    <rect x="24" y="32" width="80" height="64" rx="10" {stroke(7)}/>
    <circle cx="82" cy="52" r="7" {FILL}/>
    <path d="M 30 92 L 56 62 L 72 80 L 82 70 L 100 92 Z" {FILL}/>""",
    # Lines of a log, each with its marker.
    "Logs": f"""
    <circle cx="34" cy="40" r="5" {FILL}/><path d="M 48 40 H 96" {stroke(6)}/>
    <circle cx="34" cy="58" r="5" {FILL}/><path d="M 48 58 H 84" {stroke(6)}/>
    <circle cx="34" cy="76" r="5" {FILL}/><path d="M 48 76 H 92" {stroke(6)}/>
    <circle cx="34" cy="94" r="5" {FILL}/><path d="M 48 94 H 74" {stroke(6)}/>""",
    # A folded map with a pin.
    "Maps": f"""
    <path d="M 24 40 L 48 32 L 80 42 L 104 34 V 88 L 80 96 L 48 86 L 24 94 Z" {stroke(6)}/>
    <path d="M 48 32 V 86 M 80 42 V 96" {stroke(5)}/>
    <path d="M 64 78 C 54 66 50 60 50 53 A 14 14 0 0 1 78 53 C 78 60 74 66 64 78 Z" {FILL}/>
    <circle cx="64" cy="53" r="5" fill="{{bg}}"/>""",
    # Two notes joined by a beam.
    "Music": f"""
    <path d="M 52 84 V 38 L 94 30 V 76" {stroke(7)}/>
    <path d="M 52 38 L 94 30 V 42 L 52 50 Z" {FILL}/>
    <ellipse cx="42" cy="86" rx="12" ry="9" {FILL}/>
    <ellipse cx="84" cy="78" rx="12" ry="9" {FILL}/>""",
    # A selection's corners around a dashed area.
    "Screenshot": f"""
    <path d="M 26 46 V 32 Q 26 26 32 26 H 46 M 82 26 H 96 Q 102 26 102 32 V 46 M 102 82 V 96 Q 102 102 96 102 H 82 M 46 102 H 32 Q 26 102 26 96 V 82" {stroke(8)}/>
    <rect x="44" y="44" width="40" height="40" rx="4" fill="none" stroke="{{ink}}" stroke-width="5" stroke-dasharray="8 6"/>""",
    # A gear: eight rounded teeth around a ring.
    "Settings": "".join(
        f'<rect x="57" y="24" width="14" height="18" rx="3.5" fill="{{ink}}" transform="rotate({k * 45} 64 64)"/>'
        for k in range(8)) + """
    <circle cx="64" cy="64" r="27" fill="{ink}"/>
    <circle cx="64" cy="64" r="11" fill="{bg}"/>""",
    # A pulse on a screen.
    "SystemMonitor": f"""
    <rect x="24" y="32" width="80" height="58" rx="9" {stroke(7)}/>
    <path d="M 32 66 H 46 L 54 50 L 64 78 L 74 58 L 80 66 H 96" {stroke(6)}/>
    <path d="M 50 100 H 78" {stroke(7)}/>""",
    # A prompt.
    "Terminal": f"""
    <path d="M 32 44 L 54 64 L 32 84" {stroke(10)}/>
    <path d="M 62 88 H 96" {stroke(10)}/>""",
    # A page with a pen.
    "TextEditor": f"""
    <path d="M 34 30 H 74 V 98 H 34 Z" {stroke(6)}/>
    <path d="M 44 46 H 64 M 44 60 H 64 M 44 74 H 56" {stroke(5)}/>
    <path d="M 96 40 L 104 48 L 74 94 L 62 100 L 64 86 Z" {FILL}/>""",
    # A compass.
    "Tour": f"""
    <circle cx="64" cy="64" r="34" {stroke(7)}/>
    <path d="M 64 36 L 74 64 L 64 92 L 54 64 Z" {FILL}/>
    <circle cx="64" cy="64" r="4" fill="{{bg}}"/>""",
    # A film frame with a play button.
    "VideoPlayer": f"""
    <rect x="24" y="34" width="80" height="60" rx="10" {stroke(7)}/>
    <path d="M 24 46 H 104 M 24 82 H 104" {stroke(5)}/>
    <path d="M 56 52 L 76 64 L 56 76 Z" {FILL} stroke="{{ink}}" stroke-width="4" stroke-linejoin="round"/>""",
    # The sun behind a cloud.
    "Weather": f"""
    <circle cx="52" cy="50" r="16" {FILL}/>
    <path d="M 52 22 V 26 M 28 50 H 24 M 32 30 L 35 33 M 72 30 L 69 33" {stroke(5)}/>
    <path d="M 44 96 H 92 A 14 14 0 0 0 92 68 A 20 20 0 0 0 54 64 A 16 16 0 0 0 44 96 Z" fill="{{bg}}" stroke="{{bg}}" stroke-width="10"/>
    <path d="M 44 96 H 92 A 14 14 0 0 0 92 68 A 20 20 0 0 0 54 64 A 16 16 0 0 0 44 96 Z" {FILL}/>""",
    # A globe.
    "Web": f"""
    <circle cx="64" cy="64" r="34" {stroke(7)}/>
    <ellipse cx="64" cy="64" rx="14" ry="34" {stroke(6)}/>
    <path d="M 30 64 H 98 M 36 46 H 92 M 36 82 H 92" {stroke(5)}/>""",
}


# App: (directory, colour light, colour dark).
APPS = {
    "AudioPlayer": ("audio-player", "#ffa94d", "#e8590c"),
    "Calculator": ("calculator", "#8d99a6", "#3d4752"),
    "Calendar": ("calendar", "#ff8787", "#c92a2a"),
    "Camera": ("camera", "#6e7681", "#24292f"),
    "Characters": ("characters", "#b197fc", "#6741d9"),
    "Clocks": ("clocks", "#5c7cfa", "#26318c"),
    "Contacts": ("contacts", "#38d9a9", "#087f5b"),
    "Disks": ("disks", "#adb5bd", "#495057"),
    "DiskUsage": ("disk-usage", "#69db7c", "#2b8a3e"),
    "DocumentScanner": ("document-scanner", "#66d9e8", "#0c8599"),
    "DocumentViewer": ("document-viewer", "#ff8a8a", "#b02525"),
    "Files": ("files", "#74c0fc", "#1864ab"),
    "Fonts": ("fonts", "#e2b15c", "#8f5a0a"),
    "Help": ("help", "#ffd43b", "#e67700"),
    "ImageViewer": ("image-viewer", "#4dabf7", "#1c5aa8"),
    "Logs": ("logs", "#5c6370", "#1e2228"),
    "Maps": ("maps", "#8ce99a", "#2f9e44"),
    "Music": ("music", "#f783ac", "#c2255c"),
    "Screenshot": ("screenshot", "#7b8cff", "#3b2ea8"),
    "Settings": ("settings", "#9aa5b1", "#4a5561"),
    "SystemMonitor": ("system-monitor", "#40c057", "#1b5e2a"),
    "Terminal": ("terminal", "#454b54", "#121417"),
    "TextEditor": ("text-editor", "#ffe066", "#d9a400"),
    "Tour": ("tour", "#3bc9db", "#1162a3"),
    "VideoPlayer": ("video-player", "#e599f7", "#9c36b5"),
    "Weather": ("weather", "#74c0fc", "#2f6fd1"),
    "Web": ("web", "#4dabf7", "#0b5394"),
}


def icon(app):
    _, light, dark = APPS[app]
    symbol = SYMBOLS[app]
    # Ink that matches the tile, for cut-outs in a symbol.
    mid = dark
    shadow = symbol.replace("{ink}", SHADOW).replace("{shadow}", SHADOW).replace("{bg}", SHADOW)
    front = symbol.replace("{ink}", INK).replace("{shadow}", SHADOW).replace("{bg}", mid)
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128">
  <!-- Helix app icon: org.helix.{app}. Generated by tools/icons/make_icons.py. -->
{tile(light, dark)}  <g transform="translate(0 2.5)" opacity=".2">{shadow}
  </g>
  <g>{front}
  </g>
</svg>
"""


def main():
    only = set(sys.argv[1:])
    for app, (directory, _, _) in APPS.items():
        if only and app not in only:
            continue
        out = os.path.join(ROOT, "apps", directory, "data", "icons", f"org.helix.{app}.svg")
        os.makedirs(os.path.dirname(out), exist_ok=True)
        with open(out, "w") as f:
            f.write(icon(app))
        print(out)


if __name__ == "__main__":
    main()
