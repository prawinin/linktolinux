#!/usr/bin/env python3
"""Generate platform launcher resources from the canonical SVG."""
from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
ANDROID = ROOT / "android-companion/app/src/main/res"
SVG = ROOT / "assets/icon.svg"
NS = "http://schemas.android.com/apk/res/android"
ET.register_namespace("android", NS)

def write_vector(path: Path, paths: list[ET.Element]) -> None:
    vector = ET.Element("vector", {f"{{{NS}}}width": "108dp", f"{{{NS}}}height": "108dp",
        f"{{{NS}}}viewportWidth": "108", f"{{{NS}}}viewportHeight": "108"})
    for item in paths:
        ET.SubElement(vector, "path", {f"{{{NS}}}fillColor": item.attrib["fill"],
            f"{{{NS}}}fillType": "evenOdd" if item.get("fill-rule") == "evenodd" else "nonZero", f"{{{NS}}}pathData": item.attrib["d"]})
    path.parent.mkdir(parents=True, exist_ok=True)
    ET.indent(vector)
    ET.ElementTree(vector).write(path, encoding="utf-8", xml_declaration=True)

def main() -> None:
    source = ET.parse(SVG).getroot()
    paths = list(source.findall("{http://www.w3.org/2000/svg}path"))
    if [item.get("id") for item in paths] != ["background", "mark"]:
        raise ValueError("Icon must contain background and mark paths")
    for name, size in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 512)]:
        subprocess.run(["rsvg-convert", "-w", str(size), "-h", str(size), "-o", str(ROOT / "src-tauri/icons" / name), str(SVG)], check=True, timeout=30)
    (ROOT / "public/icon.svg").write_bytes(SVG.read_bytes())
    write_vector(ANDROID / "drawable/ic_launcher_foreground.xml", paths[1:])
    for name in ["ic_launcher", "ic_launcher_round"]:
        write_vector(ANDROID / f"mipmap-anydpi/{name}.xml", paths)
    notification = ET.Element("vector", {f"{{{NS}}}width": "24dp", f"{{{NS}}}height": "24dp",
        f"{{{NS}}}viewportWidth": "64", f"{{{NS}}}viewportHeight": "64"})
    group = ET.SubElement(notification, "group", {f"{{{NS}}}translateX": "-22", f"{{{NS}}}translateY": "-22"})
    ET.SubElement(group, "path", {f"{{{NS}}}fillColor": "#FFFFFF", f"{{{NS}}}fillType": "evenOdd", f"{{{NS}}}pathData": paths[1].attrib["d"]})
    ET.indent(notification)
    ET.ElementTree(notification).write(ANDROID / "drawable/ic_notification.xml", encoding="utf-8", xml_declaration=True)
    for version in [26]:
        for name in ["ic_launcher", "ic_launcher_round"]:
            icon = ET.Element("adaptive-icon")
            ET.SubElement(icon, "background", {f"{{{NS}}}drawable": "@color/launcher_background"})
            ET.SubElement(icon, "foreground", {f"{{{NS}}}drawable": "@drawable/ic_launcher_foreground"})
            ET.SubElement(icon, "monochrome", {f"{{{NS}}}drawable": "@drawable/ic_launcher_foreground"})
            path = ANDROID / f"mipmap-anydpi-v{version}" / f"{name}.xml"
            path.parent.mkdir(parents=True, exist_ok=True)
            ET.indent(icon)
            ET.ElementTree(icon).write(path, encoding="utf-8", xml_declaration=True)
    (ANDROID / "values/colors.xml").write_text(f'<resources>\n    <color name="launcher_background">{paths[0].attrib["fill"]}</color>\n</resources>\n')

if __name__ == "__main__":
    main()
