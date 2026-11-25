#!/usr/bin/env python3
"""render_cast.py <in.cast> <out.ass> [max_idle_s]

Replays an asciicast v2 file as a terminal screen (the last ROWS lines,
long lines wrapped at COLS) and writes it as ASS subtitle events, one per
screen state, for ffmpeg's ass filter over a plain background. Idle gaps
longer than max_idle_s (default: none) are shortened to it; the total length
is printed on stdout."""
import json, re, sys

src, dst = sys.argv[1], sys.argv[2]
max_idle = float(sys.argv[3]) if len(sys.argv) > 3 else None
lines = open(src).read().splitlines()
head = json.loads(lines[0]); cols, rows = head["width"], head["height"]
events = [json.loads(l) for l in lines[1:] if l.strip()]
ansi = re.compile(r"\x1b\[[0-9;?]*[A-Za-z]")
screen = [""]; states = []; t_prev = 0.0; clock = 0.0
for t, kind, text in events:
    gap = t - t_prev; t_prev = t
    clock += min(gap, max_idle) if max_idle is not None else gap
    for ch in ansi.sub("", text):
        if ch == "\n": screen.append("")
        elif ch == "\r": pass
        else: screen[-1] += ch
    wrapped = []
    for line in screen[-rows * 2:]:
        while len(line) > cols: wrapped.append(line[:cols]); line = line[cols:]
        wrapped.append(line)
    view = wrapped[-rows:]
    if states and abs(states[-1][0] - clock) < 0.05: states[-1] = (states[-1][0], view)
    else: states.append((clock, view))
end = clock + 8.0
def ts(s):
    h = int(s // 3600); m = int(s % 3600 // 60); sec = s % 60
    return f"{h}:{m:02d}:{sec:05.2f}"
def esc(s):
    # libass: \{ and \} are literal braces; a backslash before N, n or h would be a control
    s = re.sub(r"\\(?=[Nnh{}])", "\\\\​", s)
    return s.replace("{", "\\{").replace("}", "\\}") or " "
W, H = 1680, 1050
out = ["[Script Info]", "ScriptType: v4.00+", f"PlayResX: {W}", f"PlayResY: {H}", "WrapStyle: 2", "",
       "[V4+ Styles]",
       "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding",
       "Style: T,DejaVu Sans Mono,18,&H00E0E0E0,&H00E0E0E0,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,7,0,0,0,1",
       "", "[Events]", "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text"]
for i, (t, view) in enumerate(states):
    t_end = states[i + 1][0] if i + 1 < len(states) else end
    if t_end <= t: continue
    body = "\\N".join(esc(l) for l in view)
    out.append(f"Dialogue: 0,{ts(t)},{ts(t_end)},T,,0,0,0,,{{\\pos(18,14)}}{body}")
open(dst, "w").write("\n".join(out) + "\n")
print(f"{end:.1f}")
