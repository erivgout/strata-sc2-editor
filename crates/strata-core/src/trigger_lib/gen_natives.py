"""Generate natives.rs from the game's NativeLib trigger library and the
curated identifier list in natives.txt (next to this script).

Usage: python -I gen_natives.py <core.sc2mod dir> <out natives.rs>

<core.sc2mod dir> must contain base.sc2data/triggerlibs/nativelib.triggerlib
and enus.sc2data/localizeddata/triggerstrings.txt, extracted from the game's
CASC storage (Mods/Core.SC2Mod). Run rustfmt on the output afterwards."""
import xml.etree.ElementTree as ET, sys, re, os

HERE = os.path.dirname(os.path.abspath(__file__))
BASE = sys.argv[1]
F = os.path.join(BASE, "base.sc2data", "triggerlibs", "nativelib.triggerlib")
S = os.path.join(BASE, "enus.sc2data", "localizeddata", "triggerstrings.txt")
OUT = sys.argv[2]

root = ET.parse(F).getroot()
strings = {}
with open(S, encoding="utf-8-sig") as f:
    for line in f:
        if "=" in line:
            k, v = line.rstrip("\r\n").split("=", 1)
            strings[k] = v
el = {}
for e in root.iter("Element"):
    el[(e.get("Type"), e.get("Id"))] = e

def sentence(x):
    words = x.split(" ")
    out = []
    for n, w in enumerate(words):
        if n == 0 or (len(w) > 1 and w.isupper()) or w[:1] == "(" and w[1:].isupper():
            out.append(w)
        else:
            out.append(w.lower())
    return " ".join(out)

def s(kind, i):
    return sentence(strings.get(f"{kind}/Name/lib_Ntve_{i}", ""))

INT_LIKE = {"dialog", "control", "objective", "ping", "leaderboard", "transmission", "portrait",
            "cinematic", "planet", "texttag", "byte"}
STR_LIKE = {"gamelink", "filepath", "userinstance", "userfield", "catalogentry", "catalogfieldpath",
            "catalogfieldname", "modelanim", "convline", "convcharacter", "conversationtag",
            "attributegame", "attributeplayer", "gameoption", "gameoptionvalue", "anygamelink",
            "reference", "cooldown", "charge", "layoutframe", "fontstyle", "convstateindex",
            "aidef", "aidefwave", "modelcamera", "attributevalue", "playercolor", "difficulty"}

def flag(e, f):
    return e.find(f) is not None

def preset_values(pid):
    p = el.get(("Preset", pid))
    out = []
    if p is None:
        return out, "int", False
    base = p.find("BaseType")
    base = base.get("Value") if base is not None else "int"
    bits = flag(p, "PresetAsBits")
    pident = p.findtext("Identifier") or ""
    native = flag(p, "PresetCustom") and False
    for it in p.findall("Item"):
        if it.get("Type") != "PresetValue":
            continue
        out.append((it.get("Id"), pv_code(it.get("Id"), pident)))
    return [(s("PresetValue", vid), code) for vid, code in out], base, bits

def pv_code(vid, pident=None):
    v = el.get(("PresetValue", vid))
    if v is None:
        return None
    val = v.findtext("Value")
    if val is not None:
        return val.strip()
    if pident is None:
        # find owning preset
        for (t, i), e in el.items():
            if t == "Preset" and any(it.get("Id") == vid for it in e.findall("Item")):
                pident = e.findtext("Identifier")
                break
    return f"libNtve_ge_{pident}_{v.findtext('Identifier')}"

def galaxy_type(t, pid=None):
    if t == "preset":
        _, base, _ = preset_values(pid) if pid else ([], "int", False)
        return galaxy_type(base)
    if t in INT_LIKE:
        return "int"
    if t in STR_LIKE:
        return "string"
    return t

def fn_galaxy_name(fd):
    ident = fd.findtext("Identifier") or ""
    if flag(fd, "FlagNative") or flag(fd, "FlagNoScriptPrefix"):
        return ident
    return "libNtve_gf_" + ident

