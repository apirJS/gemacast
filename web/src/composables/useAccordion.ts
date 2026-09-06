import { ref } from 'vue'

export function useAccordion(initialId: string | null = null) {
  const openId = ref<string | null>(initialId)

  const isOpen = (id: string) => openId.value === id

  const toggle = (id: string) => {
    openId.value = isOpen(id) ? null : id
  }

  return { openId, isOpen, toggle }
}
