<script lang="ts">
  import appIconUrl from "../assets/squallz-app-icon.png";

  let {
    size = 18,
    class: cls = "",
    title,
  }: { size?: number; class?: string; title?: string } = $props();
</script>

<img
  src={appIconUrl}
  width={size}
  height={size}
  class={`app-icon${cls ? ` ${cls}` : ""}`}
  alt={title ?? ""}
  title={title}
  aria-hidden={title ? undefined : "true"}
  draggable="false"
/>
