export type ResolvedTheme = "light" | "dark";
export type DensityChoice = "compact" | "standard" | "comfort";
export type NumericSetting = number | null;
export type CreateProfileId = "fast" | "balanced" | "maximum" | "custom";
export type CreateFormatId = "zip" | "7z" | "sqz" | "tar.zst" | "wim";
export type Screen =
  | "browse"
  | "recent"
  | "create"
  | "extract"
  | "convert"
  | "batch"
  | "checksum"
  | "duplicates"
  | "password"
  | "conflict"
  | "cannotRepair"
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
export type ChecksumAlgorithmId =
  | "sha256"
  | "sha224"
  | "sha384"
  | "sha512"
  | "sha1"
  | "md5"
  | "blake3"
  | "crc32";
export type Palette = {
  id: PaletteId;
  name: string;
  mood: string;
  accent: string;
  support: string;
  base: string;
  darkAccent?: string;
  darkSupport?: string;
  darkBase?: string;
  note: string;
  contrast: string;
  darkContrast?: string;
};

type CreateProfile = {
  label: string;
  level: number;
  detail: string;
};

type CreateFormat = {
  label: string;
  extension: string;
  filterName: string;
  extensions: string[];
  method: string;
  password: string;
  can_encrypt_data: boolean;
  can_encrypt_names: boolean;
  split: string;
  recovery: string;
  note: string;
};

export type ScreenAction = {
  screen: Screen;
  label: string;
  icon: string;
  detail: string;
};

export const screenIds: Screen[] = [
  "browse",
  "recent",
  "create",
  "extract",
  "convert",
  "batch",
  "checksum",
  "duplicates",
  "password",
  "conflict",
  "cannotRepair",
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
  fast: { label: "Fast", level: 2, detail: "Quick sharing, lower CPU pressure" },
  balanced: { label: "Balanced", level: 6, detail: "Good ratio without making the Mac feel busy" },
  maximum: { label: "Maximum", level: 9, detail: "Best ratio for long-term storage" },
  custom: { label: "Custom", level: 6, detail: "User-defined level saved for future jobs" },
};

export const createFormats: Record<CreateFormatId, CreateFormat> = {
  zip: {
    label: "ZIP",
    extension: "zip",
    filterName: "ZIP archive",
    extensions: ["zip"],
    method: "Deflate",
    password: "Data encryption only; file names remain visible",
    can_encrypt_data: true,
    can_encrypt_names: false,
    split: "ZIP split is supported through .zip/.zNN sets",
    recovery: "Use PAR2 after create for open-format recovery",
    note: "Best for sharing with Windows and built-in tools",
  },
  "7z": {
    label: "7Z",
    extension: "7z",
    filterName: "7Z archive",
    extensions: ["7z"],
    method: "LZMA2",
    password: "AES-256 can encrypt file names",
    can_encrypt_data: true,
    can_encrypt_names: true,
    split: "Byte split volumes supported",
    recovery: "Use PAR2 sidecar for standard 7Z recovery",
    note: "Best ratio and privacy for long-term archives",
  },
  sqz: {
    label: "SQZ",
    extension: "sqz",
    filterName: "SQZ archive",
    extensions: ["sqz"],
    method: "Squallz container",
    password: "No launch encryption claim; use standard encrypted inner archives when needed",
    can_encrypt_data: false,
    can_encrypt_names: false,
    split: "SQZV split volumes with recovery sidecars",
    recovery: "Embedded recovery is available through SQZ workflows",
    note: "Best when self-repair and export are more important than ubiquity",
  },
  "tar.zst": {
    label: "TAR.ZST",
    extension: "tar.zst",
    filterName: "TAR.ZST archive",
    extensions: ["tar.zst", "tzst"],
    method: "TAR + Zstandard",
    password: "No built-in encryption",
    can_encrypt_data: false,
    can_encrypt_names: false,
    split: "No native split claim",
    recovery: "Use PAR2 sidecar after create",
    note: "Fast developer archives with strong compression speed",
  },
  wim: {
    label: "WIM",
    extension: "wim",
    filterName: "WIM image",
    extensions: ["wim"],
    method: "External wimlib-imagex capture",
    password: "No Squallz password layer for WIM",
    can_encrypt_data: false,
    can_encrypt_names: false,
    split: "Split WIM is not a launch claim",
    recovery: "Use PAR2 sidecar after image creation",
    note: "Requires SQUALLZ_WIMLIB or wimlib-imagex on PATH",
  },
};

export const moveTargetPresets = ["moved/", "reports/", "screenshots/"];

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
  { screen: "settingsGeneral", label: "General", icon: "settings", detail: "Startup, language, defaults" },
  { screen: "appearance", label: "Appearance", icon: "list", detail: "Display settings and interface mode" },
  { screen: "colors", label: "Colors", icon: "palette", detail: "Palettes and custom accent" },
  { screen: "settingsSecurity", label: "Security", icon: "shield-alert", detail: "Safety limits and extraction guards" },
  { screen: "settingsPerformance", label: "Performance", icon: "hourglass", detail: "Workers, task flow, scale limits" },
  { screen: "passwordBook", label: "Password Book", icon: "lock", detail: "Secret-store status and saved archive secrets" },
  { screen: "integration", label: "File Associations", icon: "archive", detail: "Open With and file-manager actions" },
];

