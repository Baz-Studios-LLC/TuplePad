#!/usr/bin/env python3
"""Writes core/tuplepad-app/ui/theme.slint from the theme table below. Edit the colors here and
rerun: `scripts/make-themes.py`."""

import os

FIELDS = ["accent", "accent-text", "editor-background", "text", "line-number", "current-line-number",
          "current-line", "selection", "occurrence", "bracket", "bracket-border", "caret", "preedit-underline",
          "indent-guide", "sidebar-background", "sidebar-hover", "separator", "header-background",
          "status-background", "primary-text", "secondary-text", "tertiary-text", "scrollbar-thumb",
          "control-hover", "control-pressed", "icon", "control-border", "scrim", "drop-tint", "selected-row",
          "selected-row-text", "selected-row-detail", "selected-badge", "find-match", "mark"]

# (id, name, summary, filled selection, light colors, light syntax, dark colors, dark syntax). Syntax:
# plain, keyword, string, escape, comment, number, constant, function, type, variable, property,
# operator, punctuation, tag, attribute, label, namespace, heading, emphasis, strong, link, code,
# inserted, deleted, invisible.
THEMES = [
    ("tuplepad", "TuplePad", "Neutral, with midnight blue", False,
     "#103474 #FFFFFF #FFFFFF #1D1D1F #B4B4BA #6E6E73 #0000000A #10347429 #10347414 #10347420 #10347459 #103474 #103474 #00000012 #F4F4F6 #0000000A #E2E2E6 #FFFFFF #FAFAFB #1D1D1F #86868B #AEAEB2 #0000004D #0000000D #00000017 #6E6E73 #00000024 #00000038 #10347414 #E3E4E9 #1D1D1F #6E6E73 #FFFFFFCC #F5B33B66 #9B7BD84D",
     "#1D1D1F #7C3AAD #A3611B #C2402A #8B93A1 #C2410C #C2410C #2F5FC4 #0E7C86 #1D1D1F #3E5F7D #1F7FA8 #6B7280 #C2273D #A15C00 #7C3AAD #3E5F7D #2F5FC4 #1D1D1F #1D1D1F #1F7FA8 #A3611B #2E7D32 #C2273D #C7C7CC",
     "#2F5596 #FFFFFF #1E1E21 #E6E6E9 #55555B #A1A1A6 #FFFFFF0A #4A6FB866 #FFFFFF14 #8AB4FF29 #8AB4FF73 #8AB4FF #8AB4FF #FFFFFF10 #252528 #FFFFFF0D #333337 #1E1E21 #1A1A1D #E6E6E9 #98989D #6E6E73 #FFFFFF40 #FFFFFF12 #FFFFFF1F #A1A1A6 #FFFFFF26 #000000A0 #3B66C433 #3A3A3F #F2F2F4 #A1A1A6 #FFFFFF1A #F5B33B4D #B49CF04D",
     "#E6E6E9 #C792EA #E5B97A #F2A07A #7D8799 #F78C6C #F78C6C #82AAFF #7FD1D9 #E6E6E9 #B2CCD6 #89DDFF #9AA3B2 #F07178 #FFCB6B #C792EA #B2CCD6 #82AAFF #E6E6E9 #E6E6E9 #89DDFF #E5B97A #8FD1A0 #F07178 #48484E"),
    ("midnight", "Midnight", "Ivory and midnight blue", True,
     "#103474 #FFF8EC #FFF8EC #1B2A4A #C4B9A5 #6E6450 #1034740A #1034742E #10347414 #10347424 #10347466 #103474 #103474 #1B2A4A14 #F3EADA #1034740D #E6DAC4 #FFF8EC #F9F1E2 #1B2A4A #8C826F #B3A891 #1B2A4A40 #1B2A4A0F #1B2A4A1A #7A705D #1B2A4A29 #1B2A4A47 #1034741A #103474 #FFF8EC #FFF8ECB8 #FFFFFF22 #E8A23866 #9B7BD84D",
     "#1B2A4A #6B3FA0 #9A5B13 #B4441B #9A917F #B4441B #B4441B #1F4FA8 #0F6E7A #1B2A4A #3D5878 #2C6E99 #6D6457 #B02A44 #8C5A00 #6B3FA0 #3D5878 #103474 #1B2A4A #1B2A4A #2C6E99 #9A5B13 #3D7A3A #B02A44 #CFC4AE",
     "#2B4C8F #FFF8EC #0F1A2E #ECE4D4 #3E4D70 #A9B7D6 #FFFFFF0B #3D63B066 #FFFFFF14 #9DBEFF33 #9DBEFF80 #9DBEFF #9DBEFF #FFFFFF12 #0A1322 #FFFFFF0D #1B2842 #0F1A2E #0C1627 #ECE4D4 #7E8BA8 #56637F #FFFFFF38 #FFFFFF12 #FFFFFF1F #A3AFC9 #FFFFFF2E #000000A0 #3B66C433 #2B4C8F #FFF8EC #FFF8ECB8 #FFFFFF22 #F5B33B4D #B49CF04D",
     "#ECE4D4 #C3A6FF #F2C48D #F5A97F #6F7F9C #FF9E80 #FF9E80 #8AB4FF #7FD8E8 #ECE4D4 #B9C9E8 #8FD0F0 #8D9AB5 #FF8FA3 #FFD27F #C3A6FF #B9C9E8 #8AB4FF #ECE4D4 #ECE4D4 #8FD0F0 #F2C48D #9BD8A5 #FF8FA3 #3A4A6C"),
    ("graphite", "Graphite", "Quiet and low-contrast", False,
     "#3B4A66 #FFFFFF #F7F7F6 #2B2B2B #B4B4B4 #6E6E6E #0000000A #3B4A6629 #00000012 #3B4A6620 #3B4A6659 #3B4A66 #3B4A66 #00000012 #EFEFED #0000000A #E0E0DD #F7F7F6 #F2F2F0 #2B2B2B #8A8A88 #B0B0AE #00000040 #0000000D #00000017 #6E6E6C #00000024 #00000038 #3B4A6614 #E2E2DF #2B2B2B #6E6E6C #FFFFFFCC #E0B04A59 #8E7AB84D",
     "#2B2B2B #6E4A87 #6A7A3A #8F5A2E #A0A0A0 #8F5A2E #8F5A2E #3E6283 #3E7470 #2B2B2B #55606A #5A6670 #8A8A8A #8A4B5A #7D6A3E #6E4A87 #55606A #3E6283 #2B2B2B #2B2B2B #3E6283 #6A7A3A #55803F #9A4B4B #C8C8C6",
     "#4E5D7A #FFFFFF #232425 #D6D6D4 #5C5E60 #A8AAAC #FFFFFF0A #6B7C9C59 #FFFFFF12 #9EC0DE26 #9EC0DE66 #9EC0DE #9EC0DE #FFFFFF10 #1C1D1E #FFFFFF0D #333436 #232425 #1F2021 #D6D6D4 #8E9092 #5F6163 #FFFFFF38 #FFFFFF12 #FFFFFF1F #A0A2A4 #FFFFFF26 #000000A0 #6B7C9C33 #38393B #ECECEA #A0A2A4 #FFFFFF1A #E0B04A40 #B4A2D84D",
     "#D6D6D4 #C2A6CF #B8C99A #D8B48A #787A7C #D8B48A #D8B48A #9EC0DE #95C8C0 #D6D6D4 #B6BEC6 #A8B2BA #8A8C8E #D69AA8 #D9C48F #C2A6CF #B6BEC6 #9EC0DE #D6D6D4 #D6D6D4 #9EC0DE #B8C99A #A4C98E #D69AA8 #48494B"),
    ("sepia", "Sepia", "Warm paper tones", False,
     "#2F4A7A #FFFFFF #F4ECD8 #4A3B2A #B8A888 #7A6A4E #5A40200D #8B5A2B33 #5A402014 #2F4A7A20 #2F4A7A59 #2F4A7A #2F4A7A #5A40201A #EBE1C8 #5A402012 #DCCFB0 #F4ECD8 #EFE6CF #4A3B2A #8C7A5E #B3A383 #5A402047 #5A402012 #5A40201F #7A6A4E #5A40202E #3A2A1447 #2F4A7A14 #E2D6BA #4A3B2A #7A6A4E #FFFFFFAA #D9963066 #8B5A9A40",
     "#4A3B2A #8B3A62 #7A5A12 #A0431B #9C8B70 #A0431B #A0431B #2F5D8A #2F6E5E #4A3B2A #5A4E3E #5E6E7E #8A7A62 #9A3A3A #8A6A1A #8B3A62 #5A4E3E #2F5D8A #4A3B2A #4A3B2A #2F5D8A #7A5A12 #4F7A32 #9A3A3A #CDBF9E",
     "#4A6A9E #FFFFFF #2A241C #E8DCC6 #6E6250 #BFAF92 #FFF0D00D #C8A06A40 #FFF0D014 #E8C77A26 #E8C77A66 #E8C77A #E8C77A #FFF0D012 #231E17 #FFF0D00D #3A3226 #2A241C #252019 #E8DCC6 #A8977A #6E6250 #FFF0D038 #FFF0D012 #FFF0D01F #B8A888 #FFF0D026 #000000A0 #C8A06A33 #3A3226 #F2E8D4 #B8A888 #FFF0D01A #E8B05A4D #C9A0D84D",
     "#E8DCC6 #E39AC0 #E5C07B #F29E74 #8C7E66 #F29E74 #F29E74 #8FB8E0 #8FD1BE #E8DCC6 #D6C8AE #B8C4CE #A89A80 #EE9A8A #E8C77A #E39AC0 #D6C8AE #8FB8E0 #E8DCC6 #E8DCC6 #8FB8E0 #E5C07B #A8D08D #EE9A8A #4E4436"),
    ("high-contrast", "High Contrast", "Strongest legibility", False,
     "#0A2A66 #FFFFFF #FFFFFF #000000 #707070 #000000 #0000001A #0A2A6647 #0000001F #0A2A6633 #0A2A66A6 #000000 #000000 #00000033 #F2F2F2 #00000014 #9A9A9A #FFFFFF #F7F7F7 #000000 #404040 #6A6A6A #00000080 #00000014 #00000026 #303030 #00000066 #00000059 #0A2A6626 #D9D9D9 #000000 #404040 #FFFFFFE6 #FFB000A6 #A040FF73",
     "#000000 #9A0089 #7A3E00 #B00000 #4F4F4F #A83200 #A83200 #0035B5 #005E4E #000000 #1A1A6E #003C80 #303030 #A8001E #6E4A00 #9A0089 #1A1A6E #0035B5 #000000 #000000 #003C80 #7A3E00 #006400 #A8001E #9A9A9A",
     "#4F7FE8 #FFFFFF #000000 #FFFFFF #9A9A9A #FFFFFF #FFFFFF1F #4F7FE873 #FFFFFF26 #FFFFFF33 #FFFFFFB3 #FFFFFF #FFFFFF #FFFFFF33 #0D0D0D #FFFFFF14 #6A6A6A #000000 #0A0A0A #FFFFFF #C8C8C8 #9A9A9A #FFFFFF80 #FFFFFF1A #FFFFFF2E #D0D0D0 #FFFFFF66 #000000B3 #4F7FE840 #333333 #FFFFFF #C8C8C8 #FFFFFF26 #FFD24DA6 #C890FF80",
     "#FFFFFF #FF7AD9 #FFD866 #FF9F43 #B0B0B0 #FF9F43 #FF9F43 #6CC1FF #4DFFD2 #FFFFFF #D8E6FF #8FE3FF #D0D0D0 #FF6E8A #FFE07A #FF7AD9 #D8E6FF #6CC1FF #FFFFFF #FFFFFF #8FE3FF #FFD866 #7CFF8F #FF6E8A #5A5A5A"),
]


