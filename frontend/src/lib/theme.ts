import { defaultCustomAccent, type Palette, type ResolvedTheme } from "./ui-model";

export type RgbColor = {
  r: number;
  g: number;
  b: number;
};

export type HslColor = {
  h: number;
  s: number;
  l: number;
};

const WHITE = "#FFFFFF";
const NEAR_BLACK = "#071216";
const LIGHT_BASE = "#F6F8FA";
const DARK_BASE = "#121B20";

export function normalizeHexColor(value: string | null | undefined): string | null {
  const trimmed = value?.trim();
  if (!trimmed) return null;
  const short = /^#?([0-9a-fA-F]{3})$/.exec(trimmed);
  if (short) {
    return `#${short[1]
      .split("")
      .map((part) => `${part}${part}`)
      .join("")
      .toUpperCase()}`;
  }
  const full = /^#?([0-9a-fA-F]{6})$/.exec(trimmed);
  return full ? `#${full[1].toUpperCase()}` : null;
}

export function parseHexColor(value: string): RgbColor {
  const normalized = normalizeHexColor(value) ?? defaultCustomAccent;
  return {
    r: Number.parseInt(normalized.slice(1, 3), 16),
    g: Number.parseInt(normalized.slice(3, 5), 16),
    b: Number.parseInt(normalized.slice(5, 7), 16),
  };
}

export function colorToHex(color: RgbColor): string {
  return `#${[color.r, color.g, color.b]
    .map((part) => clampChannel(part).toString(16).padStart(2, "0"))
    .join("")
    .toUpperCase()}`;
}

export function rgbToHsl(color: RgbColor): HslColor {
  const r = color.r / 255;
  const g = color.g / 255;
  const b = color.b / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const delta = max - min;
  const l = (max + min) / 2;

  if (delta === 0) {
    return { h: 0, s: 0, l };
  }

  const s = delta / (1 - Math.abs(2 * l - 1));
  let h = 0;
  if (max === r) {
    h = ((g - b) / delta) % 6;
  } else if (max === g) {
    h = (b - r) / delta + 2;
  } else {
    h = (r - g) / delta + 4;
  }

  return {
    h: (h * 60 + 360) % 360,
    s: Math.max(0, Math.min(1, s)),
    l: Math.max(0, Math.min(1, l)),
  };
}

export function hslToRgb(color: HslColor): RgbColor {
  const h = (((color.h % 360) + 360) % 360) / 360;
  const s = Math.max(0, Math.min(1, color.s));
  const l = Math.max(0, Math.min(1, color.l));

  if (s === 0) {
    const gray = clampChannel(l * 255);
    return { r: gray, g: gray, b: gray };
  }

  const q = l < 0.5 ? l * (1 + s) : l + s - l * s;
  const p = 2 * l - q;
  return {
    r: clampChannel(hueToRgb(p, q, h + 1 / 3) * 255),
    g: clampChannel(hueToRgb(p, q, h) * 255),
    b: clampChannel(hueToRgb(p, q, h - 1 / 3) * 255),
  };
}

export function colorWheelHsl(hex: string): HslColor {
  return rgbToHsl(parseHexColor(hex));
}

export function colorWheelMarkerStyle(hex: string): string {
  const hsl = colorWheelHsl(hex);
  const radians = (hsl.h * Math.PI) / 180;
  const radius = hsl.s * 44;
  const x = 50 + Math.sin(radians) * radius;
  const y = 50 - Math.cos(radians) * radius;
  const neutral = hslToRgb({ h: 0, s: 0, l: hsl.l });
  const neutralHex = colorToHex(neutral);
  const neutralRgba = (alpha: number) => colorToRgba(neutral, alpha);
  const stops = [0, 28, 54, 120, 178, 220, 264, 312, 360]
    .map((hue) => `${colorToHex(hslToRgb({ h: hue, s: 1, l: hsl.l }))} ${hue}deg`)
    .join(", ");
  const background = `radial-gradient(circle, ${neutralHex} 0 10%, ${neutralRgba(0.78)} 25%, ${neutralRgba(0)} 67%), conic-gradient(from 0deg, ${stops})`;
  return `--wheel-x: ${x.toFixed(2)}%; --wheel-y: ${y.toFixed(2)}%; --wheel-marker: ${hex}; --wheel-bg: ${background}`;
}

export function colorFromWheelPoint(hex: string, x: number, y: number, size: number): string {
  const hsl = colorWheelHsl(hex);
  const center = size / 2;
  const dx = x - center;
  const dy = y - center;
  const distance = Math.hypot(dx, dy);
  const maxRadius = size * 0.44;
  const saturation = Math.max(0, Math.min(1, distance / maxRadius));
  const hue = distance < 1 ? hsl.h : ((Math.atan2(dx, -dy) * 180) / Math.PI + 360) % 360;
  return colorToHex(hslToRgb({ h: hue, s: saturation, l: hsl.l }));
}

export function contrastRatio(a: RgbColor, b: RgbColor): number {
  const bright = Math.max(relativeLuminance(a), relativeLuminance(b));
  const dark = Math.min(relativeLuminance(a), relativeLuminance(b));
  return (bright + 0.05) / (dark + 0.05);
}

