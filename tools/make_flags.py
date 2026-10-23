#!/usr/bin/env python3
"""Genera assets/flags.png (atlante 16 colonne di bandiere 40x30) e src/country_codes.rs.

Ordine delle celle = ordine di CODES (alpha-2 alfabetico, da iso-codes).
Bandiere da flagcdn.com; una bandiera mancante resta trasparente.
Uso: python3 tools/make_flags.py  (richiede Pillow e /usr/share/iso-codes)
"""
import io, json, sys, urllib.request
from pathlib import Path
from PIL import Image

W, H, COLS = 40, 30, 16
root = Path(__file__).resolve().parent.parent
rows = json.load(open("/usr/share/iso-codes/json/iso_3166-1.json"))["3166-1"]
rows.sort(key=lambda r: r["alpha_2"])
atlas = Image.new("RGBA", (COLS * W, ((len(rows) + COLS - 1) // COLS) * H), (0, 0, 0, 0))
for i, r in enumerate(rows):
    a2 = r["alpha_2"].lower()
    try:
        data = urllib.request.urlopen(f"https://flagcdn.com/w40/{a2}.png", timeout=20).read()
        img = Image.open(io.BytesIO(data)).convert("RGBA").resize((W, H), Image.LANCZOS)
        atlas.paste(img, ((i % COLS) * W, (i // COLS) * H))
    except Exception as e:
        print(f"{a2}: {e}", file=sys.stderr)
atlas.save(root / "assets/flags.png", optimize=True)

codes = "\n".join(f'    ("{r["alpha_2"]}", "{r["alpha_3"]}"),' for r in rows)
(root / "src/country_codes.rs").write_text(
    "// Generato da tools/make_flags.py: non modificare a mano.\n"
    "// L'indice di ogni voce è la cella della sua bandiera in assets/flags.png.\n\n"
    f"pub const FLAG_W: u32 = {W};\npub const FLAG_H: u32 = {H};\npub const FLAG_COLS: u32 = {COLS};\n\n"
    "/// (alpha-2, alpha-3) ISO 3166-1, in ordine alfabetico di alpha-2.\n"
    f"pub const CODES: [(&str, &str); {len(rows)}] = [\n{codes}\n];\n"
)