def palette(name, filled, colors, syntax):
    colors, syntax = colors.split(), syntax.split()
    assert len(colors) == len(FIELDS), (name, len(colors))
    assert len(syntax) == 25, (name, len(syntax))
    lines = [f"    out property <ThemeColors> {name}: {{"]
    lines += [f"        {field}: {color}," for field, color in zip(FIELDS, colors)]
    lines.append(f"        filled-selection: {'true' if filled else 'false'},")
    rows = [", ".join(syntax[i:i + 9]) for i in range(0, 25, 9)]
    lines.append("        syntax: [\n            " + ",\n            ".join(rows) + ",\n        ],")
    lines.append("    };")
    return "\n".join(lines)


def main():
    fields = "\n".join(f"    {f}: color," for f in FIELDS)
    palettes = "\n".join(
        palette(f"{tid}-{mode}", filled, colors, syntax)
        for tid, _, _, filled, lc, ls, dc, ds in THEMES
        for mode, colors, syntax in (("light", lc, ls), ("dark", dc, ds)))
    choose = "\n        : ".join(
        f"theme == {i} ? (dark ? Themes.{tid}-dark : Themes.{tid}-light)" for i, (tid, *_rest) in enumerate(THEMES[1:], 1))
    choose += f"\n        : (dark ? Themes.{THEMES[0][0]}-dark : Themes.{THEMES[0][0]}-light)"
    names = ", ".join(f'"{name}"' for _, name, *_ in THEMES)
    summaries = ", ".join(f'"{summary}"' for _, _, summary, *_ in THEMES)
    ids = ", ".join(f'"{tid}"' for tid, *_ in THEMES)
    lights = ", ".join(f"{tid}-light" for tid, *_ in THEMES)
    darks = ", ".join(f"{tid}-dark" for tid, *_ in THEMES)
    outs = "\n".join(f"    out property <color> {f}: colors.{f};" for f in FIELDS)
    text = f'''// Generated by scripts/make-themes.py from its theme table; edit the table there and rerun.

import {{ Palette }} from "std-widgets.slint";

// The colors of one theme in light or dark. Syntax colors are indexed by `Span.style`: 0 is plain
// text, 1–23 follow HighlightStyle in the core (keyword, string, escape, comment, number,
// constant, function, type, variable, property, operator, punctuation, tag, attribute, label,
// namespace, heading, emphasis, strong, link, code, inserted, deleted), and 24 is invisibles.
export struct ThemeColors {{
{fields}
    // The selected sidebar row is filled with the accent (instead of a quiet highlight).
    filled-selection: bool,
    syntax: [color],
}}

export global Themes {{
    out property <[string]> ids: [{ids}];
    out property <[string]> names: [{names}];
    out property <[string]> summaries: [{summaries}];
{palettes}
    out property <[ThemeColors]> lights: [{lights}];
    out property <[ThemeColors]> darks: [{darks}];
}}

export global Theme {{
    // 0 follows the system, 1 is always light, 2 always dark.
    in-out property <int> appearance: 0;
    // An index into `Themes.ids`.
    in-out property <int> theme: 0;
    out property <bool> system-dark: Palette.color-scheme == ColorScheme.dark;
    out property <bool> dark: appearance == 0 ? system-dark : appearance == 2;
    out property <ThemeColors> colors: {choose};

{outs}
    out property <bool> filled-selection: colors.filled-selection;
    out property <[color]> syntax: colors.syntax;

    // Fonts
    out property <length> ui-font-size: 13px;
    in-out property <string> mono-family: "Menlo";
}}
'''
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    with open(os.path.join(root, "core/tuplepad-app/ui/theme.slint"), "w") as f:
        f.write(text)


if __name__ == "__main__":
    main()