export function deriveCustomPaletteTokens(
  hex: string,
  theme: ResolvedTheme,
  contrastGuard: boolean,
): Record<string, string> {
  const source = parseHexColor(hex);
  const white = parseHexColor(WHITE);
  const nearBlack = parseHexColor(NEAR_BLACK);
  const luminance = relativeLuminance(source);
  const guardedAccent =
    theme === "dark"
      ? mixColor(source, white, luminance < 0.34 ? 0.45 : 0.24)
      : mixColor(source, nearBlack, luminance > 0.58 ? 0.28 : 0.08);
  const accent = contrastGuard ? guardedAccent : source;
  const accent2 =
    contrastGuard
      ? theme === "dark"
        ? mixColor(accent, white, 0.16)
        : mixColor(accent, relativeLuminance(accent) < 0.2 ? white : nearBlack, 0.12)
      : theme === "dark"
        ? mixColor(accent, white, 0.18)
        : mixColor(accent, nearBlack, 0.1);
  const accentSoft =
    theme === "dark"
      ? colorToRgba(accent, 0.16)
      : colorToHex(mixColor(accent, white, 0.86));
  const accentInk =
    theme === "dark"
      ? colorToHex(mixColor(accent, white, 0.72))
      : colorToHex(mixColor(accent, nearBlack, 0.52));
  const contrastColor = contrastRatio(accent, white) >= 4.5 ? white : nearBlack;
  const accentHex = colorToHex(accent);
  const accent2Hex = colorToHex(accent2);
  const hoverMixTarget = theme === "dark" ? white : nearBlack;
  const accentHoverHex = colorToHex(mixColor(accent, hoverMixTarget, theme === "dark" ? 0.14 : 0.08));
  const accent2HoverHex = colorToHex(mixColor(accent2, hoverMixTarget, theme === "dark" ? 0.18 : 0.12));

  return {
    "--accent": accentHex,
    "--accent-2": accent2Hex,
    "--accent-soft": accentSoft,
    "--accent-ink": accentInk,
    "--accent-shadow": colorToRgba(accent, theme === "dark" ? 0.22 : 0.26),
    "--color-accent": "var(--accent)",
    "--color-accent-strong": "var(--accent-2)",
    "--color-accent-bg": "var(--accent-soft)",
    "--color-accent-fg": "var(--accent-ink)",
    "--color-accent-contrast": colorToHex(contrastColor),
    "--color-accent-border": colorToRgba(accent, theme === "dark" ? 0.38 : 0.34),
    "--color-accent-shadow": "var(--accent-shadow)",
    "--color-focus-ring": colorToRgba(accent, theme === "dark" ? 0.36 : 0.3),
    "--button-primary-bg": `linear-gradient(135deg, ${accentHex}, ${accent2Hex})`,
    "--button-primary-fg": colorToHex(contrastColor),
    "--button-primary-border": colorToRgba(accent, theme === "dark" ? 0.42 : 0.34),
    "--button-primary-bg-hover": `linear-gradient(135deg, ${accentHoverHex}, ${accent2HoverHex})`,
    "--button-primary-fg-hover": colorToHex(contrastColor),
    "--button-selected-bg": accentSoft,
    "--button-selected-fg": accentInk,
    "--button-selected-border": colorToRgba(accent, theme === "dark" ? 0.38 : 0.34),
    "--badge-bg": theme === "dark" ? colorToRgba(accent, 0.16) : colorToHex(mixColor(accent, white, 0.88)),
    "--badge-fg": accentInk,
    "--chip-bg": accentSoft,
    "--chip-fg": accentInk,
    "--chip-border": colorToRgba(accent, theme === "dark" ? 0.38 : 0.34),
  };
}

export function tokenStyle(tokens: Record<string, string>): string {
  return Object.entries(tokens)
    .map(([name, value]) => `${name}: ${value}`)
    .join("; ");
}

export function customPaletteTokenStyle(hex: string, theme: ResolvedTheme, contrastGuard: boolean): string {
  return tokenStyle(deriveCustomPaletteTokens(hex, theme, contrastGuard));
}

export function buildCustomPaletteData(hex: string, theme: ResolvedTheme, contrastGuard: boolean): Palette {
  const tokens = deriveCustomPaletteTokens(hex, theme, contrastGuard);
  const accent = tokens["--accent"];
  const support = tokens["--accent-2"];
  const contrast = contrastRatio(parseHexColor(accent), parseHexColor(tokens["--color-accent-contrast"])).toFixed(1);
  return {
    id: "custom",
    name: "Custom Accent",
    mood: "Personal",
    accent,
    support,
    base: theme === "dark" ? DARK_BASE : LIGHT_BASE,
    note: "Saved custom color with automatic light and dark contrast.",
    contrast: `${contrast}:1`,
  };
}

function clampChannel(value: number): number {
  return Math.max(0, Math.min(255, Math.round(value)));
}

function hueToRgb(p: number, q: number, t: number): number {
  let hue = t;
  if (hue < 0) hue += 1;
  if (hue > 1) hue -= 1;
  if (hue < 1 / 6) return p + (q - p) * 6 * hue;
  if (hue < 1 / 2) return q;
  if (hue < 2 / 3) return p + (q - p) * (2 / 3 - hue) * 6;
  return p;
}

function mixColor(from: RgbColor, to: RgbColor, amount: number): RgbColor {
  const mix = Math.max(0, Math.min(1, amount));
  return {
    r: clampChannel(from.r + (to.r - from.r) * mix),
    g: clampChannel(from.g + (to.g - from.g) * mix),
    b: clampChannel(from.b + (to.b - from.b) * mix),
  };
}

function colorToRgba(color: RgbColor, alpha: number): string {
  return `rgba(${clampChannel(color.r)}, ${clampChannel(color.g)}, ${clampChannel(color.b)}, ${alpha})`;
}

function relativeLuminance(color: RgbColor): number {
  const linear = [color.r, color.g, color.b].map((channel) => {
    const value = channel / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
}
