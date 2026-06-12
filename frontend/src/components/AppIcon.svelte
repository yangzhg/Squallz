<script lang="ts">
  let {
    size = 18,
    class: cls = "",
    title,
  }: { size?: number; class?: string; title?: string } = $props();

  const iconId = $props.id();
  const bgId = `${iconId}-squallz-appicon-bg`;
  const coreId = `${iconId}-squallz-appicon-core`;
  const shadowId = `${iconId}-squallz-appicon-shadow`;
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 512 512"
  fill="none"
  xmlns="http://www.w3.org/2000/svg"
  class={`app-icon${cls ? ` ${cls}` : ""}`}
  role={title ? "img" : undefined}
  aria-label={title}
  aria-hidden={title ? undefined : "true"}
>
  {#if title}
    <title>{title}</title>
  {/if}
  <defs>
    <linearGradient id={bgId} x1="52" y1="44" x2="460" y2="468" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#20C7BE" />
      <stop offset="0.5" stop-color="#0F9FAE" />
      <stop offset="1" stop-color="#2E86C8" />
    </linearGradient>
    <linearGradient id={coreId} x1="164" y1="150" x2="348" y2="364" gradientUnits="userSpaceOnUse">
      <stop offset="0" stop-color="#F9FFFF" />
      <stop offset="1" stop-color="#D7F4FA" />
    </linearGradient>
    <filter id={shadowId} x="108" y="110" width="296" height="300" color-interpolation-filters="sRGB" filterUnits="userSpaceOnUse">
      <feDropShadow dx="0" dy="16" stdDeviation="14" flood-color="#062F3D" flood-opacity="0.24" />
    </filter>
  </defs>

  <rect class="app-icon-bg" x="32" y="32" width="448" height="448" rx="112" fill={`url(#${bgId})`} />

  <g>
    <path class="app-icon-chevron app-icon-chevron-top" d="M204 116L256 168L308 116" />
    <path class="app-icon-chevron app-icon-chevron-bottom" d="M204 396L256 344L308 396" />
    <path class="app-icon-chevron app-icon-chevron-left" d="M116 204L168 256L116 308" />
    <path class="app-icon-chevron app-icon-chevron-right" d="M396 204L344 256L396 308" />
  </g>

  <g class="app-icon-core-shell" filter={`url(#${shadowId})`}>
    <rect x="166" y="162" width="180" height="188" rx="40" fill={`url(#${coreId})`} />
    <rect class="app-icon-core-rim" x="166" y="162" width="180" height="188" rx="40" />
    <path class="app-icon-line app-icon-line-strong" d="M211 220H275" />
    <path class="app-icon-line app-icon-line-soft" d="M211 256H275" />
    <path class="app-icon-line app-icon-line-strong" d="M211 292H275" />
    <path class="app-icon-zipper" d="M302 216V296" />
  </g>
</svg>
