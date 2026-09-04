import { ref } from "vue";

export function useMenu() {
  const isMenuOpen = ref(false);

  const toggleMenu = () => {
    isMenuOpen.value = !isMenuOpen.value;
  };

  return { isMenuOpen, toggleMenu };
}
