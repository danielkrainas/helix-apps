#!/usr/bin/env python3
"""Write the AppStream metainfo of every app.

    python3 tools/metainfo/make_metainfo.py

writes apps/<app>/data/<app-id>.metainfo.xml from the table below, the
app's .desktop file (its name) and the suite's release history in
crates/helix-app-common/data/releases.xml, which the About window shows
too. The files are committed; edit this script (or releases.xml) and run
it again to change them. Validate with `appstreamcli validate --no-net`.
"""

import glob
import os
import re
from xml.sax.saxutils import escape

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
HOMEPAGE = "https://helixdesktop.org"
REPOSITORY = "https://github.com/helixdesktop/apps"

# directory: (summary, [description paragraphs], [keywords])
APPS = {
    "audio-player": ("Listen to audio files", [
        "Audio Player plays a single audio file with a clear waveform to see and seek through.",
        "Playback can be controlled entirely from the keyboard, and the media keys and the desktop's media controls work too.",
    ], ["audio", "sound", "player"]),
    "calculator": ("Perform calculations", [
        "Calculator solves everything from simple sums to scientific, financial and programming problems.",
        "Type an expression the way you would write it, or use the keypad. Earlier results stay in the history so you can reuse them.",
    ], ["calculation", "arithmetic", "scientific", "financial"]),
    "calendar": ("Manage your schedule", [
        "Calendar shows your events by week or month, and lets you add, change and search them.",
        "It works with the calendars of your online accounts as well as local ones and web calendars you subscribe to.",
    ], ["calendar", "event", "reminder"]),
    "camera": ("Take pictures and videos", [
        "Camera takes photos and records videos with your webcam or any other camera connected to the computer.",
        "A countdown, a shutter sound and a quick look at the last capture are a click or a key press away.",
    ], ["camera", "photo", "video", "webcam"]),
    "characters": ("Find and insert special characters", [
        "Characters lets you find and copy letters, symbols and emoji that are not on your keyboard.",
        "Browse by category or search by name; characters you used recently are kept at hand.",
    ], ["character", "emoji", "symbol", "unicode"]),
    "clocks": ("Keep track of time", [
        "Clocks shows the time in cities around the world, and has alarms, a stopwatch and timers.",
        "Alarms and timers notify you even when the window is closed.",
    ], ["clock", "alarm", "stopwatch", "timer", "world"]),
    "contacts": ("Manage your contacts", [
        "Contacts keeps the addresses, phone numbers and other details of the people you know in one place.",
        "It uses the address books of your online accounts and can import and export vCard files.",
    ], ["contact", "address book", "people"]),
    "disk-usage": ("Check folder sizes and free space", [
        "Disk Usage Analyzer shows how the space on your drives is used, folder by folder.",
        "Scan your home folder, a drive or any other folder and explore it as a list or a ring chart.",
    ], ["storage", "space", "cleanup", "disk"]),
    "disks": ("Manage drives and media", [
        "Disks shows the drives and media connected to the computer, their partitions and their health.",
        "Format, partition, mount, encrypt and check volumes, attach disk images and read SMART data.",
    ], ["disk", "drive", "partition", "format", "volume"]),
    "document-scanner": ("Scan documents and photos", [
        "Document Scanner makes a digital copy of paper documents and photos with your scanner.",
        "Scan one page or a whole stack, crop and rotate the pages, and save them as a PDF or as images.",
    ], ["scanner", "scan", "document", "pdf"]),
    "document-viewer": ("View and read documents", [
        "Document Viewer shows PDF documents and lets you search, select and copy their text.",
        "Read comfortably with thumbnails, an outline, night mode and a presentation mode.",
    ], ["pdf", "document", "viewer", "reader"]),
    "files": ("Access and organize files", [
        "Files gives you access to the files and folders on your computer, on removable media and on the network.",
        "Copy, move, rename, compress and search files, with tabs, a sidebar of places and a list or grid view.",
    ], ["file", "folder", "manager", "explorer"]),
    "fonts": ("View and install fonts", [
        "Fonts lists the fonts installed on your computer and shows what each one looks like.",
        "Open a font file to preview its characters and details and to install it.",
    ], ["font", "typeface", "typography"]),
    "help": ("Get help with your desktop", [
        "Help shows the documentation for the desktop and the apps you use.",
        "Search the help pages, follow links between topics and print the pages you need.",
    ], ["help", "documentation", "manual"]),
    "image-viewer": ("View images", [
        "Image Viewer shows images, one at a time, and lets you move through the folder they are in.",
        "Zoom, rotate, view details, start a slideshow or set an image as the background.",
    ], ["image", "photo", "picture", "viewer"]),
    "logs": ("View system logs", [
        "Logs shows the detailed event logs of the system, grouped into useful categories.",
        "Search the logs, look at the details of a message and export what you need for a bug report.",
    ], ["log", "journal", "debug", "system"]),
    "maps": ("Find places around the world", [
        "Maps lets you search for places, see where you are and get directions.",
        "Keep your favourite places at hand and switch between the street map and other map styles.",
    ], ["map", "location", "route", "directions"]),
    "music": ("Play and organize your music", [
        "Music plays the songs in your Music folder, organized by album, artist and playlist.",
        "Make playlists, mark your favourite songs and control playback from the desktop's media controls.",
    ], ["music", "song", "album", "playlist", "player"]),
    "screenshot": ("Take screenshots", [
        "Screenshot saves an image of the whole screen, a window or a selected area.",
        "Take the picture after a delay, include the pointer, then copy it or save it to a file.",
    ], ["screenshot", "capture", "screen"]),
    "settings": ("Configure the desktop and system", [
        "Settings brings the preferences of the Helix desktop together in one place.",
        "Change the appearance, displays, sound, network, accessibility options and much more.",
    ], ["settings", "preferences", "configuration", "control center"]),
    "system-monitor": ("Monitor processes and resources", [
        "System Monitor shows which programs are running and how much processor, memory, network and disk they use.",
        "Search processes, change their priority, stop or end them, and watch usage over time.",
    ], ["process", "task", "cpu", "memory", "monitor"]),
    "terminal": ("Use the command line", [
        "Terminal gives you a command line in tabs and windows.",
        "It shows when a command runs with administrator rights or on a remote computer, and can search the output.",
    ], ["terminal", "command line", "shell", "console"]),
    "text-editor": ("Edit text files", [
        "Text Editor is a simple editor for notes, documents and source code.",
        "It keeps unsaved changes safe, highlights code and can find and replace text.",
    ], ["text", "editor", "notepad", "code"]),
    "tour": ("Get to know the desktop", [
        "Tour introduces the Helix desktop the first time you log in.",
        "Learn how to start apps, switch between them and find your way around.",
    ], ["tour", "welcome", "introduction"]),
    "video-player": ("Watch videos", [
        "Video Player plays video files with a simple, unobtrusive interface.",
        "Subtitles, frame-by-frame stepping and screenshots of the current frame are built in.",
    ], ["video", "movie", "player"]),
    "weather": ("Check the weather", [
        "Weather shows the current conditions and the forecast for the places you choose.",
        "See the next hours and days at a glance, with temperatures in the unit you prefer.",
    ], ["weather", "forecast", "temperature"]),
    "web": ("Browse the web", [
        "Web is a browser for the Helix desktop with a clean, simple interface.",
        "It has tabs, bookmarks, history, private browsing, a reader view and web apps that run in their own window.",
    ], ["web", "browser", "internet"]),
}