def param_code(pid, depth=0):
    """Galaxy code for a Param element (default value)."""
    p = el.get(("Param", pid))
    if p is None or depth > 4:
        return None
    pv = p.find("Preset")
    if pv is not None:
        return ("preset", pv_code(pv.get("Id")))
    fc = p.find("FunctionCall")
    if fc is not None:
        return ("raw", call_code(fc.get("Id"), depth + 1))
    val = p.findtext("Value")
    vt = p.find("ValueType")
    vt = vt.get("Type") if vt is not None else ""
    if val is not None:
        if vt == "int":
            return ("int", val)
        if vt == "fixed":
            return ("fixed", val)
        if vt == "bool":
            return ("bool", val)
        if vt == "text":
            txt = strings.get(f"Param/Value/lib_Ntve_{pid}", val)
            return ("raw", 'StringToText("' + txt.replace('\\', '\\\\').replace('"', '\\"') + '")')
        return ("str", val)
    ve = p.find("ValueElement")
    if ve is not None:
        ids = [x.get("Id") for x in p.findall("ValueId")]
        if not ids:
            if vt == "preset":
                return ("raw", "0")
        return ("raw", " | ".join(pv_code(i) for i in ids) or "0")
    if vt == "text":
        txt = strings.get(f"Param/Value/lib_Ntve_{pid}", "")
        return ("raw", 'StringToText("' + txt.replace('"', '\\"') + '")')
    if vt == "string":
        return ("str", "")
    return None

def call_code(fcid, depth):
    fc = el.get(("FunctionCall", fcid))
    fd = el.get(("FunctionDef", fc.find("FunctionDef").get("Id")))
    given = {}
    for pp in fc.findall("Parameter"):
        pe = el.get(("Param", pp.get("Id")))
        if pe is not None and pe.find("ParameterDef") is not None:
            given[pe.find("ParameterDef").get("Id")] = pp.get("Id")
    args = []
    for pr in fd.findall("Parameter"):
        pd = el.get(("ParamDef", pr.get("Id")))
        if pr.get("Id") in given:
            c = param_code(given[pr.get("Id")], depth)
        else:
            d = pd.find("Default") if pd is not None else None
            c = param_code(d.get("Id"), depth) if d is not None else None
        args.append(lit(c) if c else "null")
    return f"{fn_galaxy_name(fd)}({', '.join(args)})"

def lit(c):
    k, v = c
    if k == "str":
        return '"' + v.replace('\\', '\\\\').replace('"', '\\"') + '"'
    if k == "fixed" and "." not in v:
        return v + ".0"
    return v

def rs(x):
    return '"' + x.replace('\\', '\\\\').replace('"', '\\"') + '"'

def rs_def(c, ty):
    if c is None:
        return "Def::None"
    k, v = c
    if k == "int":
        if ty == "fixed":
            return f"Def::Fixed({float(v)!r})"
        return f"Def::Int({int(v)})"
    if k == "fixed":
        return f"Def::Fixed({float(v)!r})"
    if k == "bool":
        return f"Def::Bool({v})"
    if k == "str":
        return f"Def::Str({rs(v)})"
    if k == "preset":
        return f"Def::Preset({rs(v)})"
    return f"Def::Raw({rs(v)})"

def find_fd(ident):
    cands = [e for (t, i), e in el.items() if t == "FunctionDef" and e.findtext("Identifier") == ident]
    if not cands:
        return None
    cands.sort(key=lambda e: (flag(e, "FlagOperator"), flag(e, "ScriptCode"), flag(e, "FlagHidden"),
                              flag(e, "Deprecated"), not flag(e, "FlagNative")))
    return cands[0]

