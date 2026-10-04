/** Complete current settings response; individual tests override only their scenario. */
export function settingsDto(overrides = {}) {
  return {
    theme: null, language: null, ui_mode: null, ui_density: null,
    accent_palette: null, custom_accent: null, accent_contrast_guard: null,
    default_create_dir: null, default_extract_dir: null, reveal_after_extract: false,
    check_updates_automatically: true, safety_max_output_bytes: null,
    safety_max_entries: null, safety_max_compression_ratio: null,
    performance_threads: null, performance_memory_limit_bytes: null,
    performance_parallel_jobs: null,
    ...overrides,
  };
}
