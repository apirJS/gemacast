<script setup lang="ts">
import { useAccordion } from '@/composables/useAccordion'
import SectionHeading from '@/components/shared/SectionHeading.vue'
import FaqItem from './FaqItem.vue'

interface Question {
  id: string
  question: string
  answer: string
}

const questions: Question[] = [
  {
    id: 'discovery',
    question: 'The phone cannot find my PC',
    answer:
      'Both devices have to be on the same network, so a guest Wi-Fi or a VPN will hide your PC even at full signal. The firewall has to let Gemacast through, which is ports UDP 23555 (for presence), UDP 23556 (for audio stream) and TCP 23559 (for controls). Windows asks you about this once on the first launch, so it stays blocked if you closed that popup.',
  },
  {
    id: 'latency',
    question: 'What is the real end-to-end latency?',
    answer:
      'Add up three things: 10 ms to record the audio on PC, half the round trip (RTT) to send it, and whatever the buffer is holding. The buffer is usually the biggest part. The phone shows you the buffer and the round trip while it plays, so you can add up your own number.',
  },
  {
    id: 'dtim',
    question: 'The buffer grows past 200 ms when the screen turns off',
    answer:
      'Android is saving battery. With the screen off it puts the Wi-Fi chip to sleep and only wakes it up on a set schedule, called DTIM. Audio stops arriving in a steady trickle and starts landing in batch, 100 to 200 ms apart. The buffer grows to cover the longest gap it sees, and that is the expected behavior, because a smaller buffer would just stutter the stream. To keep it low, turn on Keep Screen On in settings, or use USB tether or ADB, where the chip never sleeps.',
  },
  {
    id: 'pairing',
    question: 'Why is there a pairing step?',
    answer:
      'Without it, anything on your network could start a stream, see a list of your running apps, and change what your PC is recording. With pairing, you can allow which phone to connect. You allow it once, the first time you connect, and after that your phone remembers the PC and connects flawlessly.',
  },
  {
    id: 'code',
    question: 'What is the 6-digit code for?',
    answer:
      'It shows that your phone is talking straight to your PC, with nothing in between (no man-in-middle). Each device works out the code by itself, using details only those two share. If some other machine were sitting in the middle, the two codes would come out different and you would see it right away. So just check that both screens show the same six digits, then approve. It is not a password, so it does not matter if someone else sees it.',
  },
  {
    id: 'encryption',
    question: 'Is the audio encrypted?',
    answer:
      'No, and that is on purpose. Pairing is protected, but the audio itself is sent plain. The whole point of this app is low delay, and the audio goes out in tiny pieces, 100 of them every second. Locking and unlocking every one of those adds work at both ends and makes each piece bigger, which is exactly the kind of cost that shows up as delay. So anyone on the same network could listen in if they wanted to. Stick to a network you trust, or use USB.',
  },
]

const { isOpen, toggle } = useAccordion()
</script>

<template>
  <section
    id="faq"
    class="flex flex-col items-center justify-center gap-y-10 px-4 py-20 sm:px-6 sm:py-24 lg:px-10"
  >
    <SectionHeading eyebrow="FAQ" title="Common questions" />

    <ul class="flex w-full max-w-3xl flex-col gap-y-3">
      <FaqItem
        v-for="entry in questions"
        :key="entry.id"
        :question="entry.question"
        :answer="entry.answer"
        :is-open="isOpen(entry.id)"
        @toggle="toggle(entry.id)"
      />
    </ul>
  </section>
</template>
