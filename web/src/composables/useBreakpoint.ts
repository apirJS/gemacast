import { onMounted, onUnmounted, ref } from "vue";

export function useBreakpoint(query = "(min-width: 640px)") {
  const isDesktop = ref(false);
  let mql: MediaQueryList | null = null;

  const handleChange = (e: MediaQueryListEvent) => {
    isDesktop.value = e.matches;
  };

  onMounted(() => {
    mql = window.matchMedia(query);
    isDesktop.value = mql.matches;

    mql.addEventListener("change", handleChange);
  });

  onUnmounted(() => {
    mql?.removeEventListener("change", handleChange);
  });

  return { isDesktop };
}
