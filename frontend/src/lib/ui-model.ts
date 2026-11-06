export type ResolvedTheme = "light" | "dark";
export type DensityChoice = "compact" | "standard" | "comfort";
export type NumericSetting = number | null;
export type CreateProfileId = "fast" | "balanced" | "maximum" | "custom";
export type CreateFormatId = "zip" | "7z" | "sqz" | "tar.zst" | "wim";
export type CreateSplitPreset = "none" | "25-mib" | "100-mib" | "700-mib" | "4-gib" | "custom";
export type CreateSplitUnit = "mib" | "gib";
export type CreateSplitMode = "generic" | "native";
export type Screen =
  | "browse"
  | "recent"
  | "create"
  | "extract"
  | "convert"
  | "batch"
  | "nestedExtract"
  | "updateReview"
  | "checksum"
  | "duplicates"
  | "password"
  | "recovery"
  | "archiveInfo"
  | "integration"
  | "appearance"
  | "colors"
  | "settingsGeneral"
  | "settingsSecurity"
  | "settingsPerformance"
  | "passwordBook";
export type PaletteId = "aqua" | "sage" | "nordic" | "copper" | "aubergine" | "mono" | "custom";
export type ChecksumAlgorithmId = ChecksumAlgorithm;
export type Palette = {
  id: PaletteId;
  accent: string;
  support: string;
  base: string;
  darkAccent?: string;
  darkSupport?: string;
  darkBase?: string;
};

type CreateProfile = {
  level: number;
};

type CreateFormat = {
  label: string;
  extension: string;
  extensions: string[];
  can_encrypt_data: boolean;
  can_encrypt_names: boolean;
};

export type ScreenAction = {
  screen: Screen;
  labelKey: string;
  icon: string;
};

export const screenIds: Screen[] = [
  "browse",
  "recent",
  "create",
  "extract",
  "convert",
  "batch",
  "nestedExtract",
  "updateReview",
  "checksum",
  "duplicates",
  "password",
  "recovery",
  "archiveInfo",
  "integration",
  "appearance",
  "colors",
  "settingsGeneral",
  "settingsSecurity",
  "settingsPerformance",
  "passwordBook",
];

export const defaultCustomAccent = "#6E506F";
export const paletteIds: PaletteId[] = ["aqua", "sage", "nordic", "copper", "aubergine", "mono", "custom"];
export const checksumAlgorithms: ChecksumAlgorithmId[] = [
  "sha256",
  "blake3",
  "sha512",
  "sha384",
  "sha224",
  "sha1",
  "md5",
  "crc32",
];
export const createProfileIds: CreateProfileId[] = ["fast", "balanced", "maximum", "custom"];
export const createFormatIds: CreateFormatId[] = ["zip", "7z", "sqz", "tar.zst", "wim"];

export const createProfiles: Record<CreateProfileId, CreateProfile> = {
  fast: { level: 2 },
  balanced: { level: 6 },
  maximum: { level: 9 },
  custom: { level: 6 },
};

export const createFormats: Record<CreateFormatId, CreateFormat> = {
  zip: {
    label: "ZIP",
    extension: "zip",
    extensions: ["zip"],
    can_encrypt_data: true,
    can_encrypt_names: false,
  },
  "7z": {
    label: "7Z",
    extension: "7z",
    extensions: ["7z"],
    can_encrypt_data: true,
    can_encrypt_names: true,
  },
  sqz: {
    label: "SQZ",
    extension: "sqz",
    extensions: ["sqz"],
    can_encrypt_data: false,
    can_encrypt_names: false,
  },
  "tar.zst": {
    label: "TAR.ZST",
    extension: "tar.zst",
    extensions: ["tar.zst", "tzst"],
    can_encrypt_data: false,
    can_encrypt_names: false,
  },
  wim: {
    label: "WIM",
    extension: "wim",
    extensions: ["wim"],
    can_encrypt_data: false,
    can_encrypt_names: false,
  },
};

export const nav = [
  ["archive", "Recent"],
  ["folder-open", "Archives"],
  ["sparkles", "Create"],
  ["archive", "Extract"],
  ["repeat", "Convert"],
  ["check-circle", "Checksum"],
  ["search", "Duplicates"],
  ["shield-alert", "Recovery"],
  ["settings", "Settings"],
];