export const quickActions: ScreenAction[] = [
  { screen: "extract", label: "Extract selected", icon: "archive", detail: "Destination, conflicts, password" },
  { screen: "batch", label: "Batch extract review", icon: "list", detail: "Review all archives before starting" },
  { screen: "checksum", label: "Checksum files", icon: "check-circle", detail: "SHA-256, BLAKE3, CRC32" },
  { screen: "duplicates", label: "Find duplicates", icon: "search", detail: "BLAKE3 scan with CLI parity" },
  { screen: "settingsSecurity", label: "Security settings", icon: "shield-alert", detail: "Safety limits and guards" },
  { screen: "passwordBook", label: "Password Book", icon: "lock", detail: "Saved archive password status" },
  { screen: "colors", label: "Open Colors", icon: "palette", detail: "Accent palette and contrast guard" },
  { screen: "integration", label: "File Associations", icon: "settings", detail: "Open With and file-manager actions" },
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

export const contextActions = [
  "Extract Here",
  "Extract to <archive>/",
  "Add to archive...",
  "Compress to ZIP",
  "Compress to 7Z",
  "Protect with PAR2",
  "Test archive",
  "Repair archive",
  "Export SQZ",
  "Convert archive",
];

export const recoveryModes = [
  { name: "PAR2 sidecar", detail: "Keeps .7z unchanged", size: "+3.8 GB", tone: "safe" },
  { name: "SQZ container", detail: "Payload self-repair and export", size: "+2/8 shards", tone: "self" },
  { name: "Dual protection", detail: "Run SQZ and PAR2 as separate passes", size: "Separate jobs", tone: "strong" },
];

export const recoveryBlocks = [
  { group: "G0", data: "192", recovery: "24", damage: "0", status: "OK" },
  { group: "G1", data: "192", recovery: "24", damage: "2", status: "Repairable" },
  { group: "G2", data: "188", recovery: "24", damage: "0", status: "OK" },
];

export const palettes: Palette[] = [
  {
    id: "aqua",
    name: "Aqua Graphite",
    mood: "Default",
    accent: "#0A7C86",
    support: "#12805C",
    base: "#F6F8FA",
    darkAccent: "#20C7BE",
    darkSupport: "#46D8CF",
    darkBase: "#12151A",
    note: "Clean teal, graphite neutrals, calm utility feel.",
    contrast: "7.3:1",
    darkContrast: "6.6:1",
  },
  {
    id: "sage",
    name: "Sage Titanium",
    mood: "Quiet",
    accent: "#4F7D67",
    support: "#6F7F8B",
    base: "#F7F8F5",
    darkAccent: "#88CAA5",
    darkSupport: "#A6DDBD",
    darkBase: "#12151A",
    note: "Muted sage with titanium gray for restrained desktop work.",
    contrast: "6.6:1",
    darkContrast: "7.8:1",
  },
  {
    id: "nordic",
    name: "Nordic Blue",
    mood: "Precise",
    accent: "#315F8F",
    support: "#6E879F",
    base: "#F4F7FA",
    darkAccent: "#6DA4DF",
    darkSupport: "#91C4F7",
    darkBase: "#12151A",
    note: "Cool professional blue without turning the app into one flat hue.",
    contrast: "7.8:1",
    darkContrast: "6.4:1",
  },
  {
    id: "copper",
    name: "Ink Copper",
    mood: "Premium",
    accent: "#9A5A2E",
    support: "#344054",
    base: "#F8F6F2",
    darkAccent: "#D59B62",
    darkSupport: "#EFB879",
    darkBase: "#12151A",
    note: "Ink neutrals with a restrained copper accent for classic tools.",
    contrast: "6.9:1",
    darkContrast: "6.9:1",
  },
  {
    id: "aubergine",
    name: "Aubergine Steel",
    mood: "Editorial",
    accent: "#6E506F",
    support: "#5D7280",
    base: "#F7F5F8",
    darkAccent: "#BF93C4",
    darkSupport: "#D7AFD9",
    darkBase: "#12151A",
    note: "Deep wine-plum balanced by steel so it avoids purple overload.",
    contrast: "7.1:1",
    darkContrast: "7.1:1",
  },
  {
    id: "mono",
    name: "Monochrome Pro",
    mood: "Minimal",
    accent: "#475569",
    support: "#334155",
    base: "#F6F7F9",
    darkAccent: "#B8C4D2",
    darkSupport: "#D1DBE6",
    darkBase: "#12151A",
    note: "Graphite-first UI for users who want the app to disappear.",
    contrast: "8.4:1",
    darkContrast: "9.2:1",
  },
  {
    id: "custom",
    name: "Custom Accent",
    mood: "Personal",
    accent: defaultCustomAccent,
    support: "#64748B",
    base: "#F6F8FA",
    note: "Saved custom color with automatic light and dark contrast.",
    contrast: "auto",
  },
];

export const builtInPalettes = palettes.filter((palette) => palette.id !== "custom");