entries = []
missing = []
cat = "General"
seen = set()
for line in open(os.path.join(HERE, "natives.txt"), encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    if line.startswith("#"):
        cat = line[1:].strip()
        continue
    fd = find_fd(line)
    if fd is None:
        missing.append(line)
        continue
    if flag(fd, "ScriptCode") or flag(fd, "FlagOperator") or flag(fd, "Template"):
        missing.append(line + " (inline script code)")
        continue
    fid = fd.get("Id")
    gname = fn_galaxy_name(fd)
    if gname in seen:
        continue
    seen.add(gname)
    grammar = strings.get(f"FunctionDef/Grammar/lib_Ntve_{fid}", "")
    label = s("FunctionDef", fid)
    hint = strings.get(f"FunctionDef/Hint/lib_Ntve_{fid}", "")
    hint = re.sub(r"<[^>]+>", "", hint).strip()
    m = re.match(r"(.+?[.!?])(\s|$)", hint)
    if m:
        hint = m.group(1)
    rt = fd.find("ReturnType/Type")
    rtv = ""
    if rt is not None:
        rtv = rt.get("Value")
        te = fd.find("ReturnType/TypeElement")
        rtv = galaxy_type(rtv, te.get("Id") if te is not None else None)
    if flag(fd, "FlagEvent"):
        kind = "Event"
    elif flag(fd, "FlagAction"):
        kind = "Action"
    elif rtv == "bool":
        kind = "Condition"
    else:
        kind = "Function"
    alts = {}
    def gsub(m):
        ident = m.group(1)
        if m.group(2):
            alts[ident] = m.group(2)
        return "~" + ident + "~"
    phrase = re.sub(r"~([A-Za-z0-9_]+)(?:\|([^~]*))?~", gsub, grammar)
    phrase = re.sub(r"\s+", " ", phrase).strip()
    params = []
    for pr in fd.findall("Parameter"):
        pdef = el.get(("ParamDef", pr.get("Id")))
        if pdef is None:
            continue
        pident = pdef.findtext("Identifier") or ""
        t = pdef.find("ParameterType/Type")
        t = t.get("Value") if t is not None else "int"
        gt = pdef.find("ParameterType/GameType")
        link = gt.get("Value") if gt is not None else ""
        te = pdef.find("ParameterType/TypeElement")
        presets, bits = [], False
        if t == "preset" and te is not None:
            presets, base, bits = preset_values(te.get("Id"))
            ty = galaxy_type(base)
            link = el[("Preset", te.get("Id"))].findtext("Identifier") or ""
        else:
            ty = galaxy_type(t)
        extra = pdef.find("Preset")
        if extra is not None:
            ep, _, _ = preset_values(extra.get("Id"))
            presets = presets + ep
        if flag(pdef, "ParamFlagReference") and t == "unit" and kind == "Event":
            ty = "unitref"
        d = pdef.find("Default")
        dc = param_code(d.get("Id")) if d is not None else None
        a = alts.get(pident, "")
        if "/" not in a or ty != "bool":
            a = ""
        params.append(
            f"P {{ id: {rs(pident)}, label: {rs(s('ParamDef', pr.get('Id')) or pident)}, ty: {rs(ty)}, editor: {rs(t)}, link: {rs(link)}, default: {rs_def(dc, ty)}, presets: &[{', '.join('(' + rs(l) + ', ' + rs(c) + ')' for l, c in presets if c)}], bits: {'true' if bits else 'false'}, alts: {rs(a)} }}")
    entries.append(
        f"    F {{\n        name: {rs(gname)},\n        label: {rs(label)},\n        kind: FnKind::{kind},\n        category: {rs(cat)},\n        ret: {rs(rtv)},\n        params: &[\n" +
        "".join(f"            {p},\n" for p in params) +
        f"        ],\n        phrase: {rs(phrase)},\n        desc: {rs(hint)},\n        native: {'true' if flag(fd, 'FlagNative') else 'false'},\n    }},\n")

# every callable symbol known to the game's native library
known = set()
for (t, i), e in el.items():
    if t != "FunctionDef":
        continue
    if flag(e, "ScriptCode") or flag(e, "FlagOperator") or flag(e, "Template"):
        continue
    if not e.findtext("Identifier"):
        continue
    known.add(fn_galaxy_name(e))
extras = ["UnitFromId", "PointFromId", "RegionFromId", "CameraInfoFromId", "UnitRefFromUnit",
          "UnitRefFromVariable", "UnitRefToUnit", "libNtve_InitLib", "libNtve_InitVariables",
          "TriggerAddEventMapInit", "Wait", "StringExternal", "TriggerCreate", "TriggerEnable", "PlayerGroupNextPlayer", "UnitGroupUnitFromEnd"]
known.update(extras)
known = sorted(known)

with open(OUT, "w", encoding="utf-8", newline="\n") as o:
    o.write("//! Generated from the StarCraft II NativeLib trigger library (game build 98310)\n")
    o.write("//! by gen_natives.py from the identifiers in natives.txt; signatures, presets\n")
    o.write("//! and phrases are the game's own. Regenerate rather than hand-edit.\n\n")
    o.write("use super::{Def, FnKind, FnSpec as F, ParamSpec as P};\n\n")
    o.write(f"pub static FUNCTIONS: &[F] = &[\n{''.join(entries)}];\n\n")
    o.write("/// Every function the native library defines (sorted), plus a few hidden\n")
    o.write("/// natives the editor's own codegen uses. Used by the linter.\n")
    o.write("pub static KNOWN_FUNCTIONS: &[&str] = &[\n")
    for k in known:
        o.write(f"    {rs(k)},\n")
    o.write("];\n")
print("entries", len(entries), "known", len(known))
print("missing", missing)