export const settingsSections: ScreenAction[] = [
  { screen: "settingsGeneral", labelKey: "gui.settings.section.general", icon: "settings" },
  { screen: "appearance", labelKey: "gui.settings.section.appearance", icon: "list" },
  { screen: "colors", labelKey: "gui.settings.section.colors", icon: "palette" },
  { screen: "settingsSecurity", labelKey: "gui.settings.section.security", icon: "shield-alert" },
  { screen: "settingsPerformance", labelKey: "gui.settings.section.performance", icon: "hourglass" },
  { screen: "passwordBook", labelKey: "gui.settings.section.password_book", icon: "lock" },
  { screen: "integration", labelKey: "gui.settings.section.formats_integration", icon: "archive" },
];

export const quickActions: ScreenAction[] = [
  { screen: "extract", labelKey: "gui.quick.extract_archive", icon: "archive" },
  { screen: "batch", labelKey: "gui.quick.batch_extract_review", icon: "list" },
  { screen: "checksum", labelKey: "gui.quick.checksum_files", icon: "check-circle" },
  { screen: "duplicates", labelKey: "gui.quick.find_duplicates", icon: "search" },
  { screen: "settingsSecurity", labelKey: "gui.quick.security_settings", icon: "shield-alert" },
  { screen: "passwordBook", labelKey: "gui.quick.password_book", icon: "lock" },
  { screen: "colors", labelKey: "gui.quick.open_colors", icon: "palette" },
  { screen: "integration", labelKey: "gui.quick.formats_integration", icon: "settings" },
];

export const classicCommands = [
  ["archive", "Add", "accent"],
  ["folder-open", "Extract To", "accent"],
  ["check-circle", "Test", "accent"],
  ["shield-alert", "Protect", "accent"],
  ["eye", "View", "neutral"],
  ["x-circle", "Delete", "danger"],
  ["repeat", "Rename", "neutral"],
  ["repeat", "Move", "neutral"],
  ["folder-open", "New Folder", "neutral"],
  ["check-circle", "Checksum", "accent"],
  ["search", "Duplicates", "accent"],
  ["repeat", "Convert", "neutral"],
  ["info", "Info", "neutral"],
];

export const palettes: Palette[] = [
  {
    id: "aqua",
    accent: "#0A7C86",
    support: "#12805C",
    base: "#F6F8FA",
    darkAccent: "#20C7BE",
    darkSupport: "#46D8CF",
    darkBase: "#12151A",
  },
  {
    id: "sage",
    accent: "#4F7D67",
    support: "#6F7F8B",
    base: "#F7F8F5",
    darkAccent: "#88CAA5",
    darkSupport: "#A6DDBD",
    darkBase: "#12151A",
  },
  {
    id: "nordic",
    accent: "#315F8F",
    support: "#6E879F",
    base: "#F4F7FA",
    darkAccent: "#6DA4DF",
    darkSupport: "#91C4F7",
    darkBase: "#12151A",
  },
  {
    id: "copper",
    accent: "#9A5A2E",
    support: "#344054",
    base: "#F8F6F2",
    darkAccent: "#D59B62",
    darkSupport: "#EFB879",
    darkBase: "#12151A",
  },
  {
    id: "aubergine",
    accent: "#6E506F",
    support: "#5D7280",
    base: "#F7F5F8",
    darkAccent: "#BF93C4",
    darkSupport: "#D7AFD9",
    darkBase: "#12151A",
  },
  {
    id: "mono",
    accent: "#475569",
    support: "#334155",
    base: "#F6F7F9",
    darkAccent: "#B8C4D2",
    darkSupport: "#D1DBE6",
    darkBase: "#12151A",
  },
  {
    id: "custom",
    accent: defaultCustomAccent,
    support: "#64748B",
    base: "#F6F8FA",
  },
];

export const builtInPalettes = palettes.filter((palette) => palette.id !== "custom");
import type { ChecksumAlgorithm } from "./ipc";
export interface FormatCapabilityCard {
  id: string;
  name: string;
  state: string;
  create: string;
  volumes: string;
  encrypt: string;
  note: string;
}