def releases():
    with open(os.path.join(ROOT, "crates", "helix-app-common", "data", "releases.xml")) as f:
        return f.read().rstrip("\n")


def desktop_file(directory):
    files = [f for f in glob.glob(os.path.join(ROOT, "apps", directory, "data", "org.helixdesktop.*.desktop"))
             if not f.endswith("-autostart.desktop")]
    assert len(files) == 1, (directory, files)
    return files[0]


def metainfo(directory):
    summary, paragraphs, keywords = APPS[directory]
    desktop = desktop_file(directory)
    app_id = os.path.basename(desktop)[: -len(".desktop")]
    with open(desktop) as f:
        entry = f.read()
    name = re.search(r"^Name=(.*)$", entry, re.M).group(1)
    categories = [c for c in re.search(r"^Categories=(.*)$", entry, re.M).group(1).split(";")
                  if c and c not in ("GTK", "GNOME")]
    # One paragraph: AppStream wants the first one to say enough on its own.
    paras = f"      <p>{escape(' '.join(paragraphs))}</p>"
    cats = "\n".join(f"    <category>{escape(c)}</category>" for c in categories)
    kws = "\n".join(f"      <keyword>{escape(k)}</keyword>" for k in keywords)
    return app_id, f"""<?xml version="1.0" encoding="UTF-8"?>
<!-- Generated by tools/metainfo/make_metainfo.py; edit that and run it again. -->
<component type="desktop-application">
  <id>{app_id}</id>
  <metadata_license>CC0-1.0</metadata_license>
  <project_license>CC0-1.0</project_license>
  <name>{escape(name)}</name>
  <summary>{escape(summary)}</summary>
  <description>
{paras}
  </description>
  <developer id="org.helixdesktop">
    <name>The Helix Desktop Project</name>
  </developer>
  <categories>
{cats}
  </categories>
  <launchable type="desktop-id">{app_id}.desktop</launchable>
  <icon type="stock">{app_id}</icon>
  <url type="homepage">{HOMEPAGE}</url>
  <url type="bugtracker">{REPOSITORY}/issues</url>
  <url type="vcs-browser">{REPOSITORY}</url>
  <keywords>
{kws}
  </keywords>
  <requires>
    <display_length compare="ge">360</display_length>
  </requires>
  <supports>
    <control>keyboard</control>
    <control>pointing</control>
    <control>touch</control>
  </supports>
  <content_rating type="oars-1.1"/>
{releases()}
</component>
"""


def main():
    for directory in APPS:
        app_id, text = metainfo(directory)
        out = os.path.join(ROOT, "apps", directory, "data", f"{app_id}.metainfo.xml")
        with open(out, "w") as f:
            f.write(text)
        print(out)
    missing = {os.path.basename(os.path.dirname(d)) for d in glob.glob(os.path.join(ROOT, "apps", "*", "src"))} - set(APPS)
    assert not missing, f"no metainfo entry for {missing}"


if __name__ == "__main__":
    main()
