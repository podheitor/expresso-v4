<script lang="ts">
    // Redirect to mail (default module)
    import { goto } from '$app/navigation';
    import { onMount } from 'svelte';
    onMount(() => goto('/mail'));
</script>
