<script lang="ts">
  import ConvertWorkspace from "./ConvertWorkspace.svelte";
  import { convertSessionFor } from "../lib/convert-session.svelte";
  import type {
    ConvertRouteBridge,
    ConvertRouteOwner,
    ConvertWorkspaceVariant,
  } from "../lib/convert-route";

  let {
    variant,
    owner,
    bridge,
  }: {
    variant: ConvertWorkspaceVariant;
    owner: ConvertRouteOwner;
    bridge: ConvertRouteBridge;
  } = $props();

  let session = $derived(convertSessionFor(owner, bridge));
  let surface = $derived(session.surface(variant));

  $effect(() => {
    bridge.register(session);
    session.syncArchive(bridge.getArchive());
  });
</script>

<ConvertWorkspace {variant} {surface} />
