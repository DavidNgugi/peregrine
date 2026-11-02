/**
 * The download queue as card marks, read once for every poster on screen.
 *
 * A grid can hold hundreds of cards, and a poll per card is hundreds of requests for one answer.
 * The first card to mount starts a single poll, the last one to unmount stops it, and every card
 * reads the same map. A transfer in flight is re-read every few seconds so its percent moves; an
 * idle queue is re-read slowly so a download started on another screen still appears.
 */

import { useSyncExternalStore } from "react";

import { api } from "./api.js";
import { cardMarks, type CardMark } from "./download-state.js";

const ACTIVE_MS = 3000;
const IDLE_MS = 15_000;

const EMPTY: ReadonlyMap<number, CardMark> = new Map();

let marks: ReadonlyMap<number, CardMark> = EMPTY;
const listeners = new Set<() => void>();
let timer: number | null = null;
let reading = false;

function schedule(): void {
  const moving = [...marks.values()].some((mark) => mark.tone === "active");
  timer = window.setTimeout(read, moving ? ACTIVE_MS : IDLE_MS);
}

function read(): void {
  timer = null;
  reading = true;
  void api
    .downloads()
    .then((rows) => {
      marks = cardMarks(rows);
      for (const listener of listeners) listener();
    })
    .catch(() => {
      // A dropped read is not an empty queue; the cards keep the last marks until the next one.
    })
    .finally(() => {
      reading = false;
      if (listeners.size > 0) schedule();
    });
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  // A read still in flight from the last screen schedules the next one itself when it lands.
  if (listeners.size === 1 && timer === null && !reading) read();
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== null) {
      window.clearTimeout(timer);
      timer = null;
    }
  };
}

/** The mark for one title, or undefined when it has no download to show. */
export function useCardMark(titleId: number): CardMark | undefined {
  return useSyncExternalStore(
    subscribe,
    () => marks,
    () => EMPTY,
  ).get(titleId);
}
