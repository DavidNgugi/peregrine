/**
 * The hook layer for the jobs screen: one selector hook for the screen, and the bridge
 * that subscribes to the core's event topics.
 *
 * The bridge is mounted by the shell, not by the screen, so the subscription survives
 * navigation. It subscribes once and returns a cleanup that unlistens, and it drops any
 * envelope it does not recognise rather than rendering it.
 */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { eventTopics } from "@peregrine/ipc-client";
import { useEffect } from "react";
import { useStore } from "zustand";

import { parseJobEvent } from "../job-progress.js";
import { jobsStore, type JobsState } from "../job-store.js";

/** The jobs screen's state and actions. */
export function useJobs(): JobsState {
  return useStore(jobsStore);
}

/**
 * Subscribes to the job streams for as long as the component is mounted.
 *
 * Both topics are delivered by the core's coalescing bus, so this handler runs at most
 * four times a second per topic however fast the job itself reports.
 */
export function useJobsBridge(): void {
  useEffect(() => {
    const unlisteners: UnlistenFn[] = [];
    let cancelled = false;

    const subscribe = (topic: string): void => {
      void listen<unknown>(topic, (event) => {
        const parsed = parseJobEvent(topic, event.payload);
        if (parsed !== null) {
          jobsStore.getState().apply(parsed);
        }
      }).then((unlisten) => {
        if (cancelled) {
          // The effect was cleaned up before the listener registered, which happens when a
          // screen mounts and unmounts inside one tick; unlistening now is what keeps the
          // bridge from leaking a listener per mount.
          unlisten();
        } else {
          unlisteners.push(unlisten);
        }
      });
    };

    subscribe(eventTopics.jobProgress.name);
    subscribe(eventTopics.jobState.name);

    return () => {
      cancelled = true;
      for (const unlisten of unlisteners) {
        unlisten();
      }
    };
  }, []);
}
